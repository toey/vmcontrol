use crate::config::get_conf;
use crate::ssh::send_cmd;

pub fn curl_request(url: &str) {
    match reqwest::blocking::get(url) {
        Ok(resp) => {
            let body = resp.text().unwrap_or_default();
            println!("DEBUG: size=> {}", body.len());
            println!("DEBUG: content=> {}", body);
        }
        Err(e) => {
            eprintln!("ERROR: {}", e);
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

/// Send a command to QEMU monitor via unix socket using shell pipe
/// Strips ANSI escape sequences from output for clean results
pub fn qemu_monitor_cmd(smac: &str, command: &str) -> Result<String, String> {
    let pctl_path = get_conf("pctl_path");
    let sock_path = format!("{}/{}", pctl_path, smac);

    // Use printf + nc -U, then strip ANSI codes and filter prompts
    // nc -U exits with code 1 when connection closes, so use || true to ignore
    let shell_cmd = format!(
        "((sleep 0.2; printf '{}\\n'; sleep 0.5) | nc -U {} 2>/dev/null || true) | perl -pe \"s/\\e\\[[0-9;]*[a-zA-Z]//g\" | tr -d '\\r' | grep -v '(qemu)' | grep -v 'QEMU.*monitor' | grep -v '^$' || true",
        command.replace("'", "'\\''"),
        sock_path
    );

    send_cmd(&shell_cmd)
}

/// Map pctl mode to QEMU monitor command and execute
pub fn send_cmd_pctl(mode: &str, smac: &str) -> String {
    // Map mode to QEMU HMP command
    let (vm_name, qemu_cmd) = match mode {
        "stop" => (smac.to_string(), "quit".to_string()),
        "reset" => (smac.to_string(), "system_reset".to_string()),
        "powerdown" => (smac.to_string(), "system_powerdown".to_string()),
        "mountiso" => {
            // smac is "vmname isoname" for mountiso
            let parts: Vec<&str> = smac.splitn(2, ' ').collect();
            if parts.len() < 2 {
                return format!("Error: mountiso requires smac and isoname\n");
            }
            let vm = parts[0].to_string();
            let iso = parts[1];
            let iso_path = get_conf("iso_path");
            let cmd = format!("change ide0-cd0 {}/{}", iso_path, iso);
            (vm, cmd)
        }
        "unmountiso" => {
            (smac.to_string(), "eject ide0-cd0".to_string())
        }
        "livemigrate" => {
            let parts: Vec<&str> = smac.splitn(2, ' ').collect();
            if parts.len() < 2 {
                return format!("Error: livemigrate requires smac and target ip\n");
            }
            let vm = parts[0].to_string();
            let target = parts[1];
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
