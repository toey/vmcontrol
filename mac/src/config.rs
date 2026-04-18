use std::collections::HashMap;
use std::sync::OnceLock;

static CONFIG_CACHE: OnceLock<HashMap<String, String>> = OnceLock::new();

fn defaults() -> HashMap<String, String> {
    let mut m = HashMap::new();
    #[cfg(windows)]
    {
        m.insert("qemu_path".into(), r"C:\Program Files\qemu\qemu-system-x86_64.exe".into());
        m.insert("qemu_img_path".into(), r"C:\Program Files\qemu\qemu-img.exe".into());
        m.insert("qemu_aarch64_path".into(), r"C:\Program Files\qemu\qemu-system-aarch64.exe".into());
        m.insert("edk2_aarch64_bios".into(), r"C:\Program Files\qemu\share\edk2-aarch64-code.fd".into());
        m.insert("edk2_aarch64_secure_code".into(), r"C:\Program Files\qemu\share\AAVMF_CODE.secboot.fd".into());
        m.insert("edk2_aarch64_secure_vars".into(), r"C:\Program Files\qemu\share\AAVMF_VARS.ms.fd".into());
        m.insert("edk2_x86_secure_code".into(), r"C:\Program Files\qemu\share\edk2-x86_64-secure-code.fd".into());
        m.insert("edk2_x86_vars".into(), r"C:\Program Files\qemu\share\edk2-i386-vars.fd".into());
        m.insert("ctl_bin_path".into(), r"C:\vmcontrol\bin".into());
        m.insert("pctl_path".into(), r"C:\vmcontrol".into());
        m.insert("disk_path".into(), r"C:\vmcontrol\disks".into());
        m.insert("iso_path".into(), r"C:\vmcontrol\iso".into());
        m.insert("live_path".into(), r"C:\vmcontrol\backups".into());
        m.insert("db_path".into(), r"C:\vmcontrol\vmcontrol.db".into());
        m.insert("mds_config_path".into(), r"C:\vmcontrol\mds.json".into());
        m.insert("gzip_path".into(), "gzip".into());
        m.insert("vs_up_script".into(), "vs-up.bat".into());
        m.insert("vs_down_script".into(), "vs-down.bat".into());
        m.insert("pctl_script".into(), "pctl.bat".into());
        m.insert("swtpm_path".into(), r"C:\msys64\mingw64\bin\swtpm.exe".into());
        m.insert("mkisofs_path".into(), r"C:\msys64\mingw64\bin\mkisofs.exe".into());
        m.insert("websockify_path".into(), r"C:\msys64\mingw64\bin\websockify.exe".into());
        m.insert("qemu_cpu_x86".into(), "qemu64".into());
        m.insert("qemu_accel".into(), "tcg,thread=multi".into());
        m.insert("vnc_bind_host".into(), "0.0.0.0".into());
    }
    #[cfg(not(windows))]
    {
        m.insert("qemu_path".into(), "/opt/homebrew/bin/qemu-system-x86_64".into());
        m.insert("qemu_img_path".into(), "/opt/homebrew/bin/qemu-img".into());
        m.insert("qemu_aarch64_path".into(), "/opt/homebrew/bin/qemu-system-aarch64".into());
        m.insert("edk2_aarch64_bios".into(), "/opt/homebrew/share/qemu/edk2-aarch64-code.fd".into());
        m.insert("edk2_aarch64_secure_code".into(), "/opt/homebrew/share/qemu/AAVMF_CODE.secboot.fd".into());
        m.insert("edk2_aarch64_secure_vars".into(), "/opt/homebrew/share/qemu/AAVMF_VARS.ms.fd".into());
        m.insert("ctl_bin_path".into(), "/opt/ctl/bin".into());
        m.insert("pctl_path".into(), "/opt/ctl/data".into());
        m.insert("disk_path".into(), "/opt/ctl/data/disks".into());
        m.insert("iso_path".into(), "/opt/ctl/data/iso".into());
        m.insert("live_path".into(), "/opt/ctl/data/backups".into());
        m.insert("db_path".into(), "/opt/ctl/data/vmcontrol.db".into());
        m.insert("mds_config_path".into(), "/opt/ctl/data/mds.json".into());
        m.insert("gzip_path".into(), "/usr/bin/gzip".into());
        m.insert("vs_up_script".into(), "vs-up.sh".into());
        m.insert("vs_down_script".into(), "vs-down.sh".into());
        m.insert("pctl_script".into(), "pctl.sh".into());
        m.insert("ovs_vsctl_path".into(), "/opt/homebrew/bin/ovs-vsctl".into());
        m.insert("bridge_sudo".into(), "true".into());
        m.insert("bridge_sudo_path".into(), "/usr/bin/sudo".into());
        m.insert("qemu_nbd_path".into(), "/usr/bin/qemu-nbd".into());
        m.insert("disk_mount_base".into(), "/tmp/vmcontrol-mnt".into());
        m.insert("qemu_accel".into(), "hvf:tcg".into());
        m.insert("qemu_cpu_x86".into(), "Haswell-v4".into());
    }
    m.insert("domain".into(), "localhost".into());
    m.insert("static_path".into(), "./static".into());
    m.insert("qemu_machine".into(), "pc".into());
    m.insert("internal_mcast_port".into(), "11111".into());
    m
}

fn load_config() -> HashMap<String, String> {
    // Search order: cwd, then install.bat / installer layout per-platform.
    // First existing file wins.
    let candidates: Vec<&str> = {
        #[cfg(windows)]
        { vec!["config.yaml", r"C:\vmcontrol\bin\config.yaml"] }
        #[cfg(not(windows))]
        { vec!["config.yaml", "/opt/ctl/bin/config.yaml"] }
    };
    let config_path = candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .copied();
    let mut map = defaults();
    if let Some(path) = config_path {
        match std::fs::read_to_string(path) {
            Ok(content) => match serde_yaml::from_str::<HashMap<String, String>>(&content) {
                Ok(file_map) => {
                    eprintln!("config: loaded from {}", path);
                    for (k, v) in file_map {
                        map.insert(k, v);
                    }
                }
                Err(e) => eprintln!("config: parse yaml error in {}: {}", path, e),
            },
            Err(e) => eprintln!("config: read error {}: {}", path, e),
        }
    } else {
        eprintln!(
            "config: no config.yaml found in {:?} -- using built-in defaults",
            candidates
        );
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

/// Get config value with explicit fallback default
pub fn get_conf_or(name: &str, default: &str) -> String {
    let map = CONFIG_CACHE.get_or_init(load_config);
    map.get(name).cloned().unwrap_or_else(|| default.to_string())
}
