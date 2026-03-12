use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::config::get_conf;
use crate::ssh::run_cmd;

/// Tracks a currently-mounted disk
#[derive(Debug, Clone, Serialize)]
pub struct MountedDisk {
    pub disk_name: String,
    pub nbd_device: String,
    pub mount_point: String,
    pub partition_path: String,
    pub mounted_at: String,
    pub lvm_vg: Option<String>,
    /// macOS only: path to temporary raw file for qcow2↔raw conversion
    pub raw_file: Option<String>,
    /// true if mounted read-only (e.g. ext4fuse on macOS)
    pub read_only: bool,
}

pub type MountedDiskStore = Arc<Mutex<HashMap<String, MountedDisk>>>;

/// Entry in a directory listing
#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: String,
    pub permissions: String,
}

// ──────────────────────────────────────────
// Linux implementation
// ──────────────────────────────────────────

#[cfg(target_os = "linux")]
fn find_free_nbd() -> Result<String, String> {
    for i in 0..16 {
        let pid_path = format!("/sys/block/nbd{}/pid", i);
        match std::fs::read_to_string(&pid_path) {
            Err(_) => return Ok(format!("/dev/nbd{}", i)),
            Ok(content) if content.trim().is_empty() || content.trim() == "0" => {
                return Ok(format!("/dev/nbd{}", i));
            }
            _ => continue,
        }
    }
    Err("No free NBD devices available (all /dev/nbd0-15 in use)".into())
}

#[cfg(target_os = "linux")]
fn list_nbd_partitions(nbd_dev: &str) -> Vec<String> {
    let mut parts = Vec::new();
    // Wait a moment for kernel to detect partitions
    std::thread::sleep(std::time::Duration::from_millis(500));
    for i in 1..=16 {
        let p = format!("{}p{}", nbd_dev, i);
        if std::path::Path::new(&p).exists() {
            parts.push(p);
        }
    }
    parts
}

#[cfg(target_os = "linux")]
fn discover_partition(nbd_dev: &str, sudo: &str) -> Result<(String, Option<String>), String> {
    let partitions = list_nbd_partitions(nbd_dev);

    if partitions.is_empty() {
        // Try the device itself (no partition table — raw filesystem)
        return Ok((nbd_dev.to_string(), None));
    }

    // Try each partition for LVM first
    for part in &partitions {
        if let Ok(output) = run_cmd(sudo, &["pvs", "--noheadings", "-o", "vg_name", part]) {
            let vg_name = output.trim().to_string();
            if !vg_name.is_empty() {
                // Activate the VG
                run_cmd(sudo, &["vgchange", "-ay", &vg_name])
                    .map_err(|e| format!("LVM vgchange failed: {}", e))?;
                std::thread::sleep(std::time::Duration::from_millis(500));

                // Find LVs
                if let Ok(lv_output) =
                    run_cmd(sudo, &["lvs", "--noheadings", "-o", "lv_path", &vg_name])
                {
                    for lv_line in lv_output.lines() {
                        let lv_path = lv_line.trim();
                        if !lv_path.is_empty() {
                            return Ok((lv_path.to_string(), Some(vg_name)));
                        }
                    }
                }
            }
        }
    }

    // No LVM — use the last (largest) partition
    let best = partitions.last().unwrap();
    Ok((best.clone(), None))
}

#[cfg(target_os = "linux")]
fn cleanup_nbd(nbd_dev: &str, sudo: &str, lvm_vg: &Option<String>) {
    if let Some(ref vg) = lvm_vg {
        let _ = run_cmd(sudo, &["vgchange", "-an", vg]);
    }
    let qemu_nbd = get_conf("qemu_nbd_path");
    let _ = run_cmd(sudo, &[&qemu_nbd, "--disconnect", nbd_dev]);
}

#[cfg(target_os = "linux")]
pub fn mount_disk(disk_name: &str, store: &MountedDiskStore) -> Result<MountedDisk, String> {
    crate::ssh::sanitize_name(disk_name)?;

    // Check not already mounted
    {
        let locked = store.lock().map_err(|e| format!("Lock error: {}", e))?;
        if locked.contains_key(disk_name) {
            return Err(format!("Disk '{}' is already mounted", disk_name));
        }
    }

    // Check disk not in use by running VM
    crate::operations::check_disk_not_in_use(disk_name)?;

    // Verify disk file exists
    let disk_path = get_conf("disk_path");
    let qcow2_file = format!("{}/{}.qcow2", disk_path, disk_name);
    if !std::path::Path::new(&qcow2_file).exists() {
        return Err(format!("Disk file not found: {}", qcow2_file));
    }

    let sudo = get_conf("bridge_sudo_path");

    // Load NBD kernel module
    let _ = run_cmd(&sudo, &["modprobe", "nbd", "max_part=16"]);

    // Find free NBD device
    let nbd_dev = find_free_nbd()?;

    // Attach qcow2 to NBD
    let qemu_nbd = get_conf("qemu_nbd_path");
    run_cmd(&sudo, &[&qemu_nbd, "--connect", &nbd_dev, &qcow2_file])
        .map_err(|e| format!("qemu-nbd connect failed: {}", e))?;

    // Discover partitions (+ handle LVM)
    let (part_path, lvm_vg) = match discover_partition(&nbd_dev, &sudo) {
        Ok(r) => r,
        Err(e) => {
            cleanup_nbd(&nbd_dev, &sudo, &None);
            return Err(e);
        }
    };

    // Create mount point
    let mount_base = get_conf("disk_mount_base");
    let mount_point = format!("{}/{}", mount_base, disk_name);
    let _ = std::fs::create_dir_all(&mount_point);

    // Mount
    if let Err(e) = run_cmd(&sudo, &["mount", "-o", "rw", &part_path, &mount_point]) {
        cleanup_nbd(&nbd_dev, &sudo, &lvm_vg);
        return Err(format!("Mount failed: {}", e));
    }

    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let info = MountedDisk {
        disk_name: disk_name.to_string(),
        nbd_device: nbd_dev,
        mount_point,
        partition_path: part_path,
        mounted_at: now,
        lvm_vg,
        raw_file: None,
        read_only: false,
    };

    store.lock().map_err(|e| format!("Lock error: {}", e))?.insert(disk_name.to_string(), info.clone());
    Ok(info)
}

#[cfg(target_os = "linux")]
pub fn unmount_disk(disk_name: &str, store: &MountedDiskStore) -> Result<(), String> {
    let info = {
        let locked = store.lock().map_err(|e| format!("Lock error: {}", e))?;
        locked
            .get(disk_name)
            .cloned()
            .ok_or_else(|| format!("Disk '{}' is not mounted", disk_name))?
    };

    let sudo = get_conf("bridge_sudo_path");

    // Unmount
    run_cmd(&sudo, &["umount", &info.mount_point])
        .map_err(|e| format!("Unmount failed: {}", e))?;

    // Cleanup LVM if used
    cleanup_nbd(&info.nbd_device, &sudo, &info.lvm_vg);

    // Cleanup mount point
    let _ = std::fs::remove_dir(&info.mount_point);

    // Remove from store
    store.lock().map_err(|e| format!("Lock error: {}", e))?.remove(disk_name);
    Ok(())
}

/// Cleanup all stale mounts from a previous server run
#[cfg(target_os = "linux")]
pub fn cleanup_stale_mounts() {
    let mount_base = get_conf("disk_mount_base");
    if !std::path::Path::new(&mount_base).exists() {
        return;
    }
    let sudo = get_conf("bridge_sudo_path");
    if let Ok(entries) = std::fs::read_dir(&mount_base) {
        for entry in entries.flatten() {
            let path = entry.path();
            let _ = run_cmd(&sudo, &["umount", &path.to_string_lossy()]);
            let _ = std::fs::remove_dir(&path);
        }
    }
    // Disconnect all NBD devices
    let qemu_nbd = get_conf("qemu_nbd_path");
    for i in 0..16 {
        let dev = format!("/dev/nbd{}", i);
        let _ = run_cmd(&sudo, &[&qemu_nbd, "--disconnect", &dev]);
    }
}

// ──────────────────────────────────────────
// File operations (cross-platform, work on mount point)
// ──────────────────────────────────────────

fn sanitize_path(path: &str) -> Result<String, String> {
    let cleaned = path.trim_start_matches('/');
    if cleaned.contains("..") {
        return Err("Path cannot contain '..'".into());
    }
    for c in cleaned.chars() {
        if c.is_control() || c == '\0' {
            return Err("Path contains invalid characters".into());
        }
    }
    Ok(cleaned.to_string())
}

fn resolve_safe_path(mount_point: &str, rel_path: &str) -> Result<String, String> {
    let clean = sanitize_path(rel_path)?;
    let full = if clean.is_empty() {
        mount_point.to_string()
    } else {
        format!("{}/{}", mount_point, clean)
    };
    let canonical = std::fs::canonicalize(&full)
        .map_err(|e| format!("Path not found: {}", e))?;
    let mount_canonical = std::fs::canonicalize(mount_point)
        .map_err(|e| format!("Mount point error: {}", e))?;
    if !canonical.starts_with(&mount_canonical) {
        return Err("Access denied: path outside disk".into());
    }
    Ok(canonical.to_string_lossy().to_string())
}

fn get_mount_info(disk_name: &str, store: &MountedDiskStore) -> Result<MountedDisk, String> {
    let locked = store.lock().map_err(|e| format!("Lock error: {}", e))?;
    locked
        .get(disk_name)
        .cloned()
        .ok_or_else(|| format!("Disk '{}' is not mounted", disk_name))
}

#[cfg(unix)]
fn format_permissions(meta: &std::fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    let mut s = String::with_capacity(9);
    let flags = [
        (0o400, 'r'), (0o200, 'w'), (0o100, 'x'),
        (0o040, 'r'), (0o020, 'w'), (0o010, 'x'),
        (0o004, 'r'), (0o002, 'w'), (0o001, 'x'),
    ];
    for (bit, ch) in &flags {
        s.push(if mode & bit != 0 { *ch } else { '-' });
    }
    s
}

#[cfg(not(unix))]
fn format_permissions(_meta: &std::fs::Metadata) -> String {
    "---------".to_string()
}

fn format_mtime(meta: &std::fs::Metadata) -> String {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| {
            let secs = d.as_secs() as i64;
            // Simple UTC timestamp
            let dt = chrono::DateTime::from_timestamp(secs, 0);
            dt.map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

pub fn list_files(
    disk_name: &str,
    rel_path: &str,
    store: &MountedDiskStore,
) -> Result<Vec<FileEntry>, String> {
    let info = get_mount_info(disk_name, store)?;
    let full_path = resolve_safe_path(&info.mount_point, rel_path)?;

    let mut entries = Vec::new();
    let dir =
        std::fs::read_dir(&full_path).map_err(|e| format!("Cannot read directory: {}", e))?;

    for entry in dir.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().to_string();
        let clean_rel = sanitize_path(rel_path).unwrap_or_default();
        let entry_path = if clean_rel.is_empty() {
            format!("/{}", name)
        } else {
            format!("/{}/{}", clean_rel, name)
        };

        entries.push(FileEntry {
            name,
            path: entry_path,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified: format_mtime(&meta),
            permissions: format_permissions(&meta),
        });
    }

    // Sort: directories first, then alphabetical
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(entries)
}

pub fn read_file(
    disk_name: &str,
    rel_path: &str,
    store: &MountedDiskStore,
) -> Result<String, String> {
    let info = get_mount_info(disk_name, store)?;
    let full_path = resolve_safe_path(&info.mount_point, rel_path)?;

    let meta =
        std::fs::metadata(&full_path).map_err(|e| format!("Cannot stat file: {}", e))?;
    if meta.is_dir() {
        return Err("Cannot read a directory as a file".into());
    }
    if meta.len() > 1_048_576 {
        return Err("File too large for text editor (max 1MB)".into());
    }

    std::fs::read_to_string(&full_path).map_err(|e| format!("Cannot read file: {}", e))
}

pub fn write_file(
    disk_name: &str,
    rel_path: &str,
    content: &str,
    store: &MountedDiskStore,
) -> Result<(), String> {
    let info = get_mount_info(disk_name, store)?;

    if info.read_only {
        return Err("Disk is mounted read-only (ext4fuse). Install fuse-ext2 for write support.".into());
    }

    let full_path = resolve_safe_path(&info.mount_point, rel_path)?;

    // Re-check disk is not in use (race protection)
    crate::operations::check_disk_not_in_use(&info.disk_name)?;

    std::fs::write(&full_path, content).map_err(|e| format!("Cannot write file: {}", e))
}

// ──────────────────────────────────────────
// macOS implementation (qemu-img convert + hdiutil + fuse-ext2/ext4fuse)
// ──────────────────────────────────────────

/// Check which ext4 FUSE tool is available.
/// Returns (tool_path, is_read_write).
#[cfg(target_os = "macos")]
fn find_ext4_tool() -> Result<(String, bool), String> {
    // Try fuse-ext2 first (supports read-write)
    if let Ok(output) = run_cmd("which", &["fuse-ext2"]) {
        let path = output.trim().to_string();
        if !path.is_empty() {
            return Ok((path, true));
        }
    }
    // Fall back to ext4fuse (read-only)
    if let Ok(output) = run_cmd("which", &["ext4fuse"]) {
        let path = output.trim().to_string();
        if !path.is_empty() {
            return Ok((path, false));
        }
    }
    Err(
        "ext4 support not found.\nInstall with:\n  brew install --cask macfuse\n  brew install fuse-ext2\nOr read-only:\n  brew install ext4fuse"
            .into(),
    )
}

/// Check if a device path is a partition slice (e.g. /dev/disk4s2 vs /dev/disk4)
#[cfg(target_os = "macos")]
fn is_partition_slice(dev: &str) -> bool {
    if let Some(s_pos) = dev.rfind('s') {
        if s_pos > 0
            && !dev[s_pos + 1..].is_empty()
            && dev[s_pos + 1..].chars().all(|c| c.is_ascii_digit())
        {
            return dev.as_bytes()[s_pos - 1].is_ascii_digit();
        }
    }
    false
}

/// Parse hdiutil attach output to find the main disk device and the Linux partition.
/// Returns (disk_device, partition_device).
#[cfg(target_os = "macos")]
fn find_linux_partition(hdiutil_output: &str) -> Result<(String, String), String> {
    let mut disk_dev = String::new();
    let mut candidates = Vec::new();

    for line in hdiutil_output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let dev = parts[0];
        if !dev.starts_with("/dev/disk") {
            continue;
        }

        let type_hint = if parts.len() > 1 {
            parts[1..].join(" ").to_lowercase()
        } else {
            String::new()
        };

        if is_partition_slice(dev) {
            // Skip EFI, partition map, swap
            if type_hint.contains("efi")
                || type_hint.contains("partition_scheme")
                || type_hint.contains("partition_map")
                || type_hint.contains("swap")
            {
                continue;
            }
            candidates.push(dev.to_string());
        } else {
            disk_dev = dev.to_string();
        }
    }

    if disk_dev.is_empty() {
        return Err("No disk device found in hdiutil output".into());
    }

    if candidates.is_empty() {
        // No partition table — raw filesystem on whole disk
        return Ok((disk_dev.clone(), disk_dev));
    }

    // Pick the last non-EFI partition (usually the main filesystem)
    let partition = candidates.last().unwrap().clone();
    Ok((disk_dev, partition))
}

#[cfg(target_os = "macos")]
pub fn mount_disk(disk_name: &str, store: &MountedDiskStore) -> Result<MountedDisk, String> {
    crate::ssh::sanitize_name(disk_name)?;

    // Check not already mounted
    {
        let locked = store.lock().map_err(|e| format!("Lock error: {}", e))?;
        if locked.contains_key(disk_name) {
            return Err(format!("Disk '{}' is already mounted", disk_name));
        }
    }

    // Check disk not in use by running VM
    crate::operations::check_disk_not_in_use(disk_name)?;

    // Find ext4 FUSE tool
    let (ext4_tool, is_rw) = find_ext4_tool()?;

    // Verify disk file exists
    let disk_path = get_conf("disk_path");
    let qcow2_file = format!("{}/{}.qcow2", disk_path, disk_name);
    if !std::path::Path::new(&qcow2_file).exists() {
        return Err(format!("Disk file not found: {}", qcow2_file));
    }

    let mount_base = get_conf("disk_mount_base");
    let _ = std::fs::create_dir_all(&mount_base);
    let raw_file = format!("{}/{}.raw", mount_base, disk_name);
    let mount_point = format!("{}/{}", mount_base, disk_name);
    let _ = std::fs::create_dir_all(&mount_point);

    // Step 1: Convert qcow2 → raw
    eprintln!("[disk-edit] Converting {} to raw...", qcow2_file);
    let qemu_img = get_conf("qemu_img_path");
    if let Err(e) = run_cmd(
        &qemu_img,
        &["convert", "-f", "qcow2", "-O", "raw", &qcow2_file, &raw_file],
    ) {
        let _ = std::fs::remove_file(&raw_file);
        return Err(format!("qemu-img convert to raw failed: {}", e));
    }

    // Step 2: Attach raw image with hdiutil
    eprintln!("[disk-edit] Attaching raw image with hdiutil...");
    let hdiutil_output = match run_cmd("hdiutil", &["attach", "-nomount", &raw_file]) {
        Ok(out) => out,
        Err(e) => {
            let _ = std::fs::remove_file(&raw_file);
            return Err(format!("hdiutil attach failed: {}", e));
        }
    };

    // Step 3: Find disk device and Linux partition
    let (disk_dev, partition) = match find_linux_partition(&hdiutil_output) {
        Ok(r) => r,
        Err(e) => {
            // Try to detach whatever was attached
            for line in hdiutil_output.lines() {
                let dev = line.split_whitespace().next().unwrap_or("");
                if dev.starts_with("/dev/disk") && !is_partition_slice(dev) {
                    let _ = run_cmd("hdiutil", &["detach", dev]);
                    break;
                }
            }
            let _ = std::fs::remove_file(&raw_file);
            return Err(e);
        }
    };

    // Step 4: Mount partition using ext4 FUSE tool
    eprintln!(
        "[disk-edit] Mounting {} at {} using {} (rw={})",
        partition, mount_point, ext4_tool, is_rw
    );
    let mount_result = if ext4_tool.contains("fuse-ext2") {
        run_cmd(&ext4_tool, &[&partition, &mount_point, "-o", "rw+"])
    } else {
        // ext4fuse (read-only)
        run_cmd(&ext4_tool, &[&partition, &mount_point])
    };

    if let Err(e) = mount_result {
        let _ = run_cmd("hdiutil", &["detach", &disk_dev]);
        let _ = std::fs::remove_file(&raw_file);
        return Err(format!("ext4 mount failed: {}", e));
    }

    let now = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let info = MountedDisk {
        disk_name: disk_name.to_string(),
        nbd_device: disk_dev, // stores /dev/diskN for hdiutil detach
        mount_point,
        partition_path: partition,
        mounted_at: now,
        lvm_vg: None,
        raw_file: Some(raw_file),
        read_only: !is_rw,
    };

    store
        .lock()
        .unwrap()
        .insert(disk_name.to_string(), info.clone());
    Ok(info)
}

#[cfg(target_os = "macos")]
pub fn unmount_disk(disk_name: &str, store: &MountedDiskStore) -> Result<(), String> {
    let info = {
        let locked = store.lock().map_err(|e| format!("Lock error: {}", e))?;
        locked
            .get(disk_name)
            .cloned()
            .ok_or_else(|| format!("Disk '{}' is not mounted", disk_name))?
    };

    // Step 1: Unmount FUSE filesystem
    run_cmd("umount", &[&info.mount_point])
        .map_err(|e| format!("Unmount failed: {}", e))?;

    // Step 2: Detach hdiutil device
    let _ = run_cmd("hdiutil", &["detach", &info.nbd_device]);

    // Step 3: If read-write, convert raw back to qcow2
    if !info.read_only {
        if let Some(ref raw_file) = info.raw_file {
            let disk_path = get_conf("disk_path");
            let qcow2_file = format!("{}/{}.qcow2", disk_path, disk_name);
            let qemu_img = get_conf("qemu_img_path");

            // Convert to a temp file first for safety
            let temp_qcow2 = format!("{}.tmp.qcow2", qcow2_file);
            eprintln!(
                "[disk-edit] Converting raw back to qcow2: {} → {}",
                raw_file, qcow2_file
            );
            match run_cmd(
                &qemu_img,
                &["convert", "-f", "raw", "-O", "qcow2", raw_file, &temp_qcow2],
            ) {
                Ok(_) => {
                    if let Err(e) = std::fs::rename(&temp_qcow2, &qcow2_file) {
                        // Rename failed — try to clean up temp
                        let _ = std::fs::remove_file(&temp_qcow2);
                        eprintln!("[disk-edit] WARNING: rename failed: {}", e);
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&temp_qcow2);
                    // Don't fail the unmount — original qcow2 is still intact
                    eprintln!(
                        "[disk-edit] WARNING: raw→qcow2 conversion failed: {}. Original disk unchanged.",
                        e
                    );
                }
            }
        }
    }

    // Step 4: Cleanup raw file and mount point
    if let Some(ref raw_file) = info.raw_file {
        let _ = std::fs::remove_file(raw_file);
    }
    let _ = std::fs::remove_dir(&info.mount_point);

    store.lock().map_err(|e| format!("Lock error: {}", e))?.remove(disk_name);
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn cleanup_stale_mounts() {
    let mount_base = get_conf("disk_mount_base");
    if !std::path::Path::new(&mount_base).exists() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(&mount_base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = run_cmd("umount", &[&path.to_string_lossy()]);
                let _ = std::fs::remove_dir(&path);
            } else if path.extension().map(|e| e == "raw").unwrap_or(false) {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

// ──────────────────────────────────────────
// Windows stubs
// ──────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn mount_disk(_disk_name: &str, _store: &MountedDiskStore) -> Result<MountedDisk, String> {
    Err("Disk file editing is not supported on Windows.".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn unmount_disk(_disk_name: &str, _store: &MountedDiskStore) -> Result<(), String> {
    Err("Disk file editing is not supported on Windows.".into())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn cleanup_stale_mounts() {
    // Nothing to do on Windows
}
