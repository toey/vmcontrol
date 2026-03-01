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

/// Spawn a long-running process with stderr redirected to a log file.
/// Returns the Child handle so the caller can check if the process is still alive.
pub fn spawn_cmd_with_log(program: &str, args: &[&str], log_path: &str) -> Result<std::process::Child, String> {
    // Ensure the log directory exists
    if let Some(parent) = std::path::Path::new(log_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let log_file = std::fs::File::create(log_path)
        .map_err(|e| format!("unable to create log file {}: {}", log_path, e))?;

    let log_file2 = log_file.try_clone()
        .map_err(|e| format!("unable to clone log file handle: {}", e))?;

    let child = Command::new(program)
        .args(args)
        .stdout(log_file2)
        .stderr(log_file)
        .spawn()
        .map_err(|e| format!("unable to start {}: {}", program, e))?;

    // Spawn background thread to monitor process exit and log the result
    let log_path_owned = log_path.to_string();
    let pid = child.id();
    std::thread::spawn(move || {
        // Wait a bit then check if process still exists
        std::thread::sleep(std::time::Duration::from_secs(10));
        // Check if process is still alive using kill -0
        let status = Command::new("kill")
            .args(&["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if let Ok(s) = status {
            if !s.success() {
                // Process died - append info to log
                use std::io::Write;
                if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log_path_owned) {
                    let _ = writeln!(f, "\n[MONITOR] Process PID {} exited within 10 seconds", pid);
                }
            }
        }
    });

    Ok(child)
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
