use std::collections::HashMap;
use std::sync::OnceLock;

static CONFIG_CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();

fn defaults() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("qemu_path".into(), r"C:\Program Files\qemu\qemu-system-x86_64.exe".into());
    m.insert("ctl_bin_path".into(), r"C:\vmcontrol\bin".into());
    m.insert("live_path".into(), r"C:\vmcontrol\backups".into());
    m.insert("gzip_path".into(), "gzip".into());
    m.insert("vs_up_script".into(), "vs-up.bat".into());
    m.insert("vs_down_script".into(), "vs-down.bat".into());
    m.insert("pctl_script".into(), "pctl.bat".into());
    m.insert("pctl_path".into(), r"C:\vmcontrol".into());
    m.insert("disk_path".into(), r"C:\vmcontrol\disks".into());
    m.insert("iso_path".into(), r"C:\vmcontrol\iso".into());
    m.insert("qemu_img_path".into(), r"C:\Program Files\qemu\qemu-img.exe".into());
    m.insert("websockify_path".into(), "websockify".into());
    m.insert("python_path".into(), "python3".into());
    m.insert("domain".into(), "localhost".into());
    // New config keys
    m.insert("db_path".into(), r"C:\vmcontrol\vmcontrol.db".into());
    m.insert("mds_config_path".into(), r"C:\vmcontrol\mds.json".into());
    m.insert("static_path".into(), "./static".into());
    m.insert("qemu_accel".into(), "whpx:tcg".into());
    m.insert("qemu_machine".into(), "pc".into());
    m
}

fn load_config() -> HashMap<String, String> {
    let config_path = r"C:\vmcontrol\bin\config.yaml";
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
