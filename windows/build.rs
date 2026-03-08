// build.rs — runs on every `cargo build`
// Deletes the old .api_key file so the dev server starts without auth after rebuild.
// This makes the dev workflow smoother: build → run → generate new key from UI.

use std::path::Path;

fn main() {
    // Read pctl_path from config.yaml (same logic as src/config.rs)
    let pctl_path = read_pctl_path().unwrap_or_else(|| "/opt/ctl/data".into());
    let key_file = format!("{}/.api_key", pctl_path);

    if Path::new(&key_file).exists() {
        match std::fs::remove_file(&key_file) {
            Ok(()) => println!("cargo:warning=Deleted old API key: {}", key_file),
            Err(e) => println!("cargo:warning=Could not delete {}: {}", key_file, e),
        }
    }
}

fn read_pctl_path() -> Option<String> {
    // Check local config.yaml first, then system
    let config_path = if Path::new("config.yaml").exists() {
        "config.yaml"
    } else if Path::new("/opt/ctl/bin/config.yaml").exists() {
        "/opt/ctl/bin/config.yaml"
    } else {
        return None;
    };

    let content = std::fs::read_to_string(config_path).ok()?;
    // Simple YAML parsing — just find "pctl_path: <value>"
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("pctl_path:") {
            let val = val.trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}
