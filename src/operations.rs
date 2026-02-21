use crate::api_helpers::{send_cmd_pctl, set_ma_mode, set_update_status};
use crate::config::get_conf;
use crate::models::*;
use crate::ssh::send_cmd;

pub fn start(json_str: &str) -> Result<String, String> {
    let cfg: VmStartConfig =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;

    let qemu_path = get_conf("qemu_path");
    let pctl_path = get_conf("pctl_path");
    let disk_path = get_conf("disk_path");
    let live_path = get_conf("live_path");
    let gzip_path = get_conf("gzip_path");

    // ensure directories exist
    let _ = std::fs::create_dir_all(&pctl_path);
    let _ = std::fs::create_dir_all(&disk_path);

    let livemode = "0";
    let mut output_log = String::new();

    // gen disk
    let mut ismac = String::new();
    let mut xdisk_cmd = String::new();
    for (ix, disk) in cfg.disks.iter().enumerate() {
        if ix == 0 {
            ismac = disk.diskname.clone();
        }
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
            if let Ok(out) = send_cmd(&format!("{} create -f qcow2 {} 10G", qemu_img, disk_file)) {
                output_log.push_str(&out);
            }
        }
        xdisk_cmd.push_str(&format!(
            " -drive file={},format=qcow2,if=virtio,index={}",
            disk_file, disk.diskid
        ));
    }

    if ismac.is_empty() {
        return Err("disk name is required (first disk's diskname is used as VM identifier)".into());
    }

    // gen network adapter (user-mode networking for local)
    let mut xnic_cmd = String::new();
    for adapter in &cfg.network_adapters {
        output_log.push_str(&format!("netid : {}\n", adapter.netid));
        output_log.push_str(&format!("mac : {}\n", adapter.mac));
        output_log.push_str(&format!("vlanid : {}\n", adapter.vlan));

        xnic_cmd.push_str(&format!(
            " -netdev user,id=net{netid} -device virtio-net-pci,netdev=net{netid},mac={mac}",
            netid = adapter.netid,
            mac = adapter.mac,
        ));
    }

    let defaultboot = " -nodefaults -vga std -boot d -daemonize ";
    let vm_memory = format!(" -m {}M ", cfg.memory.size);
    let smp_cmd = format!(
        " -smp {},sockets={},cores={},threads={} ",
        cfg.cpu.cores, cfg.cpu.sockets, cfg.cpu.cores, cfg.cpu.threads
    );
    let displayvncsock = format!(" -display vnc=unix:{}/vncsock_{} ", pctl_path, ismac);
    let monitorunix = format!(
        " -monitor unix:{}/{},server,nowait ",
        pctl_path, ismac
    );
    let cdromdevice = " -drive if=ide,index=0,media=cdrom ";
    let usbdevice = " -usb -device usb-tablet,bus=usb-bus.0,port=1 ";
    let machinetype = " -machine type=pc-i440fx-9.2,accel=tcg ";

    // check live
    let live_mode_cmd = if livemode == "1" {
        format!(
            " -incoming \"exec: {} -c -d {}/{}.gz\" ",
            gzip_path, live_path, ismac
        )
    } else {
        String::new()
    };

    // check windows
    let windows_cmd = if cfg.features.is_windows == "1" {
        " -localtime "
    } else {
        ""
    };

    let full_cmd = format!(
        "{}{}{}{}{}{}{}{}{}{}{}{}{}",
        qemu_path,
        defaultboot,
        windows_cmd,
        displayvncsock,
        vm_memory,
        xdisk_cmd,
        xnic_cmd,
        machinetype,
        smp_cmd,
        cdromdevice,
        usbdevice,
        monitorunix,
        live_mode_cmd,
    );

    // start vm
    let cmd_output = send_cmd(&full_cmd).map_err(|e| format!("send cmd error: {}", e))?;
    output_log.push_str(&cmd_output);

    Ok(output_log)
}

pub fn stop(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let mut output = format!("now stopping compute {}\n", cmd.smac);
    output.push_str(&send_cmd_pctl("stop", &cmd.smac));
    Ok(output)
}

pub fn reset(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let output = send_cmd_pctl("reset", &cmd.smac);
    Ok(output)
}

pub fn powerdown(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let output = send_cmd_pctl("powerdown", &cmd.smac);
    Ok(output)
}

pub fn delete_vm(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let disk_path = get_conf("disk_path");
    let mut output = String::new();
    set_ma_mode("1", &cmd.smac);
    if let Ok(out) = send_cmd(&format!("rm -f {}/{}.qcow2", disk_path, cmd.smac)) {
        output.push_str(&out);
    }
    set_update_status("2", &cmd.smac);
    set_ma_mode("0", &cmd.smac);
    Ok(output)
}

pub fn listimage(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let disk_path = get_conf("disk_path");
    let mut output = String::new();
    set_ma_mode("1", &cmd.smac);
    if let Ok(out) = send_cmd(&format!("ls -lh {}/{}*", disk_path, cmd.smac)) {
        output.push_str(&out);
    }
    set_update_status("2", &cmd.smac);
    set_ma_mode("0", &cmd.smac);
    Ok(output)
}

pub fn create(json_str: &str) -> Result<String, String> {
    let cmd: CreateCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");
    let _ = std::fs::create_dir_all(&disk_path);
    let mut output = String::new();
    if let Ok(out) = send_cmd(&format!(
        "{} create -f qcow2 {}/{}.qcow2 {}",
        qemu_img, disk_path, cmd.smac, cmd.size
    )) {
        output.push_str(&out);
    }
    if let Ok(out) = send_cmd(&format!("{} info {}/{}.qcow2", qemu_img, disk_path, cmd.smac)) {
        output.push_str(&out);
    }
    Ok(output)
}

pub fn copyimage(json_str: &str) -> Result<String, String> {
    let cmd: CopyImageCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let disk_path = get_conf("disk_path");
    let qemu_img = get_conf("qemu_img_path");
    let _ = std::fs::create_dir_all(&disk_path);
    let mut output = String::new();
    if let Ok(out) = send_cmd(&format!(
        "cp {}/{}.qcow2 {}/{}.qcow2",
        disk_path, cmd.itemplate, disk_path, cmd.smac
    )) {
        output.push_str(&out);
    }
    if let Ok(out) = send_cmd(&format!(
        "{} resize {}/{}.qcow2 {}",
        qemu_img, disk_path, cmd.smac, cmd.size
    )) {
        output.push_str(&out);
    }
    if let Ok(out) = send_cmd(&format!("{} info {}/{}.qcow2", qemu_img, disk_path, cmd.smac)) {
        output.push_str(&out);
    }
    Ok(output)
}

pub fn mountiso(json_str: &str) -> Result<String, String> {
    let cmd: MountIsoCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let output = send_cmd_pctl("mountiso", &format!("{} {}", cmd.smac, cmd.isoname));
    Ok(output)
}

pub fn unmountiso(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let output = send_cmd_pctl("unmountiso", &cmd.smac);
    Ok(output)
}

pub fn livemigrate(json_str: &str) -> Result<String, String> {
    let cmd: LiveMigrateCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let output = send_cmd_pctl(
        "livemigrate",
        &format!("{} {}", cmd.smac, cmd.to_node_ip),
    );
    Ok(output)
}

pub fn backup(json_str: &str) -> Result<String, String> {
    let cmd: SimpleCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let output = send_cmd_pctl("backup", &cmd.smac);
    Ok(output)
}

// --- VNC operations ---

pub fn vnc_start(json_str: &str) -> Result<String, String> {
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let pctl_path = get_conf("pctl_path");
    let mut output = String::new();
    output.push_str(&format!(
        "Starting VNC proxy 0.0.0.0:{} -> vncsock_{}\n",
        cmd.novncport, cmd.smac
    ));
    let websockify = get_conf("websockify_path");
    let run_cmd = format!(
        "{} --unix-target={}/vncsock_{} -D 0.0.0.0:{}",
        websockify, pctl_path, cmd.smac, cmd.novncport
    );
    match send_cmd(&run_cmd) {
        Ok(out) => output.push_str(&out),
        Err(e) => return Err(format!("VNC start error: {}", e)),
    }
    Ok(output)
}

pub fn vnc_stop(json_str: &str) -> Result<String, String> {
    let cmd: VncCmd =
        serde_json::from_str(json_str).map_err(|e| format!("JSON parse error: {}", e))?;
    let mut output = String::new();
    output.push_str(&format!("Stopping VNC proxy port {}\n", cmd.novncport));
    let run_cmd = format!("pkill -f \"0.0.0.0:{}\"", cmd.novncport);
    match send_cmd(&run_cmd) {
        Ok(out) => output.push_str(&out),
        Err(e) => return Err(format!("VNC stop error: {}", e)),
    }
    Ok(output)
}
