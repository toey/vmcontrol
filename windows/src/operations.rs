use crate::api_helpers::{send_cmd_pctl, set_ma_mode, set_update_status};
use crate::config::get_conf;
use crate::db;
use crate::mds;
use crate::models::*;
use crate::ssh::{send_cmd, run_cmd, sanitize_name, validate_port, spawn_cmd_with_log};

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
    let qemu_path = get_conf("qemu_path");
    let pctl_path = get_conf("pctl_path");
    let disk_path = get_conf("disk_path");
    let live_path = get_conf("live_path");
    let gzip_path = get_conf("gzip_path");
    let qemu_accel = get_conf("qemu_accel");
    let qemu_machine = get_conf("qemu_machine");

    // ensure directories exist
    let _ = std::fs::create_dir_all(&pctl_path);
    let _ = std::fs::create_dir_all(&disk_path);

    let livemode = "0";
    let mut output_log = String::new();

    // Use smac as the VM identifier
    let ismac = smac.to_string();

    // Build QEMU arguments safely (no shell involved)
    let mut qemu_args: Vec<String> = Vec::new();

    // Boot options
    qemu_args.extend([
        "-nodefaults", "-vga", "std", "-boot", "d",
    ].map(String::from));

    // Windows localtime
    if cfg.features.is_windows == "1" {
        qemu_args.push("-localtime".into());
    }

    // Windows: QEMU uses TCP for VNC and monitor (no Unix sockets)
    // Find a free VNC port, skipping ports already used by running VMs (from DB)
    let db_used_ports = used_qemu_vnc_ports();
    let vnc_port = find_free_port(5900, &db_used_ports);
    let vnc_display = vnc_port - 5900;
    qemu_args.push("-display".into());
    qemu_args.push(format!("vnc=0.0.0.0:{}", vnc_display));
    output_log.push_str(&format!("VNC port: {} (display :{})\n", vnc_port, vnc_display));

    // Save qemu_vnc_port to DB config so websockify knows where to connect
    if let Ok(vm) = db::get_vm(&ismac) {
        let mut dbcfg: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
        dbcfg["qemu_vnc_port"] = serde_json::json!(vnc_port);
        let _ = db::update_vm(&ismac, &serde_json::to_string(&dbcfg).unwrap_or_default());
    }

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
        qemu_args.push("-drive".into());
        qemu_args.push(format!(
            "file={},format=qcow2,if=virtio,index={}",
            disk_file, disk.diskid
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

        qemu_args.push("-netdev".into());
        qemu_args.push(format!("user,id=net{}{}", adapter.netid, slirp_opts));
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

    // CDROM
    qemu_args.push("-drive".into());
    qemu_args.push("if=ide,index=0,media=cdrom".into());

    // Generate cloud-init seed ISO from MDS config
    match generate_seed_iso(&ismac) {
        Ok(seed_iso_path) => {
            output_log.push_str(&format!("seed ISO : {}\n", seed_iso_path));
            qemu_args.push("-drive".into());
            qemu_args.push(format!(
                "file={},if=ide,index=1,media=cdrom,readonly=on", seed_iso_path
            ));
        }
        Err(e) => {
            output_log.push_str(&format!("WARNING: seed ISO generation failed: {}\n", e));
        }
    };

    // SMBIOS cloud-init hint
    qemu_args.push("-smbios".into());
    qemu_args.push("type=11,value=cloud-init:ds=nocloud".into());

    // USB tablet (for mouse)
    qemu_args.extend(["-usb", "-device", "usb-tablet,bus=usb-bus.0,port=1"].map(String::from));

    // Monitor TCP socket (Windows)
    let monitor_port = find_free_port(55555, &[]);
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("tcp:127.0.0.1:{},server,nowait", monitor_port));
    // Save monitor port to file so api_helpers can read it
    let monitor_port_file = format!(r"{}\{}.port", pctl_path, ismac);
    let _ = std::fs::write(&monitor_port_file, monitor_port.to_string());

    // Live migration incoming (if applicable)
    if livemode == "1" {
        qemu_args.push("-incoming".into());
        qemu_args.push(format!("exec: {} -c -d {}\\{}.gz", gzip_path, live_path, ismac));
    }

    // Start VM as background process with stderr logging
    let args_ref: Vec<&str> = qemu_args.iter().map(|s| s.as_str()).collect();
    let log_path = format!(r"{}\logs\qemu_{}.log", pctl_path, ismac);
    output_log.push_str(&format!("QEMU: {} {}\n", qemu_path, qemu_args.join(" ")));
    let mut child = spawn_cmd_with_log(&qemu_path, &args_ref, &log_path)
        .map_err(|e| format!("QEMU start error: {}", e))?;

    let pid = child.id();
    output_log.push_str(&format!("Process started (PID: {})\n", pid));
    output_log.push_str(&format!("QEMU log: {}\n", log_path));

    // Wait briefly then check if QEMU is still alive
    std::thread::sleep(std::time::Duration::from_secs(2));

    match child.try_wait() {
        Ok(Some(exit_status)) => {
            // QEMU exited already -- read the log to find out why
            let stderr_content = std::fs::read_to_string(&log_path).unwrap_or_default();
            let err_msg = if stderr_content.trim().is_empty() {
                format!("QEMU exited immediately with {}", exit_status)
            } else {
                format!("QEMU exited with {}: {}", exit_status, stderr_content.trim())
            };
            let _ = db::set_vm_status(smac, "stopped");
            // Clear qemu_vnc_port from config since QEMU is not running
            if let Ok(vm) = db::get_vm(smac) {
                let mut dbcfg: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
                dbcfg.as_object_mut().map(|m| m.remove("qemu_vnc_port"));
                let _ = db::update_vm(smac, &serde_json::to_string(&dbcfg).unwrap_or_default());
            }
            return Err(err_msg);
        }
        Ok(None) => {
            // Process still running -- good
            output_log.push_str("QEMU process verified alive after 2s\n");
        }
        Err(e) => {
            output_log.push_str(&format!("WARNING: could not check process status: {}\n", e));
        }
    }

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
        let _ = send_cmd("taskkill /F /IM qemu-system-x86_64.exe");
        output.push_str("Sent taskkill for QEMU processes\n");
    }
    // Set status to stopped + clear qemu_vnc_port
    if let Err(e) = db::set_vm_status(&cmd.smac, "stopped") {
        output.push_str(&format!("WARNING: DB status update failed: {}\n", e));
    }
    if let Ok(vm) = db::get_vm(&cmd.smac) {
        let mut dbcfg: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
        dbcfg.as_object_mut().map(|m| m.remove("qemu_vnc_port"));
        let _ = db::update_vm(&cmd.smac, &serde_json::to_string(&dbcfg).unwrap_or_default());
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
    let output = send_cmd_pctl("powerdown", &cmd.smac);
    // Set status to stopped + clear qemu_vnc_port
    let _ = db::set_vm_status(&cmd.smac, "stopped");
    if let Ok(vm) = db::get_vm(&cmd.smac) {
        let mut dbcfg: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
        dbcfg.as_object_mut().map(|m| m.remove("qemu_vnc_port"));
        let _ = db::update_vm(&cmd.smac, &serde_json::to_string(&dbcfg).unwrap_or_default());
    }
    Ok(output)
}

pub fn delete_vm(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
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

/// Find next available VNC port starting from 12001, step by 2
fn next_vnc_port() -> u16 {
    let used = used_vnc_ports();
    let mut port: u16 = 12001;
    while used.contains(&port) {
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
    sanitize_name(&smac)?;

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
    sanitize_name(&smac)?;

    let empty_obj = serde_json::Value::Object(serde_json::Map::new());
    let config = val.get("config").unwrap_or(&empty_obj);
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
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let port = validate_port(&cmd.novncport)?;
    let mut output = String::new();
    output.push_str(&format!(
        "Starting VNC proxy 0.0.0.0:{} -> {}\n",
        port, cmd.smac
    ));

    // Windows: websockify is a long-running proxy -- must be spawned, not blocked on.
    // Try direct 'websockify' first, then fall back to 'python -m websockify'.
    let listen = format!("0.0.0.0:{}", port);
    // Read QEMU VNC port from DB config, default to 5900
    let vnc_port = match db::get_vm(&cmd.smac) {
        Ok(vm) => {
            let cfg: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
            cfg.get("qemu_vnc_port")
                .and_then(|v| v.as_u64())
                .map(|p| p.to_string())
                .unwrap_or_else(|| "5900".to_string())
        }
        Err(_) => "5900".to_string(),
    };
    let target = format!("127.0.0.1:{}", vnc_port);
    let pctl_path = get_conf("pctl_path");

    let mut websockify = get_conf("websockify_path");

    // Auto-detect websockify.exe if not a real path
    if !std::path::Path::new(&websockify).exists() {
        let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
        let search_dirs = vec![
            format!(r"{}\Python", localappdata),
            format!(r"{}\Programs\Python", localappdata),
        ];
        for base in &search_dirs {
            if let Ok(entries) = std::fs::read_dir(base) {
                for entry in entries.flatten() {
                    let ws = entry.path().join("Scripts").join("websockify.exe");
                    if ws.exists() {
                        websockify = ws.to_string_lossy().to_string();
                        break;
                    }
                }
            }
            if std::path::Path::new(&websockify).exists() { break; }
        }
    }

    let has_websockify = std::path::Path::new(&websockify).exists();
    let mut child_pid: u32 = 0;

    if has_websockify {
        use std::process::{Command, Stdio};
        output.push_str(&format!("Using websockify: {}\n", websockify));

        #[cfg(target_os = "windows")]
        let child = {
            use std::os::windows::process::CommandExt;
            const DETACH_FLAGS: u32 = 0x00000200 | 0x01000000;
            Command::new(&websockify)
                .args(&[listen.as_str(), target.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(DETACH_FLAGS)
                .spawn()
                .map_err(|e| format!("VNC start error: {}: {}", websockify, e))?
        };
        #[cfg(not(target_os = "windows"))]
        let child = Command::new(&websockify)
            .args(&[listen.as_str(), target.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("VNC start error: {}: {}", websockify, e))?;

        child_pid = child.id();
        output.push_str(&format!("websockify started (PID: {})\n", child_pid));
    } else {
        use std::process::{Command, Stdio};

        // Get python_path from config, auto-detect if default or not a real path
        let mut python_path = get_conf("python_path");
        if !std::path::Path::new(&python_path).exists() {
            // Auto-detect: search common Windows Python installation paths
            let localappdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
            let search_dirs = vec![
                format!(r"{}\Python", localappdata),
                format!(r"{}\Programs\Python", localappdata),
            ];
            let mut found = String::new();
            'search: for base in &search_dirs {
                if let Ok(entries) = std::fs::read_dir(base) {
                    for entry in entries.flatten() {
                        let p = entry.path().join("python.exe");
                        if p.exists() {
                            found = p.to_string_lossy().to_string();
                            break 'search;
                        }
                    }
                }
            }
            if !found.is_empty() {
                output.push_str(&format!("Auto-detected python: {}\n", found));
                python_path = found;
            } else {
                return Err("VNC start error: python not found. Set python_path in config.yaml".into());
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const DETACH_FLAGS: u32 = 0x00000200 | 0x01000000;
            let child = Command::new(&python_path)
                .args(&["-m", "websockify", listen.as_str(), target.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(DETACH_FLAGS)
                .spawn()
                .map_err(|e| format!("VNC start error: python_path='{}' err={}", python_path, e))?;
            child_pid = child.id();
        }
        #[cfg(not(target_os = "windows"))]
        {
            let child = Command::new(&python_path)
                .args(&["-m", "websockify", listen.as_str(), target.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("VNC start error: python_path='{}' err={}", python_path, e))?;
            child_pid = child.id();
        }
        output.push_str(&format!("websockify started (PID: {})\n", child_pid));
    }

    // Save websockify PID so vnc_stop can kill it
    let ws_pid_file = format!(r"{}\{}.ws_pid", pctl_path, cmd.smac);
    let _ = std::fs::write(&ws_pid_file, child_pid.to_string());

    Ok(output)
}

pub fn vnc_stop(json_str: &str) -> Result<String, String> {
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    sanitize_name(&cmd.smac)?;
    let port = validate_port(&cmd.novncport)?;
    let mut output = String::new();
    output.push_str(&format!("Stopping VNC proxy port {}\n", port));

    // Kill websockify by saved PID
    let pctl_path = get_conf("pctl_path");
    let ws_pid_file = format!(r"{}\{}.ws_pid", pctl_path, cmd.smac);
    if let Ok(pid_str) = std::fs::read_to_string(&ws_pid_file) {
        let pid = pid_str.trim();
        // Kill the process tree (cmd.exe + python3)
        match send_cmd(&format!("taskkill /F /T /PID {}", pid)) {
            Ok(out) => output.push_str(&out),
            Err(e) => output.push_str(&format!("taskkill warning: {}\n", e)),
        }
        let _ = std::fs::remove_file(&ws_pid_file);
    } else {
        output.push_str("No websockify PID file found, process may already be stopped\n");
    }

    Ok(output)
}
