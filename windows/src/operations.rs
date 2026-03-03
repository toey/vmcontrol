use crate::api_helpers::{send_cmd_pctl, set_ma_mode, set_update_status};
use crate::config::get_conf;
use crate::db;
use crate::mds;
use crate::models::*;
use crate::ssh::{run_cmd, sanitize_name, spawn_background, validate_port};

/// Validate VM name — only alphanumeric, underscore, dash allowed
fn validate_vm_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("VM name is required".into());
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

/// Generate a cloud-init NoCloud seed ISO from per-VM MDS config
fn generate_seed_iso(vm_name: &str) -> Result<String, String> {
    let pctl_path = get_conf("pctl_path");
    let seed_dir = format!("{}\\seed_{}", pctl_path, vm_name);
    let iso_path = format!("{}\\seed_{}.iso", pctl_path, vm_name);

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
    let hostname = format!("{}-{}", config.hostname_prefix, vm_name);
    let mut meta_data = String::new();
    meta_data.push_str(&format!("instance-id: {}\n", config.instance_id));
    meta_data.push_str(&format!("local-hostname: {}\n", hostname));
    meta_data.push_str(&format!("ami-id: {}\n", config.ami_id));
    meta_data.push_str(&format!("local-ipv4: {}\n", config.local_ipv4));
    if !config.ssh_pubkey.is_empty() {
        meta_data.push_str("public-keys:\n");
        meta_data.push_str(&format!("  - {}\n", config.ssh_pubkey));
    }

    // Generate user-data (cloud-config for NoCloud)
    let user_data = mds::generate_userdata_nocloud(&config);

    // Write files (no network-config -- let SLIRP DHCP handle networking)
    std::fs::write(format!("{}\\meta-data", seed_dir), &meta_data)
        .map_err(|e| format!("Failed to write meta-data: {}", e))?;
    std::fs::write(format!("{}\\user-data", seed_dir), &user_data)
        .map_err(|e| format!("Failed to write user-data: {}", e))?;

    // Create ISO using oscdimg (Windows ADK) or mkisofs
    // Use run_cmd (not send_cmd) to avoid cmd /C path mangling
    let _ = std::fs::remove_file(&iso_path); // remove old ISO if exists
    let has_oscdimg = run_cmd("where", &["oscdimg"]).is_ok();
    let has_mkisofs = run_cmd("where", &["mkisofs"]).is_ok();

    if has_oscdimg {
        run_cmd("oscdimg", &["-j1", "-lcidata", &seed_dir, &iso_path])
            .map_err(|e| format!("Failed to create seed ISO: {}", e))?;
    } else if has_mkisofs {
        run_cmd("mkisofs", &["-o", &iso_path, "-V", "cidata", "-J", "-R", &seed_dir])
            .map_err(|e| format!("Failed to create seed ISO: {}", e))?;
    } else {
        // Cleanup and return a clear error
        let _ = std::fs::remove_dir_all(&seed_dir);
        return Err("No ISO tool found. Install oscdimg (Windows ADK) or mkisofs (cdrtools)".into());
    }

    // Cleanup seed directory
    let _ = std::fs::remove_dir_all(&seed_dir);

    Ok(iso_path)
}

/// Find a free TCP port starting from `start`, checking up to 100 ports.
/// Ports in `skip` are excluded (e.g. already allocated to running VMs in DB).
fn find_free_port(start: u16, skip: &[u16]) -> u16 {
    for port in start..start.saturating_add(100) {
        if skip.contains(&port) {
            continue;
        }
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    start // fallback
}

/// Start a QEMU VM from config stored in the database
fn start_vm_with_config(smac: &str, cfg: &VmStartConfig) -> Result<String, String> {
    let is_aarch64 = cfg.features.arch == "aarch64";
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
    let _ = std::fs::create_dir_all(&pctl_path);
    let _ = std::fs::create_dir_all(&disk_path);

    let mut output_log = String::new();

    // Use smac as the VM identifier
    let ismac = smac.to_string();

    // Build QEMU arguments safely (no shell involved)
    let mut qemu_args: Vec<String> = Vec::new();

    // Boot options -- aarch64 virt machine uses virtio-gpu instead of -vga std
    // NOTE: no -daemonize — QEMU's daemonize + websocket VNC is unreliable.
    // We use spawn_background() instead to run QEMU in the background.
    if is_aarch64 {
        qemu_args.extend([
            "-nodefaults", "-boot", "d",
        ].map(String::from));
        // UEFI firmware required for aarch64
        let bios = get_conf("edk2_aarch64_bios");
        if !std::path::Path::new(&bios).exists() {
            return Err(format!("aarch64 requires UEFI firmware but '{}' not found. Install EDK2 or set edk2_aarch64_bios in config.", bios));
        }
        qemu_args.push("-bios".into());
        qemu_args.push(bios);
        // CPU for aarch64 — use "max" for broad compatibility (works with both HVF/KVM and TCG)
        qemu_args.push("-cpu".into());
        qemu_args.push("max".into());
        // VGA via virtio-gpu
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
        qemu_args.extend([
            "-nodefaults", "-vga", "std", "-boot", "d",
        ].map(String::from));
    }

    // Windows localtime
    if cfg.features.is_windows == "1" {
        qemu_args.push("-localtime".into());
    }

    // Display VNC with built-in WebSocket (no websockify needed)
    // Prevent VNC port collision: if another running VM already uses this port, auto-reassign
    let mut vnc_port = cfg.vnc_port;
    let running_ports = running_vnc_ports(smac);
    if running_ports.contains(&vnc_port) {
        let new_port = next_free_vnc_port(smac);
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
    if vnc_port <= 12000 || vnc_port > 13000 {
        return Err(format!("VNC port {} out of valid range (12001-13000)", vnc_port));
    }
    let vnc_display = vnc_port - 12000;
    qemu_args.push("-display".into());
    qemu_args.push(format!("vnc=127.0.0.1:{},websocket={}", vnc_display, vnc_port));

    // Memory
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
        // auto-create disk if not exists
        let disk_file = format!("{}\\{}.qcow2", disk_path, disk.diskname);
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
            serde_json::from_value::<mds::MdsConfig>(mds_val.clone())
                .unwrap_or_else(|_| mds::load_mds_config())
        } else {
            mds::load_mds_config()
        }
    } else {
        mds::load_mds_config()
    };
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

    for adapter in &cfg.network_adapters {
        output_log.push_str(&format!("netid : {}\n", adapter.netid));
        output_log.push_str(&format!("mac : {}\n", adapter.mac));
        output_log.push_str(&format!("vlanid : {}\n", adapter.vlan));
        output_log.push_str(&format!("mode : {}\n", adapter.mode));

        qemu_args.push("-netdev".into());

        if adapter.mode == "switch" && !adapter.switch_name.is_empty() {
            // Virtual switch mode — QEMU socket multicast with VLAN
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
                        "socket,id=net{},mcast=230.{}.{}.1:{}",
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
        } else {
            qemu_args.push(format!("user,id=net{}{}", adapter.netid, slirp_opts));
        }

        qemu_args.push("-device".into());
        qemu_args.push(format!("virtio-net-pci,netdev=net{},mac={}", adapter.netid, adapter.mac));
    }

    // Machine type -- configurable accelerator (whpx:tcg for Windows)
    qemu_args.push("-machine".into());
    qemu_args.push(format!("type={},accel={}", qemu_machine, qemu_accel));
    qemu_args.push("-no-shutdown".into());

    // SMP -- correctly compute total = sockets * cores * threads
    let sockets: u32 = cfg.cpu.sockets.parse().unwrap_or(1);
    let cores: u32 = cfg.cpu.cores.parse().unwrap_or(1);
    let threads: u32 = cfg.cpu.threads.parse().unwrap_or(1);
    let total_cpus = sockets * cores * threads;
    qemu_args.push("-smp".into());
    qemu_args.push(format!("{},sockets={},cores={},threads={}",
        total_cpus, sockets, cores, threads));

    // CDROM -- named drive "cd0" for runtime ISO mount/unmount via monitor
    // bootindex=0 so UEFI/BIOS tries CD first, then falls through to disk
    if is_aarch64 {
        // aarch64 virt has no IDE -- use virtio-scsi controller + scsi-cd
        qemu_args.push("-device".into());
        qemu_args.push("virtio-scsi-pci,id=scsi0".into());
        qemu_args.push("-drive".into());
        qemu_args.push("if=none,id=cd0,media=cdrom".into());
        qemu_args.push("-device".into());
        qemu_args.push("scsi-cd,drive=cd0,bootindex=0".into());
    } else {
        qemu_args.push("-drive".into());
        qemu_args.push("if=none,id=cd0,media=cdrom".into());
        qemu_args.push("-device".into());
        qemu_args.push("ide-cd,drive=cd0,bootindex=0".into());
    }

    // Generate cloud-init seed ISO from MDS config (skip if cloud-init disabled)
    let cloudinit_enabled = cfg.features.cloudinit != "0";
    if cloudinit_enabled {
        match generate_seed_iso(&ismac) {
            Ok(seed_iso_path) => {
                output_log.push_str(&format!("seed ISO : {}\n", seed_iso_path));
                if is_aarch64 {
                    qemu_args.push("-drive".into());
                    qemu_args.push(format!(
                        "file={},if=none,id=seed0,media=cdrom,readonly=on", seed_iso_path
                    ));
                    qemu_args.push("-device".into());
                    qemu_args.push("virtio-blk-pci,drive=seed0".into());
                } else {
                    qemu_args.push("-drive".into());
                    qemu_args.push(format!(
                        "file={},if=ide,index=1,media=cdrom,readonly=on", seed_iso_path
                    ));
                }
            }
            Err(e) => {
                output_log.push_str(&format!("WARNING: seed ISO generation failed: {}\n", e));
            }
        };

        // SMBIOS cloud-init hint (x86_64 only -- aarch64 virt doesn't support SMBIOS)
        if !is_aarch64 {
            qemu_args.push("-smbios".into());
            qemu_args.push("type=11,value=cloud-init:ds=nocloud".into());
        }
    } else {
        output_log.push_str("cloud-init: disabled\n");
    }

    // USB tablet (for mouse) -- aarch64 already has xhci+kbd+tablet from above
    if !is_aarch64 {
        qemu_args.extend(["-usb", "-device", "usb-tablet,bus=usb-bus.0,port=1"].map(String::from));
    }

    // Monitor TCP socket (Windows)
    let monitor_port = find_free_port(55555, &[]);
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("tcp:127.0.0.1:{},server,nowait", monitor_port));
    // Save monitor port to file so api_helpers can read it
    let monitor_port_file = format!(r"{}\{}.port", pctl_path, ismac);
    let _ = std::fs::write(&monitor_port_file, monitor_port.to_string());

    // Start VM as a background process (no -daemonize, which breaks WebSocket VNC)
    let args_ref: Vec<&str> = qemu_args.iter().map(|s| s.as_str()).collect();
    output_log.push_str(&format!("QEMU: {} {}\n", qemu_path, qemu_args.join(" ")));
    let (pid, log_path) = spawn_background(&qemu_path, &args_ref)
        .map_err(|e| format!("QEMU start error: {}", e))?;
    output_log.push_str(&format!("QEMU started (PID {})\n", pid));
    output_log.push_str(&format!("QEMU log: {}\n", log_path));

    // Set status to running
    if let Err(e) = db::set_vm_status(smac, "running") {
        output_log.push_str(&format!("WARNING: DB status update failed: {}\n", e));
    }

    Ok(output_log)
}

/// Start VM by smac -- loads config from DB
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
    let pctl_result = send_cmd_pctl("stop", &cmd.smac);
    output.push_str(&pctl_result);
    // Also try taskkill as fallback if monitor failed
    if pctl_result.contains("Error") || pctl_result.contains("error") {
        output.push_str("Monitor unavailable, trying taskkill...\n");
        let _ = run_cmd("taskkill", &["/F", "/IM", "qemu-system-x86_64.exe"]);
        let _ = run_cmd("taskkill", &["/F", "/IM", "qemu-system-aarch64.exe"]);
        output.push_str("Sent taskkill for QEMU processes\n");
    }
    // Set status to stopped + clear qemu_vnc_port
    if let Err(e) = db::set_vm_status(&cmd.smac, "stopped") {
        output.push_str(&format!("WARNING: DB status update failed: {}\n", e));
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
    let sock_path = format!("{}\\{}", pctl_path, cmd.smac);
    if !std::path::Path::new(&sock_path).exists() {
        let _ = db::set_vm_status(&cmd.smac, "stopped");
        output.push_str("VM stopped.\n");
    } else {
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

/// List images for a VM -- uses safe directory listing instead of shell
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

/// Collect all QEMU VNC display ports (5900+) from running VMs in DB
fn used_qemu_vnc_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    if let Ok(vms) = db::list_vms() {
        for vm in &vms {
            if vm.status != "running" {
                continue;
            }
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(p) = cfg.get("qemu_vnc_port").and_then(|v| v.as_u64()) {
                    ports.push(p as u16);
                }
            }
        }
    }
    ports
}

/// Create VM config -- save to DB + assign disk owners
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
fn next_free_vnc_port(exclude_smac: &str) -> u16 {
    let running = running_vnc_ports(exclude_smac);
    let all_used = used_vnc_ports();
    let mut port: u16 = 12001;
    while (running.contains(&port) || all_used.contains(&port)) && port < 13000 {
        port += 2;
    }
    port
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

/// Find next available Local IPv4: 10.0.{subnet}.10
/// Supports up to 10.0.254.10 (254 subnets) then wraps to 10.1.x.10, etc.
fn next_ipv4() -> String {
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

/// Validate that MAC addresses in a config are unique across all VMs.
/// exclude_smac: if updating a VM, exclude its own MACs from the check.
fn validate_mac_uniqueness(config: &serde_json::Value, exclude_smac: Option<&str>) -> Result<(), String> {
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

/// Find next available VNC port starting from 12001, step by 2
fn next_vnc_port() -> u16 {
    let used = used_vnc_ports();
    let mut port: u16 = 12001;
    while used.contains(&port) && port < 13000 {
        port += 2;
    }
    port
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
        let port = next_vnc_port();
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
            if config.get("mds").is_none() {
                config["mds"] = serde_json::json!({});
            }
            config["mds"]["local_ipv4"] = serde_json::json!(ip);
        }
    }
    // Validate MAC address uniqueness before saving
    validate_mac_uniqueness(&config, None)?;

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
    let config = val.get("config").unwrap_or(&empty_obj);

    // Validate MAC address uniqueness (exclude this VM's own MACs)
    validate_mac_uniqueness(config, Some(&smac))?;

    let config_str = serde_json::to_string(config).unwrap_or_default();

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

pub fn mountiso(json_str: &str) -> Result<String, String> {
    let cmd: MountIsoCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    sanitize_name(&cmd.isoname)?;
    let output = send_cmd_pctl("mountiso", &format!("{} {}", cmd.smac, cmd.isoname));
    Ok(output)
}

pub fn unmountiso(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let output = send_cmd_pctl("unmountiso", &cmd.smac);
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
    let output = send_cmd_pctl("backup", &cmd.smac);
    Ok(output)
}

/// Create a standalone disk -- creates .qcow2 file + saves to SQLite
pub fn create_disk(json_str: &str) -> Result<String, String> {
    let val: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let name = val.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return Err("Disk name is required".into());
    }
    sanitize_name(&name)?;

    let size = val.get("size").and_then(|v| v.as_str()).unwrap_or("40G").to_string();
    // Validate size format (number followed by G or M)
    if size.len() < 2
        || !size.ends_with('G') && !size.ends_with('M')
        || size[..size.len()-1].parse::<u64>().is_err()
    {
        return Err("Invalid disk size (use format like '40G' or '512M')".into());
    }

    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");
    let _ = std::fs::create_dir_all(&disk_path);

    let disk_file = format!("{}\\{}.qcow2", disk_path, name);
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
    if name.is_empty() {
        return Err("Disk name is required".into());
    }
    sanitize_name(&name)?;

    let size = val.get("size").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if size.len() < 2
        || !size.ends_with('G') && !size.ends_with('M')
        || size[..size.len()-1].parse::<u64>().is_err()
    {
        return Err("Invalid disk size (use format like '40G' or '512M')".into());
    }

    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");

    let disk_file = format!("{}\\{}.qcow2", disk_path, name);
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

// --- VNC operations ---

pub fn vnc_start(json_str: &str) -> Result<String, String> {
    // QEMU has built-in WebSocket VNC -- this endpoint checks status and returns the actual port.
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let _port = validate_port(&cmd.novncport)?;

    // Check if VM is running
    let vm = db::get_vm(&cmd.smac)?;
    if vm.status != "running" {
        return Err(format!("VM '{}' is not running -- start the VM first", cmd.smac));
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
        return Ok(format!("VM '{}' is already stopped -- VNC is not active\n", cmd.smac));
    }

    Ok(format!("VNC for {} will stop when VM stops (built-in QEMU)\n", cmd.smac))
}
