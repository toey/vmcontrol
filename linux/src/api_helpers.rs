use crate::config::get_conf;
use crate::ssh::{sanitize_name, validate_ip};

pub fn curl_request(url: &str) {
    match ureq::get(url).call() {
        Ok(resp) => {
            let body = resp.into_body().read_to_string().unwrap_or_default();
            log::debug!("curl_request: size={} url={}", body.len(), url);
        }
        Err(e) => {
            log::error!("curl_request error: {}", e);
        }
    }
}

pub fn set_ma_mode(mode: &str, smac: &str) {
    let domain = get_conf("domain");
    curl_request(&format!(
        "https://{}/api/v1.0/instances/{}/update-ma-mode/{}",
        domain, smac, mode
    ));
}

pub fn set_update_status(mode: &str, smac: &str) {
    let domain = get_conf("domain");
    curl_request(&format!(
        "https://{}/api/v1.0/instances/{}/update-status/{}",
        domain, smac, mode
    ));
}

/// Send a command to QEMU monitor via Unix socket (native Rust)
pub fn qemu_monitor_cmd(smac: &str, command: &str) -> Result<String, String> {
    use std::io::{Read, Write};
    use std::time::Duration;
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;
    #[cfg(windows)]
    use uds_windows::UnixStream;

    let pctl_path = get_conf("pctl_path");
    let sock_path = format!("{}/{}", pctl_path, smac);

    let mut stream = UnixStream::connect(&sock_path)
        .map_err(|e| format!("Monitor socket connect failed ({}): {}", sock_path, e))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|e| format!("Set timeout error: {}", e))?;

    // Read and discard the initial QEMU prompt
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);

    // Send the command
    stream
        .write_all(format!("{}\n", command).as_bytes())
        .map_err(|e| format!("Write error: {}", e))?;

    // Wait for QEMU to process, then read response
    std::thread::sleep(Duration::from_millis(500));
    let mut response = String::new();
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(e) => return Err(format!("Read error: {}", e)),
        }
    }

    // Clean up: remove ANSI codes, prompts, empty lines
    let clean: Vec<&str> = response
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.contains("(qemu)") && !l.contains("QEMU") && !l.is_empty())
        .collect();

    Ok(clean.join("\n"))
}

/// Map pctl mode to QEMU monitor command and execute
pub fn send_cmd_pctl(mode: &str, smac: &str) -> String {
    // Map mode to QEMU HMP command
    let (vm_name, qemu_cmd) = match mode {
        "stop" => (smac.to_string(), "quit".to_string()),
        "reset" => (smac.to_string(), "system_reset".to_string()),
        "powerdown" => (smac.to_string(), "system_powerdown".to_string()),
        "mountiso" => {
            // smac is "vmname isoname drive" for mountiso
            let parts: Vec<&str> = smac.splitn(3, ' ').collect();
            if parts.len() < 2 {
                return "Error: mountiso requires smac and isoname\n".to_string();
            }
            let vm = parts[0].to_string();
            let iso = parts[1];
            let drive = if parts.len() >= 3 { parts[2] } else { "cd0" };
            // Validate drive name (cd0–cd3)
            if !matches!(drive, "cd0" | "cd1" | "cd2" | "cd3") {
                return format!("Error: invalid drive '{}', must be cd0–cd3\n", drive);
            }
            // Validate ISO name to prevent monitor command injection
            if let Err(e) = sanitize_name(iso) {
                return format!("Error: invalid ISO name: {}\n", e);
            }
            let iso_path = get_conf("iso_path");
            let cmd = format!("change {} {}/{}", drive, iso_path, iso);
            (vm, cmd)
        }
        "unmountiso" => {
            // smac is "vmname drive" for unmountiso
            let parts: Vec<&str> = smac.splitn(2, ' ').collect();
            let vm = parts[0].to_string();
            let drive = if parts.len() >= 2 { parts[1] } else { "cd0" };
            // Validate drive name (cd0–cd3)
            if !matches!(drive, "cd0" | "cd1" | "cd2" | "cd3") {
                return format!("Error: invalid drive '{}', must be cd0–cd3\n", drive);
            }
            (vm, format!("eject {}", drive))
        }
        "livemigrate" => {
            let parts: Vec<&str> = smac.splitn(2, ' ').collect();
            if parts.len() < 2 {
                return "Error: livemigrate requires smac and target ip\n".to_string();
            }
            let vm = parts[0].to_string();
            let target = parts[1];
            // Validate IP address
            if let Err(e) = validate_ip(target) {
                return format!("Error: invalid target IP: {}\n", e);
            }
            let cmd = format!("migrate -d tcp:{}:4444", target);
            (vm, cmd)
        }
        "backup" => {
            let live_path = get_conf("live_path");
            let gzip_path = get_conf("gzip_path");
            let _ = std::fs::create_dir_all(&live_path);
            // Timestamp-based backup filename: vmname_YYYYMMDD_HHMMSS.gz
            let now = chrono::Local::now();
            let ts = now.format("%Y%m%d_%H%M%S");
            let cmd = format!(
                "migrate \"exec: {} -c > {}/{}_{}.gz\"",
                gzip_path, live_path, smac, ts
            );
            (smac.to_string(), cmd)
        }
        _ => {
            return format!("Error: unknown pctl mode '{}'\n", mode);
        }
    };

    let mut output = format!("monitor({}) => {}\n", vm_name, qemu_cmd);
    match qemu_monitor_cmd(&vm_name, &qemu_cmd) {
        Ok(resp) => {
            let clean = resp.trim().to_string();
            if !clean.is_empty() {
                output.push_str(&clean);
                output.push('\n');
            }
            output.push_str("OK\n");
        }
        Err(e) => {
            output.push_str(&format!("Error: {}\n", e));
        }
    }
    output
}
