use std::collections::HashMap;
use std::sync::OnceLock;

static CONFIG_CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();

fn defaults() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("qemu_path".into(), "/opt/homebrew/bin/qemu-system-x86_64".into());
    m.insert("ctl_bin_path".into(), "/opt/ctl/bin".into());
    m.insert("live_path".into(), "/tmp/vmcontrol/backups".into());
    m.insert("gzip_path".into(), "/usr/bin/gzip".into());
    m.insert("vs_up_script".into(), "vs-up.sh".into());
    m.insert("vs_down_script".into(), "vs-down.sh".into());
    m.insert("pctl_script".into(), "pctl.sh".into());
    m.insert("pctl_path".into(), "/tmp/vmcontrol".into());
    m.insert("disk_path".into(), "/tmp/vmcontrol/disks".into());
    m.insert("iso_path".into(), "/tmp/vmcontrol/iso".into());
    m.insert("qemu_img_path".into(), "/opt/homebrew/bin/qemu-img".into());
    m.insert("domain".into(), "localhost".into());
    // New config keys
    m.insert("db_path".into(), "/tmp/vmcontrol/vmcontrol.db".into());
    m.insert("mds_config_path".into(), "/tmp/vmcontrol/mds.json".into());
    m.insert("static_path".into(), "./static".into());
    m.insert("qemu_accel".into(), "hvf:tcg".into());
    m.insert("qemu_machine".into(), "pc".into());
    m.insert("qemu_aarch64_path".into(), "/opt/homebrew/bin/qemu-system-aarch64".into());
    m.insert("edk2_aarch64_bios".into(), "/opt/homebrew/share/qemu/edk2-aarch64-code.fd".into());
    m
}

fn load_config() -> HashMap<String, String> {
    // Check local config first, then system config
    let config_path = if std::path::Path::new("config.yaml").exists() {
        "config.yaml"
    } else {
        "/opt/ctl/bin/config.yaml"
    };
    let mut map = defaults();
    if let Ok(content) = std::fs::read_to_string(config_path) {
        match serde_yaml::from_str::<HashMap<String, String>>(&content) {
            Ok(file_map) => {
                // Merge file config over defaults
                for (k, v) in file_map {
                    map.insert(k, v);
                }
            }
            Err(e) => {
                eprintln!("config: parse yaml error: {}", e);
            }
        }
    }
    map
}

pub fn get_conf(name: &str) -> String {
    let map = CONFIG_CACHE.get_or_init(load_config);
    map.get(name).cloned().unwrap_or_else(|| {
        eprintln!("config: key '{}' not found", name);
        String::new()
    })
}
