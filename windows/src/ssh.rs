use std::process::Command;

pub fn send_cmd(command: &str) -> Result<String, String> {
    let result = Command::new("cmd")
        .arg("/C")
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
