use crate::config::get_conf;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Send a QGA command and get the JSON response
pub fn qga_command(
    smac: &str,
    command: &str,
    args: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let pctl_path = get_conf("pctl_path");
    let sock_path = format!("{}/{}_qga", pctl_path, smac);

    let mut stream = UnixStream::connect(&sock_path)
        .map_err(|e| format!("Guest agent socket connect failed ({}): {}", sock_path, e))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("Set timeout error: {}", e))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| format!("Set write timeout error: {}", e))?;

    // QGA sync: send guest-sync-delimited to establish message boundary
    let sync_id: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let sync_cmd = serde_json::json!({
        "execute": "guest-sync-delimited",
        "arguments": { "id": sync_id }
    });
    // Send 0xFF delimiter + sync command
    stream
        .write_all(&[0xFF])
        .map_err(|e| format!("Write error: {}", e))?;
    stream
        .write_all(format!("{}\n", sync_cmd).as_bytes())
        .map_err(|e| format!("Write error: {}", e))?;

    // Read sync response (may contain leading 0xFF)
    std::thread::sleep(Duration::from_millis(300));
    let mut buf = vec![0u8; 4096];
    match stream.read(&mut buf) {
        Ok(n) => {
            // Validate sync response contains our sync_id
            let sync_resp = String::from_utf8_lossy(&buf[..n]);
            let clean: String = sync_resp
                .chars()
                .filter(|c| *c != '\u{00FF}' && !c.is_control())
                .collect();
            if !clean.is_empty() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&clean) {
                    if v.get("return").and_then(|r| r.as_u64()) != Some(sync_id) {
                        return Err(format!(
                            "Sync ID mismatch: expected {}, got {}",
                            sync_id, clean
                        ));
                    }
                }
            }
        }
        Err(ref e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            // Timeout waiting for sync — agent may be slow, continue anyway
        }
        Err(e) => return Err(format!("Sync read error: {}", e)),
    }

    // Send actual command
    let cmd = if let Some(a) = args {
        serde_json::json!({ "execute": command, "arguments": a })
    } else {
        serde_json::json!({ "execute": command })
    };
    stream
        .write_all(format!("{}\n", cmd).as_bytes())
        .map_err(|e| format!("Write error: {}", e))?;

    // Read response with retry loop
    std::thread::sleep(Duration::from_millis(200));
    let mut response = String::new();
    let start = std::time::Instant::now();
    let max_wait = Duration::from_secs(10);
    loop {
        if start.elapsed() > max_wait {
            return Err(format!(
                "QGA response timeout after {:?} for command '{}'",
                max_wait, command
            ));
        }
        let mut chunk = [0u8; 8192];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let s = String::from_utf8_lossy(&chunk[..n]);
                response.push_str(&s);
                if response.contains("\"return\"") || response.contains("\"error\"") {
                    break;
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                if response.contains("\"return\"") || response.contains("\"error\"") {
                    break;
                }
                // No data yet, sleep briefly and retry
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(format!("Read error: {}", e)),
        }
    }

    // Strip leading 0xFF bytes and parse JSON — find last complete JSON object
    let clean: String = response
        .chars()
        .filter(|c| *c != '\u{00FF}' && (!c.is_control() || *c == '\n'))
        .collect();

    // Check for error response first
    for line in clean.lines().rev() {
        let trimmed = line.trim();
        if trimmed.starts_with('{') {
            let parsed: serde_json::Value = serde_json::from_str(trimmed)
                .map_err(|e| format!("QGA JSON parse error: {} (raw: {})", e, trimmed))?;
            // Check if response is an error
            if let Some(err) = parsed.get("error") {
                let class = err
                    .get("class")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown");
                let desc = err
                    .get("desc")
                    .and_then(|v| v.as_str())
                    .unwrap_or("no description");
                return Err(format!("QGA error ({}): {}", class, desc));
            }
            return Ok(parsed);
        }
    }
    Err(format!("No valid JSON in QGA response: {}", clean))
}

/// Check if guest agent is available (guest-ping)
pub fn guest_ping(smac: &str) -> bool {
    qga_command(smac, "guest-ping", None).is_ok()
}

/// Write a file to the guest filesystem via QGA
/// Sends data in 1MB base64 chunks
pub fn guest_file_write(smac: &str, guest_path: &str, data: &[u8]) -> Result<(), String> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;

    // Open file in guest
    let open_result = qga_command(
        smac,
        "guest-file-open",
        Some(serde_json::json!({
            "path": guest_path,
            "mode": "wb"
        })),
    )?;

    let handle = open_result
        .get("return")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("guest-file-open failed: {:?}", open_result))?;

    // Write in chunks (1MB raw = ~1.33MB base64)
    const CHUNK_SIZE: usize = 1024 * 1024;
    let mut offset = 0;
    let mut write_err: Option<String> = None;

    while offset < data.len() {
        let end = std::cmp::min(offset + CHUNK_SIZE, data.len());
        let chunk_b64 = b64.encode(&data[offset..end]);

        let write_result = qga_command(
            smac,
            "guest-file-write",
            Some(serde_json::json!({
                "handle": handle,
                "buf-b64": chunk_b64
            })),
        );

        match write_result {
            Ok(resp) => {
                // Verify bytes written match
                let written = resp
                    .get("return")
                    .and_then(|r| r.get("count"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0);
                let expected = (end - offset) as u64;
                if written != expected {
                    write_err = Some(format!(
                        "Partial write: expected {} bytes, wrote {} at offset {}",
                        expected, written, offset
                    ));
                    break;
                }
            }
            Err(e) => {
                write_err = Some(format!(
                    "guest-file-write failed at offset {}: {}",
                    offset, e
                ));
                break;
            }
        }

        offset = end;
    }

    // Always close the file handle
    let _ = qga_command(
        smac,
        "guest-file-close",
        Some(serde_json::json!({"handle": handle})),
    );

    if let Some(e) = write_err {
        return Err(e);
    }

    Ok(())
}
