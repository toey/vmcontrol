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

/// Generate a cloud-init NoCloud seed ISO from per-VM MDS config
fn generate_seed_iso(vm_name: &str) -> Result<String, String> {
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
    let hostname = format!("{}-{}", config.hostname_prefix, vm_name);
    let mut meta_data = String::new();
    // Use unique instance-id per VM to ensure cloud-init runs on cloned templates
    let instance_id = if config.instance_id == "i-0000000000000001" || config.instance_id.is_empty() {
        format!("i-{}", vm_name)
    } else {
        config.instance_id.clone()
    };
    meta_data.push_str(&format!("instance-id: {}\n", instance_id));
    meta_data.push_str(&format!("local-hostname: {}\n", hostname));
    meta_data.push_str(&format!("ami-id: {}\n", config.ami_id));
    meta_data.push_str(&format!("local-ipv4: {}\n", config.local_ipv4));
    if !config.ssh_pubkey.is_empty() {
        meta_data.push_str("public-keys:\n");
        meta_data.push_str(&format!("  - {}\n", config.ssh_pubkey));
    }

    // Generate user-data (cloud-config for NoCloud)
    let user_data = mds::generate_userdata_nocloud(&config, &hostname);

    // Write files
    std::fs::write(format!("{}/meta-data", seed_dir), &meta_data)
        .map_err(|e| format!("Failed to write meta-data: {}", e))?;
    std::fs::write(format!("{}/user-data", seed_dir), &user_data)
        .map_err(|e| format!("Failed to write user-data: {}", e))?;

    // Generate network-config for internal NIC (cloud-init NoCloud, version 2)
    if !config.internal_ip.is_empty() {
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

        let internal_mac = derive_internal_mac(&config.internal_ip);
        let mut net_cfg = String::from("version: 2\nethernets:\n");
        // Primary NIC — DHCP from SLIRP (preserves existing behavior)
        if !primary_mac.is_empty() {
            net_cfg.push_str("  primary:\n");
            net_cfg.push_str("    match:\n");
            net_cfg.push_str(&format!("      macaddress: \"{}\"\n", primary_mac));
            net_cfg.push_str("    dhcp4: true\n");
        }
        // Internal NIC — static IP on shared 192.168.100.0/24 subnet
        net_cfg.push_str("  internal:\n");
        net_cfg.push_str("    match:\n");
        net_cfg.push_str(&format!("      macaddress: \"{}\"\n", internal_mac));
        net_cfg.push_str("    addresses:\n");
        net_cfg.push_str(&format!("      - {}/24\n", config.internal_ip));

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

    Ok(iso_path)
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

    // Boot options — aarch64 virt machine uses virtio-gpu instead of -vga std
    // NOTE: no -daemonize — QEMU's daemonize + websocket VNC crashes on macOS.
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
        // CPU for x86_64 — use "max" to expose all host-supported features
        // (SSE4.2, AVX, etc.) which modern software like Splunk requires
        qemu_args.push("-cpu".into());
        qemu_args.push("max".into());
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
        // auto-create disk if not exists
        let disk_file = format!("{}/{}.qcow2", disk_path, disk.diskname);
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
        qemu_args.push(format!("virtio-net-pci,netdev=net{},mac={}", adapter.netid, adapter.mac));
    }

    // Internal network — shared L2 multicast for VM-to-VM communication
    if !mds_config.internal_ip.is_empty() {
        let internal_mac = derive_internal_mac(&mds_config.internal_ip);
        let internal_mcast_port = get_conf_or("internal_mcast_port", "11111");

        output_log.push_str(&format!("internal_ip : {}\n", mds_config.internal_ip));
        output_log.push_str(&format!("internal_mac: {}\n", internal_mac));
        output_log.push_str(&format!("internal_net: 230.0.100.1:{}\n", internal_mcast_port));

        qemu_args.push("-netdev".into());
        qemu_args.push(format!(
            "socket,id=netint,mcast=230.0.100.1:{}",
            internal_mcast_port
        ));
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
    qemu_args.push("-machine".into());
    qemu_args.push(format!("type={},accel={}", qemu_machine, qemu_accel));

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

    // SCSI controller (shared by seed ISO + runtime CD-ROM drives)
    qemu_args.push("-device".into());
    qemu_args.push("virtio-scsi-pci,id=scsi0".into());

    // Cloud-init seed ISO — MUST be first scsi-cd so guest sees it as /dev/sr0
    // (some cloud-init versions only scan sr0, or have timing issues with later devices)
    let cloudinit_enabled = cfg.features.cloudinit != "0";
    if cloudinit_enabled {
        match generate_seed_iso(&ismac) {
            Ok(seed_iso_path) => {
                output_log.push_str(&format!("seed ISO : {}\n", seed_iso_path));
                qemu_args.push("-drive".into());
                qemu_args.push(format!(
                    "file={},if=none,id=seed0,media=cdrom,readonly=on", seed_iso_path
                ));
                qemu_args.push("-device".into());
                qemu_args.push("scsi-cd,drive=seed0".into());
            }
            Err(e) => {
                output_log.push_str(&format!("WARNING: seed ISO generation failed: {}\n", e));
            }
        };

        // SMBIOS cloud-init hints — tells ds-identify to use NoCloud
        // type=1 serial: supported by all cloud-init versions (checks DMI product serial)
        // type=11 OEM string: supported by cloud-init 23.x+ (checks OEM strings)
        qemu_args.push("-smbios".into());
        qemu_args.push("type=1,serial=ds=nocloud".into());
        qemu_args.push("-smbios".into());
        qemu_args.push("type=11,value=cloud-init:ds=nocloud".into());
    } else {
        output_log.push_str("cloud-init: disabled\n");
    }

    // CDROM — 4 named drives "cd0"–"cd3" for runtime ISO mount/unmount via monitor
    // bootindex=0 on cd0 so UEFI/BIOS tries CD first, then falls through to disk
    for i in 0..4u8 {
        let drive_id = format!("cd{}", i);
        qemu_args.push("-drive".into());
        qemu_args.push(format!("if=none,id={},media=cdrom", drive_id));
        qemu_args.push("-device".into());
        if i == 0 {
            qemu_args.push(format!("scsi-cd,drive={},bootindex=0", drive_id));
        } else {
            qemu_args.push(format!("scsi-cd,drive={}", drive_id));
        }
    }

    // USB tablet (for mouse) — aarch64 already has xhci+kbd+tablet from above
    if !is_aarch64 {
        qemu_args.extend(["-usb", "-device", "usb-tablet,bus=usb-bus.0,port=1"].map(String::from));
    }

    // Monitor socket
    qemu_args.push("-monitor".into());
    qemu_args.push(format!("unix:{}/{},server,nowait", pctl_path, ismac));

    // Start VM as a background process (no -daemonize, which breaks WebSocket VNC)
    // Bridge mode requires sudo for vmnet (macOS) / tap (Linux)
    let needs_sudo = cfg.network_adapters.iter().any(|a| a.mode == "bridge");
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
/// Uses system time + process ID for randomness to avoid requiring the rand crate.
pub fn generate_random_mac() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let pid = std::process::id();
    // Simple LCG-based pseudo-random using nanos and pid
    let mut rng = nanos.wrapping_mul(1664525).wrapping_add(pid);
    let mut octets = [0u8; 3];
    for o in &mut octets {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        *o = (rng >> 16) as u8;
    }
    format!("52:54:00:{:02x}:{:02x}:{:02x}", octets[0], octets[1], octets[2])
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
    let output = send_cmd_pctl("backup", &cmd.smac);
    Ok(output)
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
