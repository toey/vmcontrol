use std::collections::HashMap;

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
    m
}

pub fn get_conf(name: &str) -> String {
    let config_path = r"C:\vmcontrol\bin\config.yaml";
    let map = match std::fs::read_to_string(config_path) {
        Ok(content) => {
            serde_yaml::from_str::<HashMap<String, String>>(&content).unwrap_or_else(|e| {
                eprintln!("get conf error: parse yaml: {}", e);
                defaults()
            })
        }
        Err(_) => defaults(),
    };
    map.get(name).cloned().unwrap_or_else(|| {
        eprintln!("get conf error: key '{}' not found", name);
        String::new()
    })
}
