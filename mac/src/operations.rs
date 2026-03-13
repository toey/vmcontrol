use crate::api_helpers::{send_cmd_pctl, set_ma_mode, set_update_status};
use crate::config::{get_conf, get_conf_or};
use crate::db;
use crate::mds;
use crate::models::*;
use crate::ssh::{run_cmd, sanitize_name, spawn_background, validate_port};

/// Get total physical RAM of the host in MB
pub fn host_total_ram_mb() -> u64 {
    #[cfg(target_os = "macos")]
    {
        // macOS: sysctl hw.memsize returns bytes
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(bytes) = s.trim().parse::<u64>() {
                    return bytes / (1024 * 1024);
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        // Linux: read /proc/meminfo
        if let Ok(content) = std::fs::read_to_string("/proc/meminfo") {
            for line in content.lines() {
                if line.starts_with("MemTotal:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            return kb / 1024;
                        }
                    }
                }
            }
        }
    }
    // Fallback / Windows: return 0 (skip validation)
    0
}

/// Get total logical CPU cores of the host
pub fn host_total_cpus() -> u32 {
    #[cfg(target_os = "macos")]
    {
        if let Ok(output) = std::process::Command::new("sysctl")
            .args(["-n", "hw.logicalcpu"])
            .output()
        {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(n) = s.trim().parse::<u32>() {
                    return n;
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = std::process::Command::new("nproc").output() {
            if let Ok(s) = String::from_utf8(output.stdout) {
                if let Ok(n) = s.trim().parse::<u32>() {
                    return n;
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(val) = std::env::var("NUMBER_OF_PROCESSORS") {
            if let Ok(n) = val.parse::<u32>() {
                return n;
            }
        }
    }
    0
}

/// Get total RAM allocated by running VMs (in MB)
pub fn running_vms_ram_mb(exclude_smac: Option<&str>) -> u64 {
    let mut total: u64 = 0;
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if vm.status != "running" {
                continue;
            }
            if let Some(exc) = exclude_smac {
                if vm.smac == exc {
                    continue;
                }
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(size) = cfg
                    .get("memory")
                    .and_then(|m| m.get("size"))
                    .and_then(|v| v.as_str())
                {
                    total += size.parse::<u64>().unwrap_or(0);
                }
            }
        }
    }
    total
}

/// VNC port range constants
pub const VNC_PORT_MIN: u16 = 12001;
pub const VNC_PORT_MAX: u16 = 13000;
pub const VNC_PORT_STEP: u16 = 2;
pub const VNC_PORT_BASE: u16 = 12000;

/// Check if an ISO is mounted by any running VM — returns Err with VM name if so
pub fn check_iso_not_mounted(iso_name: &str) -> Result<(), String> {
    use crate::api_helpers::qemu_monitor_cmd;
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if vm.status != "running" {
                continue;
            }
            if let Ok(info) = qemu_monitor_cmd(&vm.smac, "info block") {
                if info.contains(iso_name) {
                    return Err(format!(
                        "ISO '{}' is currently mounted on running VM '{}' — unmount it first",
                        iso_name, vm.smac
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Validate VM name — only alphanumeric, underscore, dash allowed
pub fn validate_vm_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("VM name is required".into());
    }
    if !name.chars().next().unwrap().is_ascii_alphabetic() {
        return Err("VM name must start with an English letter (a-z, A-Z)".into());
    }
    if name.len() > 255 {
        return Err("VM name too long (max 255 chars)".into());
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
            return Err(format!("Invalid character '{}' in VM name — only English letters, numbers, underscore and dash allowed", c));
        }
    }
    Ok(())
}

/// Validate disk name: must start with English letter, only [a-zA-Z0-9_-]
fn validate_disk_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Disk name is required".into());
    }
    if !name.chars().next().unwrap().is_ascii_alphabetic() {
        return Err("Disk name must start with an English letter (a-z, A-Z)".into());
    }
    if name.len() > 255 {
        return Err("Disk name too long (max 255 chars)".into());
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
            return Err(format!(
                "Invalid character '{}' in disk name — only English letters, numbers, underscore and dash allowed",
                c
            ));
        }
    }
    Ok(())
}

/// Check if a disk is owned by a running VM — returns Err if so
pub fn check_disk_not_in_use(disk_name: &str) -> Result<(), String> {
    if let Ok(disks) = db::list_disks() {
        for d in &disks {
            if d.name == disk_name && !d.owner.is_empty() {
                if let Ok(vm) = db::get_vm(&d.owner) {
                    if vm.status == "running" {
                        return Err(format!(
                            "Disk '{}' is in use by running VM '{}' — stop the VM first",
                            disk_name, d.owner
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Generate a cloud-init NoCloud seed ISO from per-VM MDS config.
/// Returns (iso_path, vmctl_password).
fn generate_seed_iso(vm_name: &str) -> Result<(String, String), String> {
    let pctl_path = get_conf("pctl_path");
    let seed_dir = format!("{}/seed_{}", pctl_path, vm_name);
    let iso_path = format!("{}/seed_{}.iso", pctl_path, vm_name);

    // Create seed directory
    let _ = std::fs::create_dir_all(&seed_dir);

    // Load per-VM MDS config from DB, fall back to global
    let config = if let Ok(vm) = db::get_vm(vm_name) {
        let vm_config: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
        if let Some(mds_val) = vm_config.get("mds") {
            serde_json::from_value::<mds::MdsConfig>(mds_val.clone())
                .unwrap_or_else(|_| mds::load_mds_config())
        } else {
            mds::load_mds_config()
        }
    } else {
        mds::load_mds_config()
    };

    // Generate meta-data (NoCloud format with full MDS fields)
    // Avoid duplicated hostnames like "GW-GW" when prefix equals VM name
    let hostname = if config.hostname_prefix.is_empty()
        || config.hostname_prefix == vm_name
        || config.hostname_prefix == "nocloud"
    {
        vm_name.to_string()
    } else {
        format!("{}-{}", config.hostname_prefix, vm_name)
    };
    let mut meta_data = String::new();
    // Use unique instance-id per boot — cloud-init only re-applies hostname/config
    // when instance-id changes. Append timestamp so every VM start is a "new instance".
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let instance_id = if config.instance_id == "i-0000000000000001" || config.instance_id.is_empty() {
        format!("i-{}-{}", vm_name, ts)
    } else {
        format!("{}-{}", config.instance_id, ts)
    };
    eprintln!("[cloud-init] instance-id={} hostname={}", instance_id, hostname);
    meta_data.push_str(&format!("instance-id: {}\n", instance_id));
    meta_data.push_str(&format!("local-hostname: {}\n", hostname));
    meta_data.push_str(&format!("ami-id: {}\n", config.ami_id));
    meta_data.push_str(&format!("local-ipv4: {}\n", config.local_ipv4));
    if !config.ssh_pubkey.is_empty() {
        meta_data.push_str("public-keys:\n");
        meta_data.push_str(&format!("  - {}\n", config.ssh_pubkey));
    }

    // Generate random password for vmctl user
    let vmctl_password = generate_random_password(12);
    eprintln!("[cloud-init] vmctl user password generated (length={})", vmctl_password.len());

    // Generate user-data (cloud-config for NoCloud)
    let user_data = mds::generate_userdata_nocloud(&config, &hostname, &vmctl_password);

    // Write files
    std::fs::write(format!("{}/meta-data", seed_dir), &meta_data)
        .map_err(|e| format!("Failed to write meta-data: {}", e))?;
    std::fs::write(format!("{}/user-data", seed_dir), &user_data)
        .map_err(|e| format!("Failed to write user-data: {}", e))?;

    // Generate network-config (cloud-init NoCloud, netplan version 2)
    // Always generate so primary NIC uses MAC-based DHCP client-id
    // (prevents all VMs from getting the same IP on vmnet-shared)
    {
        let primary_mac = if let Ok(vm_rec) = db::get_vm(vm_name) {
            let vm_cfg: serde_json::Value =
                serde_json::from_str(&vm_rec.config).unwrap_or_default();
            vm_cfg
                .get("network_adapters")
                .and_then(|a| a.as_array())
                .and_then(|arr| arr.first())
                .and_then(|a| a.get("mac"))
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string()
        } else {
            String::new()
        };

        let mut net_cfg = String::from("version: 2\nethernets:\n");
        // Primary NIC — DHCP (use MAC as client-id so each VM gets a unique IP)
        if !primary_mac.is_empty() {
            net_cfg.push_str("  primary:\n");
            net_cfg.push_str("    match:\n");
            net_cfg.push_str(&format!("      macaddress: \"{}\"\n", primary_mac));
            net_cfg.push_str("    dhcp4: true\n");
            net_cfg.push_str("    dhcp-identifier: mac\n");
        }
        // Internal NIC — static IP on shared 192.168.100.0/24 subnet
        if !config.internal_ip.is_empty() {
            let internal_mac = derive_internal_mac(&config.internal_ip);
            net_cfg.push_str("  internal:\n");
            net_cfg.push_str("    match:\n");
            net_cfg.push_str(&format!("      macaddress: \"{}\"\n", internal_mac));
            net_cfg.push_str("    addresses:\n");
            net_cfg.push_str(&format!("      - {}/24\n", config.internal_ip));
        }

        std::fs::write(format!("{}/network-config", seed_dir), &net_cfg)
            .map_err(|e| format!("Failed to write network-config: {}", e))?;
    }

    // Create ISO — platform-specific tool
    let _ = std::fs::remove_file(&iso_path); // remove old ISO if exists
    #[cfg(target_os = "macos")]
    {
        run_cmd("hdiutil", &[
            "makehybrid", "-iso", "-joliet",
            "-default-volume-name", "CIDATA",
            "-o", &iso_path, &seed_dir,
        ]).map_err(|e| format!("Failed to create seed ISO: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        // Try genisoimage first, fall back to mkisofs
        let result = run_cmd("genisoimage", &[
            "-output", &iso_path, "-volid", "CIDATA",
            "-joliet", "-rock", &seed_dir,
        ]);
        if result.is_err() {
            run_cmd("mkisofs", &[
                "-output", &iso_path, "-volid", "CIDATA",
                "-joliet", "-rock", &seed_dir,
            ]).map_err(|e| format!("Failed to create seed ISO (install genisoimage or mkisofs): {}", e))?;
        }
    }
    #[cfg(target_os = "windows")]
    {
        return Err("Seed ISO generation not supported on Windows — disable cloud-init or create ISO manually".into());
    }

    // Cleanup seed directory
    let _ = std::fs::remove_dir_all(&seed_dir);

    // Verify ISO was actually created
    if !std::path::Path::new(&iso_path).exists() {
        // hdiutil sometimes appends .iso automatically
        let alt = format!("{}.iso", iso_path);
        if std::path::Path::new(&alt).exists() {
            let _ = std::fs::rename(&alt, &iso_path);
        } else {
            return Err(format!("Seed ISO not found at {} after creation", iso_path));
        }
    }

    Ok((iso_path, vmctl_password))
}

/// Create an ISO from files in a directory and auto-mount on a free CD drive
pub fn create_and_mount_sendfiles_iso(
    smac: &str,
    temp_dir: &str,
) -> Result<(String, String), String> {
    let iso_dir = get_conf("iso_path");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let safe_name = sanitize_name(smac).unwrap_or(smac);
    let iso_name = format!("sendfiles_{}_{}.iso", safe_name, timestamp);
    let iso_path = format!("{}/{}", iso_dir, iso_name);

    // Create ISO — same platform-specific approach as generate_seed_iso
    let _ = std::fs::remove_file(&iso_path);
    #[cfg(target_os = "macos")]
    {
        run_cmd(
            "hdiutil",
            &[
                "makehybrid",
                "-iso",
                "-joliet",
                "-default-volume-name",
                "SENDFILES",
                "-o",
                &iso_path,
                temp_dir,
            ],
        )
        .map_err(|e| format!("Failed to create sendfiles ISO: {}", e))?;
    }
    #[cfg(target_os = "linux")]
    {
        let result = run_cmd(
            "genisoimage",
            &[
                "-output",
                &iso_path,
                "-volid",
                "SENDFILES",
                "-joliet",
                "-rock",
                temp_dir,
            ],
        );
        if result.is_err() {
            run_cmd(
                "mkisofs",
                &[
                    "-output",
                    &iso_path,
                    "-volid",
                    "SENDFILES",
                    "-joliet",
                    "-rock",
                    temp_dir,
                ],
            )
            .map_err(|e| {
                format!(
                    "Failed to create sendfiles ISO (install genisoimage or mkisofs): {}",
                    e
                )
            })?;
        }
    }
    #[cfg(target_os = "windows")]
    {
        return Err("ISO creation not supported on Windows host".into());
    }

    // Handle hdiutil auto-appending .iso
    if !std::path::Path::new(&iso_path).exists() {
        let alt = format!("{}.iso", iso_path);
        if std::path::Path::new(&alt).exists() {
            let _ = std::fs::rename(&alt, &iso_path);
        } else {
            return Err(format!("Sendfiles ISO not found at {} after creation", iso_path));
        }
    }

    // Cleanup temp directory
    let _ = std::fs::remove_dir_all(temp_dir);

    // Cleanup any previously mounted sendfiles ISO before mounting new one
    let block_info =
        crate::api_helpers::qemu_monitor_cmd(smac, "info block").unwrap_or_default();
    let iso_dir_path = std::path::Path::new(&iso_dir);
    for i in 0..4 {
        let drive_id = format!("cd{}", i);
        for line in block_info.lines() {
            let trimmed = line.trim();
            if (trimmed.starts_with(&format!("{} ", drive_id))
                || trimmed.starts_with(&format!("{}:", drive_id)))
                && trimmed.contains("sendfiles_")
            {
                // Unmount old sendfiles ISO
                let unmount_arg = format!("{} {}", smac, drive_id);
                let _ = crate::api_helpers::send_cmd_pctl("unmountiso", &unmount_arg);
                // Delete old ISO file — extract filename safely
                if let Some(start) = trimmed.find("sendfiles_") {
                    let rest = &trimmed[start..];
                    let end = rest
                        .find(|c: char| c == ' ' || c == ')' || c == '"' || c == '\'' || c == ',')
                        .unwrap_or(rest.len());
                    let old_iso = &rest[..end];
                    // Validate extracted name: must end with .iso, no path separators
                    if old_iso.ends_with(".iso")
                        && !old_iso.contains('/')
                        && !old_iso.contains('\\')
                        && !old_iso.contains("..")
                    {
                        let _ = std::fs::remove_file(iso_dir_path.join(old_iso));
                    }
                }
                break;
            }
        }
    }

    // Re-read block info after cleanup
    let block_info =
        crate::api_helpers::qemu_monitor_cmd(smac, "info block").unwrap_or_default();

    // Find first free CD drive (cd0–cd3)
    let mut free_drive = None;
    for i in 0..4 {
        let drive_id = format!("cd{}", i);
        let mut is_used = false;
        for line in block_info.lines() {
            let trimmed = line.trim();
            if (trimmed.starts_with(&format!("{} ", drive_id))
                || trimmed.starts_with(&format!("{}:", drive_id)))
                && !trimmed.contains("[not inserted]")
                && !trimmed.contains("not inserted")
            {
                is_used = true;
                break;
            }
        }
        if !is_used {
            free_drive = Some(drive_id);
            break;
        }
    }

    let drive = free_drive.ok_or("No free CD drive (cd0–cd3) available")?;

    // Mount ISO on the free drive — send_cmd_pctl("mountiso", "vmname isoname drive")
    let mount_arg = format!("{} {} {}", smac, iso_name, drive);
    let result = crate::api_helpers::send_cmd_pctl("mountiso", &mount_arg);
    if result.contains("Error:") {
        return Err(format!("Failed to mount sendfiles ISO: {}", result));
    }

    Ok((drive, iso_name))
}

/// Start a QEMU VM from config stored in the database
fn start_vm_with_config(smac: &str, cfg: &VmStartConfig) -> Result<String, String> {
    let is_aarch64 = cfg.features.arch == "aarch64";
    let is_windows = cfg.features.is_windows == "1";
    let qemu_path = if is_aarch64 {
        get_conf("qemu_aarch64_path")
    } else {
        get_conf("qemu_path")
    };
    let pctl_path = get_conf("pctl_path");
    let disk_path = get_conf("disk_path");
    let qemu_accel = get_conf("qemu_accel");
    let qemu_machine = if is_aarch64 { "virt".to_string() } else { get_conf("qemu_machine") };

    // ensure directories exist
    if let Err(e) = std::fs::create_dir_all(&pctl_path) {
        log::warn!("Failed to create pctl_path '{}': {}", pctl_path, e);
    }
    if let Err(e) = std::fs::create_dir_all(&disk_path) {
        log::warn!("Failed to create disk_path '{}': {}", disk_path, e);
    }

    let mut output_log = String::new();

    // Use smac as the VM identifier
    let ismac = smac.to_string();

    // Build QEMU arguments safely (no shell involved)
    let mut qemu_args: Vec<String> = Vec::new();

    // Boot options — aarch64 virt machine uses virtio-gpu instead of -vga std
    // NOTE: no -daemonize — QEMU's daemonize + websocket VNC crashes on macOS.
    // We use spawn_background() instead to run QEMU in the background.
    if is_aarch64 {
        qemu_args.extend([
            "-nodefaults", "-boot", "d",
        ].map(String::from));

        // aarch64 Windows: use Secure Boot firmware (Debian AAVMF with Microsoft keys)
        // aarch64 Linux: use standard edk2 firmware
        let use_aarch64_secboot = is_windows && {
            let sc = get_conf_or("edk2_aarch64_secure_code",
                "/opt/homebrew/share/qemu/AAVMF_CODE.secboot.fd");
            std::path::Path::new(&sc).exists()
        };

        let bios;
        let nvram_file = format!("{}/{}_efivars.fd", pctl_path, ismac);

        if use_aarch64_secboot {
            // Secure Boot firmware from Debian qemu-efi-aarch64 package
            let secure_code = get_conf_or("edk2_aarch64_secure_code",
                "/opt/homebrew/share/qemu/AAVMF_CODE.secboot.fd");
            let secure_vars = get_conf_or("edk2_aarch64_secure_vars",
                "/opt/homebrew/share/qemu/AAVMF_VARS.ms.fd");
            bios = secure_code;
            // Copy NVRAM: prefer template NVRAM (preserves boot entries), fallback to generic
            if !std::path::Path::new(&nvram_file).exists() {
                if let Some(tpl_nvram) = find_template_nvram(cfg) {
                    log::info!("Using template NVRAM: {}", tpl_nvram);
                    let _ = std::fs::copy(&tpl_nvram, &nvram_file);
                } else if std::path::Path::new(&secure_vars).exists() {
                    let _ = std::fs::copy(&secure_vars, &nvram_file);
                } else {
                    // Create empty 64MB vars file as fallback
                    let _ = std::fs::write(&nvram_file, vec![0u8; 64 * 1024 * 1024]);
                }
            }
            output_log.push_str("Secure Boot: ENABLED (AAVMF Debian aarch64 firmware)\n");
        } else {
            // Standard non-secure firmware
            bios = get_conf("edk2_aarch64_bios");
            if !std::path::Path::new(&bios).exists() {
                return Err(format!("aarch64 requires UEFI firmware but '{}' not found. Install EDK2 or set edk2_aarch64_bios in config.", bios));
            }
            // Per-VM NVRAM file: prefer template NVRAM, fallback to generic
            if !std::path::Path::new(&nvram_file).exists() {
                if let Some(tpl_nvram) = find_template_nvram(cfg) {
                    log::info!("Using template NVRAM: {}", tpl_nvram);
                    let _ = std::fs::copy(&tpl_nvram, &nvram_file);
                } else {
                    let vars_template = get_conf("edk2_aarch64_bios")
                        .replace("aarch64-code.fd", "arm-vars.fd");
                    if std::path::Path::new(&vars_template).exists() {
                        let _ = std::fs::copy(&vars_template, &nvram_file);
                    } else {
                        let _ = std::fs::write(&nvram_file, vec![0u8; 64 * 1024 * 1024]);
                    }
                }
            }
            if is_windows {
                output_log.push_str(&format!(
                    "Secure Boot: DISABLED (aarch64 secure firmware not found)\n"));
                output_log.push_str("  Install AAVMF_CODE.secboot.fd + AAVMF_VARS.ms.fd for Secure Boot\n");
                output_log.push_str("  Use Win11 Bypass button to skip Secure Boot check\n");
            }
        }

        // pflash0 = firmware code (read-only), pflash1 = NVRAM (read-write)
        qemu_args.push("-drive".into());
        qemu_args.push(format!("if=pflash,format=raw,readonly=on,file={}", bios));
        qemu_args.push("-drive".into());
        qemu_args.push(format!("if=pflash,format=raw,file={}", nvram_file));
        // CPU for aarch64 — use "host" with HVF on Apple Silicon for best performance
        qemu_args.push("-cpu".into());
        qemu_args.push("host".into());
        // Display: ramfb for early UEFI boot + virtio-gpu for OS
        qemu_args.push("-device".into());
        qemu_args.push("ramfb".into());
        qemu_args.push("-device".into());
        qemu_args.push("virtio-gpu-pci".into());
        // USB controller + keyboard + tablet (virt machine has no PS/2)
        qemu_args.push("-device".into());
        qemu_args.push("qemu-xhci".into());
        qemu_args.push("-device".into());
        qemu_args.push("usb-kbd".into());
        qemu_args.push("-device".into());
        qemu_args.push("usb-tablet".into());
    } else {
        // CPU for x86_64 — configurable model (default: Haswell-v4)
        // "Haswell-v4" provides SSE4.2/AVX/AVX2 with proper CPUID brand string.
        // On native x86 with KVM, set qemu_cpu_x86=host in config.yaml for best performance.
        let cpu_model = get_conf_or("qemu_cpu_x86", "Haswell-v4");
        qemu_args.push("-cpu".into());
        qemu_args.push(cpu_model);
        qemu_args.extend([
            "-nodefaults", "-vga", "std", "-boot", "d",
        ].map(String::from));

        // x86_64 Windows: enable UEFI Secure Boot via pflash if firmware available
        if is_windows {
            let secure_code = get_conf_or("edk2_x86_secure_code",
                "/opt/homebrew/share/qemu/edk2-x86_64-secure-code.fd");
            let vars_template = get_conf_or("edk2_x86_vars",
                "/opt/homebrew/share/qemu/edk2-i386-vars.fd");

            if std::path::Path::new(&secure_code).exists() {
                // Per-VM NVRAM file: prefer template NVRAM, fallback to generic
                let nvram_file = format!("{}/{}_efivars.fd", pctl_path, ismac);
                if !std::path::Path::new(&nvram_file).exists() {
                    if let Some(tpl_nvram) = find_template_nvram(cfg) {
                        log::info!("Using template NVRAM: {}", tpl_nvram);
                        let _ = std::fs::copy(&tpl_nvram, &nvram_file);
                    } else if std::path::Path::new(&vars_template).exists() {
                        let _ = std::fs::copy(&vars_template, &nvram_file);
                    } else {
                        let _ = std::fs::write(&nvram_file, vec![0u8; 256 * 1024]);
                    }
                }
                // pflash0 = secure firmware code (read-only)
                // pflash1 = NVRAM with Secure Boot keys (read-write)
                qemu_args.push("-drive".into());
                qemu_args.push(format!(
                    "if=pflash,format=raw,readonly=on,file={}", secure_code));
                qemu_args.push("-drive".into());
                qemu_args.push(format!(
                    "if=pflash,format=raw,file={}", nvram_file));
                // SMM required for Secure Boot on x86_64
                qemu_args.push("-global".into());
                qemu_args.push("driver=cfi.pflash01,property=secure,value=on".into());
                output_log.push_str("Secure Boot: ENABLED (edk2 secure pflash)\n");
            } else {
                output_log.push_str(&format!(
                    "Secure Boot: DISABLED (firmware not found: {})\n", secure_code));
                output_log.push_str("  Use Win11 Bypass button to skip Secure Boot check\n");
            }
        }
    }

    // Windows localtime (use RTC base=localtime instead of deprecated -localtime)
    if is_windows {
        qemu_args.extend(["-rtc", "base=localtime"].map(String::from));
    }

    // TPM 2.0 emulation via swtpm (required for Windows 11)
    if is_windows {
        let swtpm_path = get_conf_or("swtpm_path", "swtpm");
        if std::process::Command::new(&swtpm_path)
            .arg("--version")
            .output()
            .is_ok()
        {
            let tpm_dir = format!("{}/{}_tpm", pctl_path, ismac);
            let _ = std::fs::create_dir_all(&tpm_dir);
            let tpm_sock = format!("{}/{}_tpm.sock", pctl_path, ismac);

            // Remove stale socket from previous run
            let _ = std::fs::remove_file(&tpm_sock);

            // Start swtpm daemon (forks and returns when ready)
            let _ = std::process::Command::new(&swtpm_path)
                .args([
                    "socket",
                    "--tpmstate",
                    &format!("dir={}", tpm_dir),
                    "--ctrl",
                    &format!("type=unixio,path={}", tpm_sock),
                    "--tpm2",
                    "-d",
                ])
                .output();

            // Add TPM device to QEMU
            qemu_args.push("-chardev".into());
            qemu_args.push(format!("socket,id=chrtpm,path={}", tpm_sock));
            qemu_args.push("-tpmdev".into());
            qemu_args.push("emulator,id=tpm0,chardev=chrtpm".into());
            qemu_args.push("-device".into());
            if is_aarch64 {
                qemu_args.push("tpm-tis-device,tpmdev=tpm0".into());
            } else {
                qemu_args.push("tpm-crb,tpmdev=tpm0".into());
            }
            output_log.push_str("TPM 2.0 : swtpm emulator\n");
        } else {
            output_log.push_str("TPM 2.0 : swtpm not found (install with: brew install swtpm)\n");
        }
    }

    // Display VNC with built-in WebSocket (no websockify needed)
    // Prevent VNC port collision: if another running VM already uses this port, auto-reassign
    let mut vnc_port = cfg.vnc_port;
    let running_ports = running_vnc_ports(smac);
    if running_ports.contains(&vnc_port) {
        let new_port = next_free_vnc_port(smac)?;
        output_log.push_str(&format!(
            "VNC port {} already in use by another running VM, reassigned to {}\n",
            vnc_port, new_port
        ));
        vnc_port = new_port;
        // Update the saved config with the new port
        if let Ok(vm) = db::get_vm(smac) {
            if let Ok(mut saved_cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                saved_cfg["vnc_port"] = serde_json::json!(vnc_port);
                let _ = db::update_vm(smac, &serde_json::to_string(&saved_cfg).unwrap_or_default());
            }
        }
    }
    if vnc_port < VNC_PORT_MIN || vnc_port > VNC_PORT_MAX {
        return Err(format!("VNC port {} out of valid range ({}-{})", vnc_port, VNC_PORT_MIN, VNC_PORT_MAX));
    }
    let vnc_display = vnc_port - VNC_PORT_BASE;
    qemu_args.push("-display".into());
    qemu_args.push(format!("vnc=127.0.0.1:{},websocket={}", vnc_display, vnc_port));

    // Memory — validate against host RAM
    let vm_ram: u64 = cfg.memory.size.parse().unwrap_or(0);
    let host_ram = host_total_ram_mb();
    if host_ram > 0 && vm_ram > 0 {
        let used_ram = running_vms_ram_mb(Some(smac));
        let available = host_ram.saturating_sub(used_ram);
        if vm_ram > host_ram {
            return Err(format!(
                "VM requires {} MB but host only has {} MB total RAM",
                vm_ram, host_ram
            ));
        }
        // Reserve 1024 MB (1 GB) for host OS
        let reserved: u64 = 1024;
        let usable = host_ram.saturating_sub(reserved);
        if used_ram + vm_ram > usable {
            return Err(format!(
                "Not enough RAM: VM needs {} MB, running VMs use {} MB, host has {} MB (reserved {} MB for OS, available {} MB)",
                vm_ram, used_ram, host_ram, reserved, usable.saturating_sub(used_ram)
            ));
        }
        output_log.push_str(&format!(
            "ram: {} MB (host {} MB, used {} MB, available {} MB)\n",
            vm_ram, host_ram, used_ram, available
        ));
    }
    qemu_args.push("-m".into());
    qemu_args.push(format!("{}M", cfg.memory.size));

    // Disks
    for disk in &cfg.disks {
        output_log.push_str(&format!("diskid : {}\n", disk.diskid));
        output_log.push_str(&format!("diskname : {}\n", disk.diskname));
        output_log.push_str(&format!("iops-total : {}\n", disk.iops_total));
        output_log.push_str(&format!("iops-total-max : {}\n", disk.iops_total_max));
        output_log.push_str(&format!(
            "iops-total-max-length : {}\n",
            disk.iops_total_max_length
        ));
        // Validate backing chain integrity before starting
        let disk_file = format!("{}/{}.qcow2", disk_path, disk.diskname);
        if let Ok(Some(backing)) = get_disk_backing_info(&disk.diskname) {
            let backing_path = format!("{}/{}.qcow2", disk_path, backing);
            if !std::path::Path::new(&backing_path).exists() {
                return Err(format!(
                    "Disk '{}' depends on backing file '{}' which is missing! Flatten the disk or restore the backing file.",
                    disk.diskname, backing
                ));
            }
        }
        // auto-create disk if not exists
        if !std::path::Path::new(&disk_file).exists() {
            let qemu_img = get_conf("qemu_img_path");
            output_log.push_str(&format!("auto-creating disk: {}\n", disk_file));
            if let Ok(out) = run_cmd(&qemu_img, &["create", "-f", "qcow2", &disk_file, "10G"]) {
                output_log.push_str(&out);
            }
        }
        let drive_id = format!("hd{}", disk.diskid);
        qemu_args.push("-drive".into());
        qemu_args.push(format!(
            "file={},format=qcow2,if=none,id={}",
            disk_file, drive_id
        ));
        qemu_args.push("-device".into());
        // bootindex=1+ so disk boots after CD-ROM (bootindex=0)
        let bootidx = disk.diskid.parse::<u32>().unwrap_or(0) + 1;
        // Use virtio-blk-pci for all VMs (including Windows):
        // - Best I/O performance
        // - Works on both x86_64 and aarch64 (nvme has issues on aarch64 virt)
        // - Windows gets viostor driver from virtio-win ISO (auto-mounted on cd3)
        //   → During install: Load Driver → Browse → cd3:\viostor\w10\amd64
        qemu_args.push(format!(
            "virtio-blk-pci,drive={},bootindex={}",
            drive_id, bootidx
        ));
    }

    // Network adapters (user-mode networking)
    // Load per-VM MDS config for SLIRP IP settings
    let mds_config = if let Ok(vm_rec) = db::get_vm(smac) {
        let vm_cfg: serde_json::Value = serde_json::from_str(&vm_rec.config).unwrap_or_default();
        if let Some(mds_val) = vm_cfg.get("mds") {
            output_log.push_str(&format!("mds_json: {}\n", mds_val));
            match serde_json::from_value::<mds::MdsConfig>(mds_val.clone()) {
                Ok(cfg) => {
                    output_log.push_str(&format!("mds_local_ipv4: {}\n", cfg.local_ipv4));
                    cfg
                }
                Err(e) => {
                    output_log.push_str(&format!("mds_parse_err: {} — using default\n", e));
                    mds::load_mds_config()
                }
            }
        } else {
            output_log.push_str("mds_json: NONE — using default\n");
            mds::load_mds_config()
        }
    } else {
        output_log.push_str("mds_json: VM_NOT_FOUND — using default\n");
        mds::load_mds_config()
    };
    output_log.push_str(&format!("slirp_ipv4: {}\n", mds_config.local_ipv4));
    let slirp_opts = if !mds_config.local_ipv4.is_empty() {
        let parts: Vec<&str> = mds_config.local_ipv4.split('.').collect();
        if parts.len() == 4 {
            let net = format!("{}.{}.{}.0/24", parts[0], parts[1], parts[2]);
            let host = format!("{}.{}.{}.1", parts[0], parts[1], parts[2]);
            let dns = format!("{}.{}.{}.2", parts[0], parts[1], parts[2]);
            let last: u8 = parts[3].parse().unwrap_or(10);
            let dhcp_start = if last <= 2 {
                format!("{}.{}.{}.10", parts[0], parts[1], parts[2])
            } else {
                mds_config.local_ipv4.clone()
            };
            format!(",net={},host={},dns={},dhcpstart={}",
                net, host, dns, dhcp_start)
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Build hostfwd options from port_forwards config
    let hostfwd_opts = {
        let vm_cfg: serde_json::Value = if let Ok(vm_rec) = db::get_vm(smac) {
            serde_json::from_str(&vm_rec.config).unwrap_or_default()
        } else {
            serde_json::Value::default()
        };
        let mut fwd = String::new();
        if let Some(forwards) = vm_cfg.get("port_forwards").and_then(|v| v.as_array()) {
            for rule in forwards {
                let proto = rule.get("protocol").and_then(|v| v.as_str()).unwrap_or("tcp");
                let host_port = rule.get("host_port").and_then(|v| v.as_u64()).unwrap_or(0);
                let guest_port = rule.get("guest_port").and_then(|v| v.as_u64()).unwrap_or(0);
                if host_port > 0 && guest_port > 0 {
                    fwd.push_str(&format!(",hostfwd={}::{}-:{}", proto, host_port, guest_port));
                    output_log.push_str(&format!("portfwd: {}:{} -> guest:{}\n",
                        proto, host_port, guest_port));
                }
            }
        }
        fwd
    };

    for adapter in &cfg.network_adapters {
        output_log.push_str(&format!("netid : {}\n", adapter.netid));
        output_log.push_str(&format!("mac : {}\n", adapter.mac));
        output_log.push_str(&format!("vlanid : {}\n", adapter.vlan));
        output_log.push_str(&format!("mode : {}\n", adapter.mode));

        qemu_args.push("-netdev".into());

        if adapter.mode == "switch" && !adapter.switch_name.is_empty() {
            // Virtual switch mode — QEMU socket multicast with VLAN
            // (macOS has no OVS kernel module, so we use multicast for L2 switching)
            let vlan_id: u16 = adapter.vlan.parse().unwrap_or(0);
            if vlan_id > 4094 {
                return Err(format!(
                    "VLAN {} out of range (0-4094) for adapter {}",
                    vlan_id, adapter.netid
                ));
            }
            let mcast_hi = vlan_id / 256;
            let mcast_lo = vlan_id % 256;
            match db::get_switch_by_name(&adapter.switch_name) {
                Ok(sw) => {
                    output_log.push_str(&format!("switch : {} (mcast port {})\n",
                        adapter.switch_name, sw.mcast_port));
                    output_log.push_str(&format!("vlan   : {} (mcast 230.{}.{}.1:{})\n",
                        vlan_id, mcast_hi, mcast_lo, sw.mcast_port));
                    qemu_args.push(format!(
                        "socket,id=net{},mcast=230.{}.{}.1:{},localaddr=127.0.0.1",
                        adapter.netid, mcast_hi, mcast_lo, sw.mcast_port
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "Switch '{}' not found for adapter {}: {}",
                        adapter.switch_name, adapter.netid, e
                    ));
                }
            }
        } else if adapter.mode == "bridge" {
            // Bridge/tap mode — host↔VM bidirectional connectivity (requires sudo/admin)
            // Validate bridge_iface to prevent command injection
            if !adapter.bridge_iface.is_empty() {
                for c in adapter.bridge_iface.chars() {
                    if !c.is_alphanumeric() && c != '-' && c != '_' && c != ' ' {
                        return Err(format!(
                            "Invalid bridge interface name '{}' for adapter {}",
                            adapter.bridge_iface, adapter.netid
                        ));
                    }
                }
            }

            #[cfg(target_os = "macos")]
            {
                if adapter.bridge_iface.is_empty() {
                    // vmnet-shared: macOS built-in DHCP (192.168.64.x), host↔VM bidirectional
                    qemu_args.push(format!("vmnet-shared,id=net{}", adapter.netid));
                    output_log.push_str("bridge : vmnet-shared (macOS)\n");
                } else {
                    // vmnet-bridged: bridge to physical interface (e.g. en0)
                    qemu_args.push(format!(
                        "vmnet-bridged,id=net{},ifname={}",
                        adapter.netid, adapter.bridge_iface
                    ));
                    output_log.push_str(&format!(
                        "bridge : vmnet-bridged ifname={} (macOS)\n",
                        adapter.bridge_iface
                    ));
                }
            }
            #[cfg(target_os = "linux")]
            {
                if adapter.bridge_iface.is_empty() {
                    let tap_name = format!("tap{}", adapter.netid);
                    qemu_args.push(format!(
                        "tap,id=net{},ifname={},script=no,downscript=no",
                        adapter.netid, tap_name
                    ));
                    output_log.push_str(&format!("bridge : tap ifname={} (Linux)\n", tap_name));
                } else {
                    qemu_args.push(format!(
                        "bridge,id=net{},br={}",
                        adapter.netid, adapter.bridge_iface
                    ));
                    output_log.push_str(&format!(
                        "bridge : bridge br={} (Linux)\n",
                        adapter.bridge_iface
                    ));
                }
            }
            #[cfg(target_os = "windows")]
            {
                // Windows: TAP-Windows adapter (requires OpenVPN TAP driver installed)
                let tap_name = if adapter.bridge_iface.is_empty() {
                    "TAP-Windows Adapter V9".to_string()
                } else {
                    adapter.bridge_iface.clone()
                };
                qemu_args.push(format!(
                    "tap,id=net{},ifname={}",
                    adapter.netid, tap_name
                ));
                output_log.push_str(&format!("bridge : tap ifname={} (Windows)\n", tap_name));
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
            {
                return Err(format!(
                    "Bridge networking not supported on this platform for adapter {}",
                    adapter.netid
                ));
            }
        } else {
            // Default: NAT (user-mode/SLIRP) networking with port forwarding
            qemu_args.push(format!("user,id=net{}{}{}", adapter.netid, slirp_opts, hostfwd_opts));
        }

        qemu_args.push("-device".into());
        // NIC model: virtio (fastest, needs driver) or e1000 (compatible, no driver needed)
        let nic_device = match adapter.nic_model.as_str() {
            "e1000" => "e1000",
            "e1000e" => "e1000e",
            "rtl8139" => "rtl8139",
            _ => "virtio-net-pci", // default: virtio
        };
        qemu_args.push(format!("{},netdev=net{},mac={}", nic_device, adapter.netid, adapter.mac));
    }

    // Internal network — VM-to-VM communication on 192.168.100.0/24
    if !mds_config.internal_ip.is_empty() {
        let internal_mac = derive_internal_mac(&mds_config.internal_ip);

        output_log.push_str(&format!("internal_ip : {}\n", mds_config.internal_ip));
        output_log.push_str(&format!("internal_mac: {}\n", internal_mac));

        qemu_args.push("-netdev".into());
        #[cfg(target_os = "macos")]
        {
            // macOS: use vmnet-host for reliable VM-to-VM communication
            // (multicast sockets don't work reliably on macOS)
            qemu_args.push("vmnet-host,id=netint,start-address=192.168.100.1,end-address=192.168.100.254,subnet-mask=255.255.255.0".into());
            output_log.push_str("internal_net: vmnet-host 192.168.100.0/24\n");
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Linux/Windows: use multicast socket (works reliably)
            let internal_mcast_port = get_conf_or("internal_mcast_port", "11111");
            output_log.push_str(&format!("internal_net: 230.0.100.1:{}\n", internal_mcast_port));
            qemu_args.push(format!(
                "socket,id=netint,mcast=230.0.100.1:{}",
                internal_mcast_port
            ));
        }
        qemu_args.push("-device".into());
        qemu_args.push(format!(
            "virtio-net-pci,netdev=netint,mac={}",
            internal_mac
        ));
    }

    // PCI Passthrough — VFIO devices (GPU, NIC, etc.)
    for (idx, pci) in cfg.pci_devices.iter().enumerate() {
        let addr = pci.host.trim();
        if !addr.is_empty() {
            output_log.push_str(&format!("pci_passthrough[{}]: {}\n", idx, addr));
            qemu_args.push("-device".into());
            qemu_args.push(format!("vfio-pci,host={}", addr));
        }
    }

    // Machine type — configurable accelerator (hvf:tcg for macOS, kvm:tcg for Linux)
    // Windows x86_64 with Secure Boot needs q35 + smm=on
    let use_secureboot = is_windows && !is_aarch64 && {
        let sc = get_conf_or("edk2_x86_secure_code",
            "/opt/homebrew/share/qemu/edk2-x86_64-secure-code.fd");
        std::path::Path::new(&sc).exists()
    };
    qemu_args.push("-machine".into());
    if is_aarch64 {
        // virt machine: highmem=on for PCI MMIO above 4GB (required by Windows ARM64)
        qemu_args.push(format!("type={},accel={},highmem=on", qemu_machine, qemu_accel));
    } else if use_secureboot {
        // q35 + smm=on required for UEFI Secure Boot
        qemu_args.push(format!("type=q35,accel={},smm=on", qemu_accel));
    } else {
        qemu_args.push(format!("type={},accel={}", qemu_machine, qemu_accel));
    }

    // SMP — if vcpus is set, auto-compute topology; otherwise use explicit values
    let vcpus: u32 = cfg.cpu.vcpus.parse().unwrap_or(0);
    let (total_cpus, sockets, cores, threads) = if vcpus > 0 {
        // Auto topology: 1 socket, vcpus cores, 1 thread
        (vcpus, 1u32, vcpus, 1u32)
    } else {
        let s: u32 = cfg.cpu.sockets.parse().unwrap_or(1);
        let c: u32 = cfg.cpu.cores.parse().unwrap_or(1);
        let t: u32 = cfg.cpu.threads.parse().unwrap_or(1);
        (s * c * t, s, c, t)
    };
    qemu_args.push("-smp".into());
    qemu_args.push(format!("{},sockets={},cores={},threads={}",
        total_cpus, sockets, cores, threads));

    // NOTE: virtio-scsi-pci removed — CD-ROM now uses usb-storage which needs no
    // extra drivers. This ensures Windows Setup can access virtio-win ISO on cd3.

    // Cloud-init seed ISO — attach as virtio-blk so ALL guests can see it
    // (virtio-scsi requires guest kernel module; virtio-blk always works since
    //  the main disk already uses it → shows up as /dev/vdb with CIDATA label)
    let cloudinit_enabled = cfg.features.cloudinit != "0";
    if cloudinit_enabled {
        match generate_seed_iso(&ismac) {
            Ok((seed_iso_path, vmctl_pw)) => {
                output_log.push_str(&format!("seed ISO : {}\n", seed_iso_path));
                qemu_args.push("-drive".into());
                qemu_args.push(format!(
                    "file={},if=none,id=seed0,format=raw,readonly=on", seed_iso_path
                ));
                qemu_args.push("-device".into());
                qemu_args.push("virtio-blk-pci,drive=seed0".into());
                // Save vmctl password to VM config for display on noVNC
                if let Ok(vm) = db::get_vm(smac) {
                    if let Ok(mut vm_cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                        vm_cfg["vmctl_password"] = serde_json::json!(vmctl_pw);
                        let _ = db::update_vm(smac, &serde_json::to_string(&vm_cfg).unwrap_or_default());
                    }
                }
            }
            Err(e) => {
                output_log.push_str(&format!("WARNING: seed ISO generation failed: {}\n", e));
            }
        };

        // SMBIOS cloud-init hints — tells ds-identify to use NoCloud
        // (aarch64 virt machine has no SMBIOS, skip for ARM VMs)
        if !is_aarch64 {
            qemu_args.push("-smbios".into());
            qemu_args.push("type=1,serial=ds=nocloud".into());
            qemu_args.push("-smbios".into());
            qemu_args.push("type=11,value=cloud-init:ds=nocloud".into());
        }
    } else {
        output_log.push_str("cloud-init: disabled\n");
    }

    // USB xHCI controller for x86_64 (aarch64 already has one from above)
    // Must be BEFORE usb-storage CD-ROM devices so the USB bus exists
    if !is_aarch64 {
        qemu_args.push("-device".into());
        qemu_args.push("qemu-xhci,id=xhci".into());
        qemu_args.push("-device".into());
        qemu_args.push("usb-tablet,bus=xhci.0".into());
    }

    // CDROM — 4 named drives "cd0"–"cd3" for runtime ISO mount/unmount via monitor
    // bootindex=0 on cd0 so UEFI/BIOS tries CD first, then falls through to disk
    // All platforms use usb-storage (no extra drivers needed)
    //
    // For Windows VMs: auto-mount virtio-win ISO on cd3 if available
    // (provides virtio drivers for network, balloon, etc. during installation)
    let virtio_iso = if is_windows {
        let iso_dir = get_conf("iso_path");
        // Look for virtio-win*.iso in the ISO directory
        let mut found: Option<String> = None;
        if let Ok(entries) = std::fs::read_dir(&iso_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("virtio-win") && name.ends_with(".iso") {
                    found = Some(entry.path().to_string_lossy().to_string());
                    break;
                }
            }
        }
        if let Some(ref p) = found {
            output_log.push_str(&format!("virtio-win ISO: {} (auto-mount on cd3)\n", p));
        }
        found
    } else {
        None
    };

    for i in 0..4u8 {
        let drive_id = format!("cd{}", i);
        qemu_args.push("-drive".into());
        // Pre-load virtio-win ISO on cd3 for Windows VMs
        if i == 3 {
            if let Some(ref viso) = virtio_iso {
                qemu_args.push(format!("if=none,id={},media=cdrom,file={},readonly=on", drive_id, viso));
            } else {
                qemu_args.push(format!("if=none,id={},media=cdrom", drive_id));
            }
        } else {
            qemu_args.push(format!("if=none,id={},media=cdrom", drive_id));
        }
        qemu_args.push("-device".into());
        // Use usb-storage for all platforms — universally recognized by all OS
        // without extra drivers (scsi-cd on virtio-scsi requires vioscsi driver
        // which Windows Setup doesn't have, making virtio-win ISO inaccessible)
        if i == 0 {
            qemu_args.push(format!("usb-storage,drive={},removable=true,bootindex=0", drive_id));
        } else {
            qemu_args.push(format!("usb-storage,drive={},removable=true", drive_id));
        }
    }

    // (USB xHCI + tablet already added before CD-ROM loop above)

    // Monitor socket
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("unix:{}/{},server,nowait", pctl_path, ismac));

    // Guest Agent socket — for direct file transfer via QEMU Guest Agent (qemu-ga)
    let qga_sock = format!("{}/{}_qga", pctl_path, ismac);
    let _ = std::fs::remove_file(&qga_sock); // Remove stale socket
    qemu_args.push("-chardev".into());
    qemu_args.push(format!(
        "socket,id=qga0,path={},server=on,wait=off",
        qga_sock
    ));
    qemu_args.push("-device".into());
    qemu_args.push("virtio-serial-pci".into());
    qemu_args.push("-device".into());
    qemu_args.push("virtserialport,chardev=qga0,name=org.qemu.guest_agent.0".into());
    output_log.push_str(&format!("guest-agent: socket {}\n", qga_sock));

    // Start VM as a background process (no -daemonize, which breaks WebSocket VNC)
    // Bridge/vmnet modes require sudo on macOS
    let needs_bridge = cfg.network_adapters.iter().any(|a| a.mode == "bridge");
    let needs_vmnet_internal = cfg!(target_os = "macos") && !mds_config.internal_ip.is_empty();
    let needs_sudo = needs_bridge || needs_vmnet_internal;
    let use_sudo = needs_sudo && get_conf_or("bridge_sudo", "true") == "true";

    let (pid, log_path) = if use_sudo {
        let sudo_path = get_conf_or("bridge_sudo_path", "/usr/bin/sudo");
        output_log.push_str("SUDO: bridge mode requires elevated privileges\n");
        let mut sudo_args: Vec<String> = vec![qemu_path.clone()];
        sudo_args.extend(qemu_args.iter().cloned());
        output_log.push_str(&format!("QEMU: {} {} {}\n", sudo_path, qemu_path, qemu_args.join(" ")));
        let sudo_args_ref: Vec<&str> = sudo_args.iter().map(|s| s.as_str()).collect();
        spawn_background(&sudo_path, &sudo_args_ref)
            .map_err(|e| format!("QEMU start error (sudo): {}", e))?
    } else {
        output_log.push_str(&format!("QEMU: {} {}\n", qemu_path, qemu_args.join(" ")));
        let args_ref: Vec<&str> = qemu_args.iter().map(|s| s.as_str()).collect();
        spawn_background(&qemu_path, &args_ref)
            .map_err(|e| format!("QEMU start error: {}", e))?
    };
    output_log.push_str(&format!("QEMU started (PID {})\n", pid));
    output_log.push_str(&format!("QEMU log: {}\n", log_path));

    // Set status to running
    if let Err(e) = db::set_vm_status(smac, "running") {
        output_log.push_str(&format!("WARNING: DB status update failed: {}\n", e));
    }

    Ok(output_log)
}

/// Start VM by smac — loads config from DB
pub fn start(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    // Validate VM name
    sanitize_name(&cmd.smac)?;

    // Load VM config from database
    let vm = db::get_vm(&cmd.smac)?;
    let cfg: VmStartConfig =
        serde_json::from_str(&vm.config).map_err(|e| format!("Config parse error: {}", e))?;

    start_vm_with_config(&cmd.smac, &cfg)
}

pub fn stop(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let mut output = format!("now stopping compute {}\n", cmd.smac);
    let pctl_output = send_cmd_pctl("stop", &cmd.smac);
    output.push_str(&pctl_output);
    // Only mark stopped if the QEMU monitor command succeeded (no error reported)
    if !pctl_output.contains("Error:") {
        if let Err(e) = db::set_vm_status(&cmd.smac, "stopped") {
            output.push_str(&format!("WARNING: DB status update failed: {}\n", e));
        }
    } else {
        output.push_str("WARNING: stop command may have failed — status not updated\n");
    }
    Ok(output)
}

pub fn reset(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let output = send_cmd_pctl("reset", &cmd.smac);
    Ok(output)
}

pub fn powerdown(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let mut output = send_cmd_pctl("powerdown", &cmd.smac);
    // ACPI powerdown is async — wait briefly then check if QEMU process exited
    std::thread::sleep(std::time::Duration::from_secs(3));
    let pctl_path = get_conf("pctl_path");
    let sock_path = format!("{}/{}", pctl_path, cmd.smac);
    if !std::path::Path::new(&sock_path).exists() {
        // Monitor socket gone = QEMU exited
        let _ = db::set_vm_status(&cmd.smac, "stopped");
        output.push_str("VM stopped.\n");
    } else {
        // QEMU still running — guest is shutting down
        output.push_str("ACPI powerdown sent. Guest OS is shutting down (status still 'running').\n");
    }
    Ok(output)
}

pub fn delete_vm(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;

    // Must stop VM before deleting
    if let Ok(vm) = db::get_vm(&cmd.smac) {
        if vm.status == "running" {
            return Err(format!("VM '{}' is running — stop the VM first", cmd.smac));
        }
    }

    let mut output = String::new();
    set_ma_mode("1", &cmd.smac);
    // Clear disk owners for this VM (disks remain, just unassigned)
    let _ = db::clear_disk_owner_by_vm(&cmd.smac);
    // Remove VM from database
    if let Err(e) = db::delete_vm(&cmd.smac) {
        output.push_str(&format!("WARNING: DB delete failed: {}\n", e));
    }
    set_update_status("2", &cmd.smac);
    set_ma_mode("0", &cmd.smac);
    output.push_str(&format!("VM '{}' deleted\n", cmd.smac));
    Ok(output)
}

/// List images for a VM — uses safe directory listing instead of shell
pub fn listimage(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let smac = sanitize_name(&cmd.smac)?;
    let disk_path = get_conf("disk_path");
    let mut output = String::new();

    let entries = std::fs::read_dir(&disk_path)
        .map_err(|e| format!("Failed to read disk directory: {}", e))?;

    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.starts_with(smac) {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            let size_human = if size >= 1024 * 1024 * 1024 {
                format!("{:.1}G", size as f64 / (1024.0 * 1024.0 * 1024.0))
            } else if size >= 1024 * 1024 {
                format!("{:.1}M", size as f64 / (1024.0 * 1024.0))
            } else {
                format!("{}K", size / 1024)
            };
            output.push_str(&format!("{:<40} {}\n", fname, size_human));
        }
    }

    if output.is_empty() {
        output.push_str("No images found\n");
    }

    Ok(output)
}

/// Create VM config — save to DB + assign disk owners
/// Collect all VNC ports currently used by VMs in DB
fn used_vnc_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(p) = cfg.get("vnc_port").and_then(|v| v.as_u64()) {
                    ports.push(p as u16);
                }
            }
        }
    }
    ports
}

/// Collect VNC ports used by *running* VMs only (for collision detection at start time)
fn running_vnc_ports(exclude_smac: &str) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if vm.status != "running" || vm.smac == exclude_smac {
                continue;
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(p) = cfg.get("vnc_port").and_then(|v| v.as_u64()) {
                    ports.push(p as u16);
                }
            }
        }
    }
    ports
}

/// Find next available VNC port that is not used by any running VM
fn next_free_vnc_port(exclude_smac: &str) -> Result<u16, String> {
    let running = running_vnc_ports(exclude_smac);
    let all_used = used_vnc_ports();
    let mut port = VNC_PORT_MIN;
    while (running.contains(&port) || all_used.contains(&port)) && port < VNC_PORT_MAX {
        port += VNC_PORT_STEP;
    }
    if port >= VNC_PORT_MAX {
        return Err(format!("No free VNC port available in range {}-{}", VNC_PORT_MIN, VNC_PORT_MAX));
    }
    Ok(port)
}

/// Collect all Local IPv4 addresses used by VMs in DB
fn used_ipv4s() -> Vec<String> {
    let mut ips = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(ip) = cfg.get("mds").and_then(|m| m.get("local_ipv4")).and_then(|v| v.as_str()) {
                    ips.push(ip.to_string());
                }
            }
        }
    }
    ips
}

/// Check that an IP is not already used by another VM.
/// Pass exclude_smac to skip a specific VM (e.g. when updating its own IP).
pub fn validate_ip_unique(ip: &str, exclude_smac: Option<&str>) -> Result<(), String> {
    if ip.is_empty() {
        return Ok(());
    }
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Some(exc) = exclude_smac {
                if vm.smac == exc {
                    continue;
                }
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(existing_ip) = cfg
                    .get("mds")
                    .and_then(|m| m.get("local_ipv4"))
                    .and_then(|v| v.as_str())
                {
                    if existing_ip == ip {
                        return Err(format!(
                            "IP '{}' is already assigned to VM '{}'",
                            ip, vm.smac
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Repair VMs that are missing mds.local_ipv4 or internal_ip
/// (caused by old update_config bug that wiped mds section)
pub fn repair_missing_mds_ips() {
    println!("repair: checking all VMs for missing mds...");
    let vms = match db::list_vms() {
        Ok(v) => v,
        Err(e) => { println!("repair: failed to list VMs: {}", e); return; }
    };
    for vm in &vms {
        let mut cfg: serde_json::Value = match serde_json::from_str(&vm.config) {
            Ok(v) => v,
            Err(e) => { println!("repair: {} — bad config: {}", vm.smac, e); continue; }
        };

        // Ensure mds is an object
        let has_mds = cfg.get("mds").is_some() && cfg.get("mds").unwrap().is_object();
        if !has_mds {
            let obj = cfg.as_object_mut().unwrap();
            obj.insert("mds".to_string(), serde_json::json!({}));
            println!("repair: {} — created mds object", vm.smac);
        }

        let mut changed = false;

        // Check local_ipv4
        let cur_ip = cfg.get("mds").and_then(|m| m.get("local_ipv4")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        if cur_ip.is_empty() || cur_ip == "10.0.0.1" {
            let ip = next_ipv4();
            cfg.get_mut("mds").unwrap().as_object_mut().unwrap()
                .insert("local_ipv4".to_string(), serde_json::json!(ip));
            println!("repair: {} → local_ipv4={}", vm.smac, ip);
            changed = true;
        }

        // Check internal_ip
        let cur_internal = cfg.get("mds").and_then(|m| m.get("internal_ip")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        if cur_internal.is_empty() {
            let ip = next_internal_ip();
            cfg.get_mut("mds").unwrap().as_object_mut().unwrap()
                .insert("internal_ip".to_string(), serde_json::json!(ip));
            println!("repair: {} → internal_ip={}", vm.smac, ip);
            changed = true;
        }

        if changed {
            let config_str = serde_json::to_string(&cfg).unwrap_or_default();
            match db::update_vm(&vm.smac, &config_str) {
                Ok(_) => println!("repair: {} — saved OK", vm.smac),
                Err(e) => println!("repair: {} — save FAILED: {}", vm.smac, e),
            }
        } else {
            println!("repair: {} — OK (ipv4={}, internal={})", vm.smac, cur_ip, cur_internal);
        }
    }
    println!("repair: done");
}

/// Find next available Local IPv4: 10.0.{subnet}.10
/// Supports up to 10.0.254.10 (254 subnets) then wraps to 10.1.x.10, etc.
pub fn next_ipv4() -> String {
    let used = used_ipv4s();
    for major in 0u8..=10 {
        let start_minor = if major == 0 { 1u8 } else { 0u8 };
        for minor in start_minor..=254 {
            let ip = format!("10.{}.{}.10", major, minor);
            if !used.contains(&ip) {
                return ip;
            }
        }
    }
    // Fallback (should never reach with 2800+ available)
    "10.0.1.10".to_string()
}

// ──────────────────────────────────────────
// Internal network IP pool (VM-to-VM in NAT)
// ──────────────────────────────────────────

/// Derive a unique MAC address for the internal NIC from the internal IP.
/// e.g. 192.168.100.10 → 52:54:01:00:00:0a
pub fn derive_internal_mac(internal_ip: &str) -> String {
    let last_octet: u8 = internal_ip
        .rsplit('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    format!("52:54:01:00:00:{:02x}", last_octet)
}

/// Collect all internal IPs used by VMs in DB
fn used_internal_ips() -> Vec<String> {
    let mut ips = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(ip) = cfg
                    .get("mds")
                    .and_then(|m| m.get("internal_ip"))
                    .and_then(|v| v.as_str())
                {
                    if !ip.is_empty() {
                        ips.push(ip.to_string());
                    }
                }
            }
        }
    }
    ips
}

/// Find next available internal IP: 192.168.100.{10..254}
pub fn next_internal_ip() -> String {
    let used = used_internal_ips();
    for host in 10u8..=254 {
        let ip = format!("192.168.100.{}", host);
        if !used.contains(&ip) {
            return ip;
        }
    }
    // Fallback (should never reach with 245 available)
    "192.168.100.10".to_string()
}

/// Check that an internal IP is not already used by another VM.
pub fn validate_internal_ip_unique(ip: &str, exclude_smac: Option<&str>) -> Result<(), String> {
    if ip.is_empty() {
        return Ok(());
    }
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Some(exc) = exclude_smac {
                if vm.smac == exc {
                    continue;
                }
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(existing_ip) = cfg
                    .get("mds")
                    .and_then(|m| m.get("internal_ip"))
                    .and_then(|v| v.as_str())
                {
                    if !existing_ip.is_empty() && existing_ip == ip {
                        return Err(format!(
                            "Internal IP '{}' is already assigned to VM '{}'",
                            ip, vm.smac
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Collect all MAC addresses used by all VMs in DB.
/// Returns Vec<(mac, vm_name)> for error reporting.
fn used_macs(exclude_smac: Option<&str>) -> Vec<(String, String)> {
    let mut macs = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Some(exc) = exclude_smac {
                if vm.smac == exc { continue; }
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(adapters) = cfg.get("network_adapters").and_then(|a| a.as_array()) {
                    for adapter in adapters {
                        if let Some(mac) = adapter.get("mac").and_then(|m| m.as_str()) {
                            if !mac.is_empty() {
                                macs.push((mac.to_lowercase(), vm.smac.clone()));
                            }
                        }
                    }
                }
            }
        }
    }
    macs
}

/// Generate a random MAC address with 52:54:00 prefix (QEMU convention).
/// Uses /dev/urandom for cryptographic randomness.
pub fn generate_random_mac() -> String {
    let mut octets = [0u8; 3];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut octets);
    } else {
        // Fallback: use multiple entropy sources mixed together
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let pid = std::process::id();
        let addr = &octets as *const _ as u64;
        let mix = nanos as u64 ^ (pid as u64).wrapping_shl(16) ^ addr;
        octets[0] = (mix >> 0) as u8;
        octets[1] = (mix >> 8) as u8;
        octets[2] = (mix >> 16) as u8;
    }
    format!("52:54:00:{:02x}:{:02x}:{:02x}", octets[0], octets[1], octets[2])
}

/// Generate a random alphanumeric password of given length.
/// Uses /dev/urandom for cryptographic randomness.
pub fn generate_random_password(len: usize) -> String {
    const CHARSET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut password = Vec::with_capacity(len);
    let bytes = read_urandom_bytes(len);
    for b in &bytes {
        password.push(CHARSET[(*b as usize) % CHARSET.len()]);
    }
    String::from_utf8(password).unwrap_or_else(|_| "Vm3trl5ecure".into())
}

/// Read `len` bytes from /dev/urandom with a robust fallback.
/// This is the single source of randomness for the application.
pub fn read_urandom_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            return bytes;
        }
    }
    // Fallback: mix multiple entropy sources (time, pid, stack address, thread id)
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = std::process::id() as u128;
    let addr = &bytes as *const _ as u128;
    let mut state = ts ^ (pid << 32) ^ (addr << 64);
    for b in bytes.iter_mut() {
        // xorshift128-like mixing
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *b = (state & 0xFF) as u8;
    }
    bytes
}

/// Validate that MAC addresses in a config are unique across all VMs.
/// exclude_smac: if updating a VM, exclude its own MACs from the check.
pub fn validate_mac_uniqueness(config: &serde_json::Value, exclude_smac: Option<&str>) -> Result<(), String> {
    let new_macs: Vec<String> = config
        .get("network_adapters")
        .and_then(|a| a.as_array())
        .map(|adapters| {
            adapters.iter()
                .filter_map(|a| a.get("mac").and_then(|m| m.as_str()))
                .filter(|m| !m.is_empty())
                .map(|m| m.to_lowercase())
                .collect()
        })
        .unwrap_or_default();

    if new_macs.is_empty() { return Ok(()); }

    // Check for duplicates within the new config itself
    let mut seen = std::collections::HashSet::new();
    for mac in &new_macs {
        if !seen.insert(mac.clone()) {
            return Err(format!("Duplicate MAC address '{}' within this VM config", mac));
        }
    }

    // Check against existing VMs
    let existing = used_macs(exclude_smac);
    for mac in &new_macs {
        for (existing_mac, vm_name) in &existing {
            if mac == existing_mac {
                return Err(format!(
                    "MAC address '{}' is already used by VM '{}'", mac, vm_name
                ));
            }
        }
    }

    Ok(())
}

/// Find next available VNC port starting from VNC_PORT_MIN, step by VNC_PORT_STEP
pub fn next_vnc_port() -> Result<u16, String> {
    let used = used_vnc_ports();
    let mut port = VNC_PORT_MIN;
    while used.contains(&port) && port < VNC_PORT_MAX {
        port += VNC_PORT_STEP;
    }
    if port >= VNC_PORT_MAX {
        return Err(format!("No free VNC port available in range {}-{}", VNC_PORT_MIN, VNC_PORT_MAX));
    }
    Ok(port)
}

pub fn create_config(json_str: &str) -> Result<String, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let smac = val.get("smac").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if smac.is_empty() {
        return Err("VM-NAME is required".into());
    }
    validate_vm_name(&smac)?;

    // Check VM name uniqueness
    if db::get_vm(&smac).is_ok() {
        return Err(format!("VM name '{}' already exists", smac));
    }

    // Extract the VM config + auto-assign VNC port
    let empty_obj = serde_json::Value::Object(serde_json::Map::new());
    let mut config = val.get("config").unwrap_or(&empty_obj).clone();
    if config.get("vnc_port").is_none() {
        let port = next_vnc_port()?;
        config["vnc_port"] = serde_json::json!(port);
    }
    // Auto-assign unique Local IPv4 if MDS not set or default
    {
        let needs_ip = match config.get("mds").and_then(|m| m.get("local_ipv4")).and_then(|v| v.as_str()) {
            Some(ip) if !ip.is_empty() && ip != "10.0.0.1" => false,
            _ => true,
        };
        if needs_ip {
            let ip = next_ipv4();
            // Ensure mds object exists
            if config.get("mds").is_none() || !config.get("mds").unwrap().is_object() {
                config.as_object_mut().unwrap()
                    .insert("mds".to_string(), serde_json::json!({}));
            }
            config.get_mut("mds").unwrap().as_object_mut().unwrap()
                .insert("local_ipv4".to_string(), serde_json::json!(ip));
        }
    }
    // Validate IP uniqueness
    if let Some(ip) = config.get("mds").and_then(|m| m.get("local_ipv4")).and_then(|v| v.as_str()) {
        validate_ip_unique(ip, None)?;
    }
    // Auto-assign internal_ip for VM-to-VM communication
    {
        let needs_internal = match config
            .get("mds")
            .and_then(|m| m.get("internal_ip"))
            .and_then(|v| v.as_str())
        {
            Some(ip) if !ip.is_empty() => false,
            _ => true,
        };
        if needs_internal {
            let ip = next_internal_ip();
            // Ensure mds object exists
            if config.get("mds").is_none() || !config.get("mds").unwrap().is_object() {
                config.as_object_mut().unwrap()
                    .insert("mds".to_string(), serde_json::json!({}));
            }
            config.get_mut("mds").unwrap().as_object_mut().unwrap()
                .insert("internal_ip".to_string(), serde_json::json!(ip));
        }
    }
    // Validate internal IP uniqueness
    if let Some(ip) = config
        .get("mds")
        .and_then(|m| m.get("internal_ip"))
        .and_then(|v| v.as_str())
    {
        if !ip.is_empty() {
            validate_internal_ip_unique(ip, None)?;
        }
    }
    // Validate MAC address uniqueness before saving
    validate_mac_uniqueness(&config, None)?;
    // Validate disk names
    if let Some(disks) = config.get("disks").and_then(|d| d.as_array()) {
        for disk in disks {
            if let Some(dname) = disk.get("diskname").and_then(|v| v.as_str()) {
                if !dname.is_empty() {
                    validate_disk_name(dname)?;
                }
            }
        }
    }

    let config_str = serde_json::to_string(&config).unwrap_or_default();

    let mut output = String::new();

    // Save to database
    db::insert_vm(&smac, "", "", &config_str)?;

    // Set disk owners
    if let Some(disks) = config.get("disks").and_then(|d| d.as_array()) {
        for disk in disks {
            if let Some(dname) = disk.get("diskname").and_then(|v| v.as_str()) {
                if !dname.is_empty() {
                    let _ = db::set_disk_owner(dname, &smac);
                    output.push_str(&format!("Disk '{}' assigned to VM '{}'\n", dname, smac));
                }
            }
        }
    }

    output.push_str(&format!("VM '{}' created successfully\n", smac));
    Ok(output)
}

/// Update VM config in DB + reassign disk owners
pub fn update_config(json_str: &str) -> Result<String, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let smac = val.get("smac").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if smac.is_empty() {
        return Err("VM-NAME is required".into());
    }
    validate_vm_name(&smac)?;

    let empty_obj = serde_json::Value::Object(serde_json::Map::new());
    let new_config = val.get("config").unwrap_or(&empty_obj);

    // Validate MAC address uniqueness (exclude this VM's own MACs)
    validate_mac_uniqueness(new_config, Some(&smac))?;
    // Validate disk names
    if let Some(disks) = new_config.get("disks").and_then(|d| d.as_array()) {
        for disk in disks {
            if let Some(dname) = disk.get("diskname").and_then(|v| v.as_str()) {
                if !dname.is_empty() {
                    validate_disk_name(dname)?;
                }
            }
        }
    }

    // Merge: start with old config, overlay new fields (preserves mds, vnc_port, port_forwards)
    let mut config = if let Ok(old_vm) = db::get_vm(&smac) {
        serde_json::from_str::<serde_json::Value>(&old_vm.config).unwrap_or_default()
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    if let (Some(old_map), Some(new_map)) = (config.as_object_mut(), new_config.as_object()) {
        for (k, v) in new_map {
            old_map.insert(k.clone(), v.clone());
        }
    }

    let config_str = serde_json::to_string(&config).unwrap_or_default();

    db::update_vm(&smac, &config_str)?;

    // Clear old disk owners for this VM, then set new ones
    let _ = db::clear_disk_owner_by_vm(&smac);
    if let Some(disks) = config.get("disks").and_then(|d| d.as_array()) {
        for disk in disks {
            if let Some(dname) = disk.get("diskname").and_then(|v| v.as_str()) {
                if !dname.is_empty() {
                    let _ = db::set_disk_owner(dname, &smac);
                }
            }
        }
    }

    Ok(format!("VM '{}' config updated\n", smac))
}

pub fn rename_vm(old_name: &str, new_name: &str) -> Result<String, String> {
    if old_name.is_empty() || new_name.is_empty() {
        return Err("Both old and new VM names are required".into());
    }
    if old_name == new_name {
        return Ok("No rename needed".into());
    }
    validate_vm_name(old_name)?;
    validate_vm_name(new_name)?;

    // Check old VM exists
    let vm = db::get_vm(old_name).map_err(|_| format!("VM '{}' not found", old_name))?;

    // Must be stopped
    if vm.status == "running" {
        return Err("Cannot rename a running VM. Stop it first.".into());
    }

    // Check new name is not taken
    if db::get_vm(new_name).is_ok() {
        return Err(format!("VM name '{}' already exists", new_name));
    }

    // Rename the QEMU monitor socket file if it exists
    let pctl_path = get_conf("pctl_path");
    let old_sock = format!("{}/{}", pctl_path, old_name);
    let new_sock = format!("{}/{}", pctl_path, new_name);
    if std::path::Path::new(&old_sock).exists() {
        let _ = std::fs::rename(&old_sock, &new_sock);
    }

    // Rename in database (VM + disk owners)
    db::rename_vm(old_name, new_name)?;

    Ok(format!("VM renamed from '{}' to '{}'", old_name, new_name))
}

pub fn mountiso(json_str: &str) -> Result<String, String> {
    let cmd: MountIsoCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    sanitize_name(&cmd.isoname)?;
    let output = send_cmd_pctl("mountiso", &format!("{} {} {}", cmd.smac, cmd.isoname, cmd.drive));
    Ok(output)
}

pub fn unmountiso(json_str: &str) -> Result<String, String> {
    let cmd: UnmountIsoCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let output = send_cmd_pctl("unmountiso", &format!("{} {}", cmd.smac, cmd.drive));
    Ok(output)
}

pub fn livemigrate(json_str: &str) -> Result<String, String> {
    let cmd: LiveMigrateCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let output = send_cmd_pctl(
        "livemigrate",
        &format!("{} {}", cmd.smac, cmd.to_node_ip),
    );
    Ok(output)
}

pub fn backup(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    // VM must be running for memory dump
    if let Ok(vm) = db::get_vm(&cmd.smac) {
        if vm.status != "running" {
            return Err("VM must be running to create a memory dump".into());
        }
    }
    let output = send_cmd_pctl("backup", &cmd.smac);
    Ok(output)
}

// ======== Full Backup operations ========

/// Get disk names from a VM config JSON
fn get_vm_disk_names(vm_name: &str) -> Result<Vec<String>, String> {
    let vm = db::get_vm(vm_name)?;
    let config: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
    let disks: Vec<String> = config
        .get("disks")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| d.get("diskname").and_then(|v| v.as_str()))
                .filter(|n| !n.is_empty())
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default();
    if disks.is_empty() {
        return Err(format!("VM '{}' has no disks configured", vm_name));
    }
    Ok(disks)
}

/// Create a full backup of a VM's disks
pub fn create_full_backup(vm_name: &str, note: &str) -> Result<String, String> {
    sanitize_name(vm_name)?;
    // VM must be stopped
    let vm = db::get_vm(vm_name)?;
    if vm.status == "running" {
        return Err("VM must be stopped before creating a full backup".into());
    }
    let disk_names = get_vm_disk_names(vm_name)?;
    let disk_path = get_conf("disk_path");
    let live_path = get_conf("live_path");
    let qemu_img = get_conf("qemu_img_path");

    // Generate backup_id
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let rnd = generate_random_password(6);
    let backup_id = format!("bk_{}_{}", ts, rnd);
    let backup_dir = format!("{}/full_backups/{}", live_path, backup_id);
    std::fs::create_dir_all(&backup_dir)
        .map_err(|e| format!("Failed to create backup dir: {}", e))?;

    let mut total_size: i64 = 0;
    let mut backed_up: Vec<String> = Vec::new();

    for dname in &disk_names {
        let src = format!("{}/{}.qcow2", disk_path, dname);
        let dst = format!("{}/{}.qcow2", backup_dir, dname);
        if !std::path::Path::new(&src).exists() {
            continue;
        }
        // Check if linked clone — flatten with qemu-img convert
        let has_backing = get_disk_backing_info(dname).unwrap_or(None).is_some();
        let result = if has_backing {
            crate::ssh::run_cmd(&qemu_img, &["convert", "-O", "qcow2", &src, &dst])
        } else {
            std::fs::copy(&src, &dst)
                .map(|_| String::new())
                .map_err(|e| format!("Copy failed: {}", e))
        };
        if let Err(e) = result {
            // Rollback: remove partial backup
            let _ = std::fs::remove_dir_all(&backup_dir);
            return Err(format!("Backup failed for disk '{}': {}", dname, e));
        }
        if let Ok(meta) = std::fs::metadata(&dst) {
            total_size += meta.len() as i64;
        }
        backed_up.push(dname.clone());
    }

    if backed_up.is_empty() {
        let _ = std::fs::remove_dir_all(&backup_dir);
        return Err("No disk files found to backup".into());
    }

    // Write metadata.json
    let meta = serde_json::json!({
        "vm_name": vm_name,
        "disks": backed_up,
        "backup_id": backup_id,
        "created_at": chrono::Local::now().to_rfc3339(),
        "note": note,
    });
    let _ = std::fs::write(
        format!("{}/metadata.json", backup_dir),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    );

    // Insert DB record
    let disk_json = serde_json::to_string(&backed_up).unwrap_or_default();
    db::insert_backup(&backup_id, vm_name, &disk_json, "full", note, total_size)?;

    Ok(format!("Full backup '{}' created ({} disks, {})",
        backup_id, backed_up.len(), format_bytes(total_size as u64)))
}

/// Restore a full backup — copies disk files back to disk_path
pub fn restore_full_backup(backup_id: &str, vm_name: &str) -> Result<String, String> {
    sanitize_name(backup_id)?;
    sanitize_name(vm_name)?;
    let vm = db::get_vm(vm_name)?;
    if vm.status == "running" {
        return Err("VM must be stopped before restoring".into());
    }
    let backup = db::get_backup(backup_id)?;
    let disk_names: Vec<String> = serde_json::from_str(&backup.disk_names).unwrap_or_default();
    let disk_path = get_conf("disk_path");
    let live_path = get_conf("live_path");
    let backup_dir = format!("{}/full_backups/{}", live_path, backup_id);

    let mut restored = 0;
    for dname in &disk_names {
        let src = format!("{}/{}.qcow2", backup_dir, dname);
        let dst = format!("{}/{}.qcow2", disk_path, dname);
        if !std::path::Path::new(&src).exists() {
            continue;
        }
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("Restore failed for '{}': {}", dname, e))?;
        // Clear backing_file in DB since backup is standalone
        let _ = db::set_disk_backing(dname, "");
        restored += 1;
    }
    Ok(format!("Restored {} disk(s) from backup '{}'", restored, backup_id))
}

/// Delete a full backup (files + DB record)
pub fn delete_full_backup(backup_id: &str) -> Result<String, String> {
    sanitize_name(backup_id)?;
    let live_path = get_conf("live_path");
    let backup_dir = format!("{}/full_backups/{}", live_path, backup_id);
    if std::path::Path::new(&backup_dir).exists() {
        std::fs::remove_dir_all(&backup_dir)
            .map_err(|e| format!("Failed to remove backup dir: {}", e))?;
    }
    db::delete_backup_record(backup_id)?;
    Ok(format!("Deleted backup '{}'", backup_id))
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 { return format!("{} B", bytes); }
    if bytes < 1048576 { return format!("{:.1} KB", bytes as f64 / 1024.0); }
    if bytes < 1073741824 { return format!("{:.1} MB", bytes as f64 / 1048576.0); }
    format!("{:.2} GB", bytes as f64 / 1073741824.0)
}

// ======== Snapshot operations ========

/// Create a qcow2 internal snapshot for all disks of a VM
pub fn create_snapshot(vm_name: &str, note: &str) -> Result<String, String> {
    sanitize_name(vm_name)?;
    let vm = db::get_vm(vm_name)?;
    if vm.status == "running" {
        return Err("VM must be stopped before creating a snapshot".into());
    }
    let disk_names = get_vm_disk_names(vm_name)?;
    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");

    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let rnd = generate_random_password(4);
    let snapshot_id = format!("snap_{}_{}", ts, rnd);

    let mut created: Vec<String> = Vec::new();
    for dname in &disk_names {
        let disk_file = format!("{}/{}.qcow2", disk_path, dname);
        if !std::path::Path::new(&disk_file).exists() {
            continue;
        }
        let result = crate::ssh::run_cmd(&qemu_img, &["snapshot", "-c", &snapshot_id, &disk_file]);
        if let Err(e) = result {
            // Rollback: delete snapshots created so far
            for prev in &created {
                let prev_file = format!("{}/{}.qcow2", disk_path, prev);
                let _ = crate::ssh::run_cmd(&qemu_img, &["snapshot", "-d", &snapshot_id, &prev_file]);
            }
            return Err(format!("Snapshot failed for disk '{}': {}", dname, e));
        }
        db::insert_snapshot(&snapshot_id, dname, vm_name, note)?;
        created.push(dname.clone());
    }

    if created.is_empty() {
        return Err("No disk files found to snapshot".into());
    }
    Ok(format!("Snapshot '{}' created ({} disks)", snapshot_id, created.len()))
}

/// List all snapshots for a VM, grouped by snapshot_id
pub fn list_vm_snapshots(vm_name: &str) -> Result<Vec<serde_json::Value>, String> {
    sanitize_name(vm_name)?;
    let records = db::list_snapshots_by_vm(vm_name)?;
    // Group by snapshot_id
    let mut map: std::collections::BTreeMap<String, serde_json::Value> = std::collections::BTreeMap::new();
    for r in &records {
        let entry = map.entry(r.snapshot_id.clone()).or_insert_with(|| {
            serde_json::json!({
                "snapshot_id": r.snapshot_id,
                "vm_name": r.vm_name,
                "note": r.note,
                "created_at": r.created_at,
                "disks": Vec::<String>::new(),
            })
        });
        if let Some(arr) = entry.get_mut("disks").and_then(|v| v.as_array_mut()) {
            arr.push(serde_json::json!(r.disk_name));
        }
    }
    // Return in reverse order (newest first)
    Ok(map.into_values().rev().collect())
}

/// Revert a VM's disks to a snapshot
pub fn revert_snapshot(vm_name: &str, snapshot_id: &str) -> Result<String, String> {
    sanitize_name(vm_name)?;
    sanitize_name(snapshot_id)?;
    let vm = db::get_vm(vm_name)?;
    if vm.status == "running" {
        return Err("VM must be stopped before reverting to a snapshot".into());
    }
    let records = db::list_snapshots_by_vm(vm_name)?;
    let snap_disks: Vec<&db::SnapshotRecord> = records.iter()
        .filter(|r| r.snapshot_id == snapshot_id)
        .collect();
    if snap_disks.is_empty() {
        return Err(format!("Snapshot '{}' not found for VM '{}'", snapshot_id, vm_name));
    }
    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");

    let mut reverted = 0;
    for r in &snap_disks {
        let disk_file = format!("{}/{}.qcow2", disk_path, r.disk_name);
        if !std::path::Path::new(&disk_file).exists() {
            continue;
        }
        crate::ssh::run_cmd(&qemu_img, &["snapshot", "-a", snapshot_id, &disk_file])
            .map_err(|e| format!("Revert failed for disk '{}': {}", r.disk_name, e))?;
        reverted += 1;
    }
    Ok(format!("Reverted {} disk(s) to snapshot '{}'", reverted, snapshot_id))
}

/// Delete a snapshot from disk(s) and DB
pub fn delete_snapshot(vm_name: &str, snapshot_id: &str) -> Result<String, String> {
    sanitize_name(vm_name)?;
    sanitize_name(snapshot_id)?;
    let records = db::list_snapshots_by_vm(vm_name)?;
    let snap_disks: Vec<&db::SnapshotRecord> = records.iter()
        .filter(|r| r.snapshot_id == snapshot_id)
        .collect();
    if snap_disks.is_empty() {
        return Err(format!("Snapshot '{}' not found for VM '{}'", snapshot_id, vm_name));
    }
    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");

    for r in &snap_disks {
        let disk_file = format!("{}/{}.qcow2", disk_path, r.disk_name);
        if std::path::Path::new(&disk_file).exists() {
            let _ = crate::ssh::run_cmd(&qemu_img, &["snapshot", "-d", snapshot_id, &disk_file]);
        }
        let _ = db::delete_snapshot_record(snapshot_id, &r.disk_name);
    }
    Ok(format!("Deleted snapshot '{}'", snapshot_id))
}

/// Find a saved UEFI NVRAM from the template disk that a VM's disk was cloned from.
/// Checks each of the VM's disks for a matching `{disk_name}_efivars.fd` in the disk directory,
/// or follows the backing chain to find a template NVRAM.
pub fn find_template_nvram(cfg: &VmStartConfig) -> Option<String> {
    let disk_path = get_conf("disk_path");
    for disk in &cfg.disks {
        // Direct: check if this disk has a saved NVRAM (e.g. from clone-as-template)
        let nvram = format!("{}/{}_efivars.fd", disk_path, disk.diskname);
        if std::path::Path::new(&nvram).exists() {
            return Some(nvram);
        }
        // Follow backing chain: check template disk's NVRAM
        if let Ok(Some(backing)) = get_disk_backing_info(&disk.diskname) {
            let backing_nvram = format!("{}/{}_efivars.fd", disk_path, backing);
            if std::path::Path::new(&backing_nvram).exists() {
                return Some(backing_nvram);
            }
        }
    }
    None
}

/// Query the actual backing file from a qcow2 disk header using qemu-img info
pub fn get_disk_backing_info(disk_name: &str) -> Result<Option<String>, String> {
    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");
    let disk_file = format!("{}/{}.qcow2", disk_path, disk_name);
    if !std::path::Path::new(&disk_file).exists() {
        return Ok(None);
    }
    let output = crate::ssh::run_cmd(&qemu_img, &["info", "--output=json", &disk_file])?;
    let info: serde_json::Value = serde_json::from_str(&output)
        .map_err(|e| format!("Failed to parse qemu-img info: {}", e))?;
    Ok(info.get("backing-filename")
        .and_then(|v| v.as_str())
        .map(|s| {
            // Extract just the disk name (without path and .qcow2 extension)
            std::path::Path::new(s)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or(s)
                .to_string()
        }))
}

/// Create a standalone disk — creates .qcow2 file + saves to SQLite
pub fn create_disk(json_str: &str) -> Result<String, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let name = val.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    validate_disk_name(&name)?;

    let size = val.get("size").and_then(|v| v.as_str()).unwrap_or("40G").to_string();
    // Validate size format (number followed by G or M)
    if size.len() < 2
        || (!size.ends_with('G') && !size.ends_with('M'))
        || size[..size.len()-1].parse::<u64>().is_err()
    {
        return Err("Invalid disk size (use format like '40G' or '512M')".into());
    }

    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");
    let _ = std::fs::create_dir_all(&disk_path);

    let disk_file = format!("{}/{}.qcow2", disk_path, name);
    if std::path::Path::new(&disk_file).exists() {
        return Err(format!("Disk '{}' already exists", name));
    }

    let mut output = format!("Creating disk: {}\n", disk_file);
    match run_cmd(&qemu_img, &["create", "-f", "qcow2", &disk_file, &size]) {
        Ok(out) => output.push_str(&out),
        Err(e) => return Err(format!("Failed to create disk: {}", e)),
    }

    // Save to SQLite
    db::insert_disk(&name, &size)?;

    output.push_str(&format!("Disk '{}' ({}) created successfully\n", name, size));
    Ok(output)
}

/// Resize an existing disk
pub fn resize_disk(json_str: &str) -> Result<String, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let name = val.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    validate_disk_name(&name)?;

    let size = val.get("size").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if size.len() < 2
        || (!size.ends_with('G') && !size.ends_with('M'))
        || size[..size.len()-1].parse::<u64>().is_err()
    {
        return Err("Invalid disk size (use format like '40G' or '512M')".into());
    }

    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");

    let disk_file = format!("{}/{}.qcow2", disk_path, name);
    if !std::path::Path::new(&disk_file).exists() {
        return Err(format!("Disk '{}' not found", name));
    }

    check_disk_not_in_use(&name)?;

    let mut output = format!("Resizing disk: {} -> {}\n", disk_file, size);
    match run_cmd(&qemu_img, &["resize", "-f", "qcow2", &disk_file, &size]) {
        Ok(out) => output.push_str(&out),
        Err(e) => return Err(format!("Failed to resize disk: {}", e)),
    }

    db::update_disk_size(&name, &size)?;

    output.push_str(&format!("Disk '{}' resized to {} successfully\n", name, size));
    Ok(output)
}

// --- Port forwarding operations ---

/// Collect all host ports used by port_forwards across all VMs
fn used_host_ports(exclude_smac: Option<&str>) -> Vec<(u16, String, String)> {
    // Returns Vec<(host_port, protocol, vm_name)>
    let mut ports = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if let Some(exc) = exclude_smac {
                if vm.smac == exc { continue; }
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(forwards) = cfg.get("port_forwards").and_then(|v| v.as_array()) {
                    for rule in forwards {
                        let proto = rule.get("protocol").and_then(|v| v.as_str()).unwrap_or("tcp");
                        let hp = rule.get("host_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                        if hp > 0 {
                            ports.push((hp, proto.to_string(), vm.smac.clone()));
                        }
                    }
                }
            }
        }
    }
    ports
}

/// Check that a host port is not already used by another VM's port forward
pub fn validate_host_port_unique(host_port: u16, protocol: &str, exclude_smac: Option<&str>) -> Result<(), String> {
    let used = used_host_ports(exclude_smac);
    for (port, proto, vm_name) in &used {
        if *port == host_port && proto == protocol {
            return Err(format!(
                "Host port {} ({}) is already used by VM '{}'",
                host_port, protocol, vm_name
            ));
        }
    }
    Ok(())
}

/// Add a port forward rule to a VM config. If VM is running, also apply via QEMU monitor.
pub fn add_port_forward(smac: &str, protocol: &str, host_port: u16, guest_port: u16) -> Result<String, String> {
    // Validate protocol
    if protocol != "tcp" && protocol != "udp" {
        return Err("Protocol must be 'tcp' or 'udp'".into());
    }
    // Validate port ranges
    if host_port < 1024 {
        return Err(format!("Host port {} is below 1024 (reserved range)", host_port));
    }
    if guest_port == 0 {
        return Err("Guest port must be 1-65535".into());
    }

    // Check host port uniqueness
    validate_host_port_unique(host_port, protocol, Some(smac))?;

    // Load current config
    let vm = db::get_vm(smac)?;
    let mut config: serde_json::Value = serde_json::from_str(&vm.config)
        .map_err(|e| format!("Config parse error: {}", e))?;

    // Get or create port_forwards array
    let forwards = config.get("port_forwards")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Check for duplicate within this VM
    for rule in &forwards {
        let p = rule.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
        let hp = rule.get("host_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        if p == protocol && hp == host_port {
            return Err(format!(
                "Port forward {}:{} already exists for this VM",
                protocol, host_port
            ));
        }
    }

    // Add new rule
    let mut new_forwards = forwards;
    new_forwards.push(serde_json::json!({
        "protocol": protocol,
        "host_port": host_port,
        "guest_port": guest_port,
    }));
    config["port_forwards"] = serde_json::json!(new_forwards);

    // Save config
    let config_str = serde_json::to_string(&config).unwrap_or_default();
    db::update_vm(smac, &config_str)?;

    let mut output = format!("Port forward added: {}:{} -> guest:{}\n", protocol, host_port, guest_port);

    // If VM is running, apply via QEMU monitor (find first NAT adapter netdev id)
    if vm.status == "running" {
        let netdev_id = find_nat_netdev_id(&config);
        let cmd = format!("hostfwd_add {} {}::{}-:{}", netdev_id, protocol, host_port, guest_port);
        match crate::api_helpers::qemu_monitor_cmd(smac, &cmd) {
            Ok(resp) => {
                if resp.is_empty() || !resp.to_lowercase().contains("error") {
                    output.push_str("Applied to running VM (live)\n");
                } else {
                    output.push_str(&format!("WARNING: live apply failed: {}\n", resp));
                }
            }
            Err(e) => {
                output.push_str(&format!("WARNING: could not apply live (will take effect on next start): {}\n", e));
            }
        }
    } else {
        output.push_str("Will take effect on next VM start\n");
    }

    Ok(output)
}

/// Remove a port forward rule from a VM config. If VM is running, also remove via QEMU monitor.
pub fn remove_port_forward(smac: &str, protocol: &str, host_port: u16) -> Result<String, String> {
    // Load current config
    let vm = db::get_vm(smac)?;
    let mut config: serde_json::Value = serde_json::from_str(&vm.config)
        .map_err(|e| format!("Config parse error: {}", e))?;

    let forwards = config.get("port_forwards")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Find and remove matching rule
    let new_forwards: Vec<serde_json::Value> = forwards.iter()
        .filter(|rule| {
            let p = rule.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
            let hp = rule.get("host_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            !(p == protocol && hp == host_port)
        })
        .cloned()
        .collect();

    if new_forwards.len() == forwards.len() {
        return Err(format!("Port forward {}:{} not found for this VM", protocol, host_port));
    }

    config["port_forwards"] = serde_json::json!(new_forwards);

    // Save config
    let config_str = serde_json::to_string(&config).unwrap_or_default();
    db::update_vm(smac, &config_str)?;

    let mut output = format!("Port forward removed: {}:{}\n", protocol, host_port);

    // If VM is running, remove via QEMU monitor
    if vm.status == "running" {
        let netdev_id = find_nat_netdev_id(&config);
        let cmd = format!("hostfwd_remove {} {}::{}", netdev_id, protocol, host_port);
        match crate::api_helpers::qemu_monitor_cmd(smac, &cmd) {
            Ok(resp) => {
                if resp.is_empty() || !resp.to_lowercase().contains("error") {
                    output.push_str("Removed from running VM (live)\n");
                } else {
                    output.push_str(&format!("WARNING: live remove failed: {}\n", resp));
                }
            }
            Err(e) => {
                output.push_str(&format!("WARNING: could not remove live (will take effect on next start): {}\n", e));
            }
        }
    } else {
        output.push_str("Will take effect on next VM start\n");
    }

    Ok(output)
}

/// Find the QEMU netdev ID for the first NAT adapter
fn find_nat_netdev_id(config: &serde_json::Value) -> String {
    if let Some(adapters) = config.get("network_adapters").and_then(|v| v.as_array()) {
        for adapter in adapters {
            let mode = adapter.get("mode").and_then(|v| v.as_str()).unwrap_or("nat");
            if mode == "nat" || mode.is_empty() {
                let netid = adapter.get("netid").and_then(|v| v.as_str()).unwrap_or("0");
                return format!("net{}", netid);
            }
        }
    }
    "net0".to_string()
}

// --- VNC operations ---

pub fn vnc_start(json_str: &str) -> Result<String, String> {
    // QEMU has built-in WebSocket VNC — this endpoint checks status and returns the actual port.
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let _port = validate_port(&cmd.novncport)?;

    // Check if VM is running
    let vm = db::get_vm(&cmd.smac)?;
    if vm.status != "running" {
        return Err(format!("VM '{}' is not running — start the VM first", cmd.smac));
    }

    // Get actual VNC port from saved config
    let actual_port = if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
        cfg.get("vnc_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16
    } else {
        0
    };

    if actual_port == 0 {
        return Err("VNC port not configured for this VM".into());
    }

    Ok(format!("VNC WebSocket ready on port {} (built-in QEMU)\n", actual_port))
}

pub fn vnc_stop(json_str: &str) -> Result<String, String> {
    // QEMU built-in WebSocket VNC stops when VM stops.
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let _ = validate_port(&cmd.novncport)?;

    // Check if VM exists
    let vm = db::get_vm(&cmd.smac)?;
    if vm.status != "running" {
        return Ok(format!("VM '{}' is already stopped — VNC is not active\n", cmd.smac));
    }

    Ok(format!("VNC for {} will stop when VM stops (built-in QEMU)\n", cmd.smac))
}
