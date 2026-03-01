use std::process::{Command, Stdio};

/// Execute a shell command via sh -c (UNSAFE with user input — prefer run_cmd)
pub fn send_cmd(command: &str) -> Result<String, String> {
    let result = Command::new("sh")
        .arg("-c")
        .arg(command)
        .output()
        .map_err(|e| format!("unable to execute command: {}", e))?;

    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();

    if !result.status.success() {
        if stderr.is_empty() {
            return Err(format!("command failed with exit code: {}", result.status));
        }
        return Err(format!("command failed: {}", stderr));
    }

    let mut output = stdout;
    if !stderr.is_empty() {
        output.push_str(&stderr);
    }
    Ok(output)
}

/// Execute a command safely with explicit arguments (no shell injection)
pub fn run_cmd(program: &str, args: &[&str]) -> Result<String, String> {
    let result = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("unable to execute '{}': {}", program, e))?;

    let stdout = String::from_utf8_lossy(&result.stdout).to_string();
    let stderr = String::from_utf8_lossy(&result.stderr).to_string();

    if !result.status.success() {
        if stderr.is_empty() {
            return Err(format!("'{}' failed with exit code: {}", program, result.status));
        }
        return Err(format!("'{}' failed: {}", program, stderr));
    }

    let mut output = stdout;
    if !stderr.is_empty() {
        output.push_str(&stderr);
    }
    Ok(output)
}

/// Spawn a long-running process in the background with stderr logging.
/// Used for QEMU instead of -daemonize which has WebSocket VNC bugs.
/// Returns (pid, log_path) so caller can check logs on failure.
pub fn spawn_background(program: &str, args: &[&str]) -> Result<(u32, String), String> {
    let pctl_path = std::env::temp_dir().join("vmcontrol");
    let _ = std::fs::create_dir_all(pctl_path.join("logs"));
    let log_path = pctl_path.join(format!("logs/qemu_{}.log", std::process::id()));
    let log_file = std::fs::File::create(&log_path)
        .map_err(|e| format!("unable to create log file: {}", e))?;
    let log_file2 = log_file.try_clone()
        .map_err(|e| format!("unable to clone log handle: {}", e))?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(log_file2)
        .stderr(log_file)
        .spawn()
        .map_err(|e| format!("unable to spawn '{}': {}", program, e))?;
    // Brief wait then verify QEMU didn't crash immediately
    std::thread::sleep(std::time::Duration::from_secs(1));
    match child.try_wait() {
        Ok(Some(status)) => {
            let stderr = std::fs::read_to_string(&log_path).unwrap_or_default();
            let msg = if stderr.trim().is_empty() {
                format!("process exited immediately with {}", status)
            } else {
                format!("process exited with {}: {}", status, stderr.trim())
            };
            Err(msg)
        }
        Ok(None) => Ok((child.id(), log_path.to_string_lossy().to_string())),
        Err(e) => Ok((child.id(), format!("(could not check status: {})", e))),
    }
}

/// Validate that a string is safe for use as a VM name / identifier.
/// Only allows alphanumeric, dash, underscore, dot, colon.
pub fn sanitize_name(name: &str) -> Result<&str, String> {
    if name.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if name.len() > 255 {
        return Err("Name too long (max 255 chars)".into());
    }
    if name.contains("..") {
        return Err("Name cannot contain '..'".into());
    }
    for c in name.chars() {
        if !c.is_alphanumeric() && c != '-' && c != '_' && c != '.' && c != ':' {
            return Err(format!("Invalid character '{}' in name", c));
        }
    }
    Ok(name)
}

/// Validate a port number string, returns the parsed u16
pub fn validate_port(port_str: &str) -> Result<u16, String> {
    let port: u16 = port_str
        .parse()
        .map_err(|_| format!("Invalid port number: '{}'", port_str))?;
    if port < 1024 {
        return Err(format!("Port {} is below 1024 (reserved)", port));
    }
    Ok(port)
}

/// Validate an IP address (basic check)
pub fn validate_ip(ip: &str) -> Result<&str, String> {
    if ip.is_empty() {
        return Err("IP address cannot be empty".into());
    }
    let parts: Vec<&str> = ip.split('.').collect();
    if parts.len() != 4 {
        return Err(format!("Invalid IP address: '{}'", ip));
    }
    for part in &parts {
        part.parse::<u8>()
            .map_err(|_| format!("Invalid IP address octet: '{}'", part))?;
    }
    Ok(ip)
}
