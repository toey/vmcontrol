use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};

use crate::config::get_conf;
use crate::models::ApiResponse;

// ──────────────────────────────────────────
// MDS Config Model
// ──────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct MdsConfig {
    pub instance_id: String,
    pub ami_id: String,
    pub hostname_prefix: String,
    pub local_ipv4: String,
    pub internal_ip: String,
    pub vlan: String,
    pub ssh_pubkey: String,
    pub root_password: String,
    pub userdata_extra: String,
    pub default_mac: String,
    pub kea_socket_path: String,
    // Cloud-init options
    pub timezone: String,
    pub locale: String,
    pub extra_packages: String,       // comma-separated: "curl,htop,vim"
    pub dns_nameservers: String,      // comma-separated: "8.8.8.8,1.1.1.1"
    pub disable_root_ssh: bool,
    pub growpart: bool,
    pub ntp_servers: String,          // comma-separated: "pool.ntp.org"
    pub swap_size_mb: u32,            // 0 = disabled
    pub phone_home_url: String,
    pub power_state: String,          // "" | "reboot" | "poweroff"
    pub extra_runcmd: String,         // newline-separated commands
    pub write_files: String,          // JSON array: [{"path":"/etc/foo","content":"bar"}]
}

impl Default for MdsConfig {
    fn default() -> Self {
        MdsConfig {
            instance_id: "i-0000000000000001".into(),
            ami_id: "ami-00000001".into(),
            hostname_prefix: "vm".into(),
            local_ipv4: "10.0.0.1".into(),
            internal_ip: "".into(),
            vlan: "0".into(),
            ssh_pubkey: "".into(),
            root_password: String::new(),  // Empty = no password set; generate_userdata will use random
            userdata_extra: "".into(),
            default_mac: "52:54:00:00:00:01".into(),
            kea_socket_path: "".into(),
            timezone: "".into(),
            locale: "".into(),
            extra_packages: "".into(),
            dns_nameservers: "".into(),
            disable_root_ssh: false,
            growpart: true,
            ntp_servers: "".into(),
            swap_size_mb: 0,
            phone_home_url: "".into(),
            power_state: "".into(),
            extra_runcmd: "".into(),
            write_files: "".into(),
        }
    }
}

pub fn load_mds_config() -> MdsConfig {
    let mds_config_path = get_conf("mds_config_path");
    match std::fs::read_to_string(&mds_config_path) {
        Ok(content) => {
            serde_json::from_str(&content).unwrap_or_else(|e| {
                eprintln!("MDS config parse error: {}", e);
                MdsConfig::default()
            })
        }
        Err(_) => MdsConfig::default(),
    }
}

pub fn save_mds_config(config: &MdsConfig) -> Result<(), String> {
    let mds_config_path = get_conf("mds_config_path");
    if let Some(parent) = std::path::Path::new(&mds_config_path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("JSON serialize error: {}", e))?;
    std::fs::write(&mds_config_path, json)
        .map_err(|e| format!("File write error: {}", e))
}

// ──────────────────────────────────────────
// Cloud-init userdata generation
// ──────────────────────────────────────────

/// Common cloud-init userdata base (shared between Ec2 and NoCloud modes)
fn generate_userdata_base(config: &MdsConfig, vmctl_password: &str) -> String {
    let mut ud = String::from("#cloud-config\n");
    ud.push_str("ssh_pwauth: true\n");
    ud.push_str("ssh_deletekeys: false\n");
    ud.push_str("users:\n");
    ud.push_str("  - default\n");
    ud.push_str("  - name: root\n");
    ud.push_str("    primary_group: root\n");
    ud.push_str("    groups: root\n");
    ud.push_str("    lock_passwd: false\n");
    ud.push_str("    shell: /bin/bash\n");
    ud.push_str("  - name: vmctl\n");
    ud.push_str("    groups: sudo\n");
    ud.push_str("    shell: /bin/bash\n");
    ud.push_str("    lock_passwd: false\n");
    ud.push_str("    sudo: ALL=(ALL) NOPASSWD:ALL\n");
    ud.push_str("resize_rootfs: True\n");
    ud.push_str("package_update: true\n");

    // ── packages ──
    ud.push_str("packages:\n");
    ud.push_str("  - qemu-guest-agent\n");
    if !config.extra_packages.is_empty() {
        for pkg in config.extra_packages.split(',') {
            let pkg = pkg.trim();
            if !pkg.is_empty() {
                ud.push_str(&format!("  - {}\n", pkg));
            }
        }
    }

    // ── runcmd ──
    ud.push_str("runcmd:\n");
    ud.push_str("  - systemctl enable --now qemu-guest-agent\n");
    if !config.extra_runcmd.is_empty() {
        for cmd in config.extra_runcmd.lines() {
            let cmd = cmd.trim();
            if !cmd.is_empty() {
                ud.push_str(&format!("  - {}\n", cmd));
            }
        }
    }
    // Sanitize root_password: strip newlines and YAML-dangerous chars
    // If root_password is empty, generate a random one for security
    let root_pw = if config.root_password.is_empty() {
        crate::operations::generate_random_password(16)
    } else {
        config.root_password.clone()
    };
    let safe_password = root_pw
        .replace('\n', "")
        .replace('\r', "")
        .replace(':', "");
    let safe_vmctl_pw = vmctl_password
        .replace('\n', "")
        .replace('\r', "")
        .replace(':', "");
    ud.push_str("chpasswd:\n");
    ud.push_str("  list: |\n");
    ud.push_str(&format!("    root:{}\n", safe_password));
    ud.push_str(&format!("    vmctl:{}\n", safe_vmctl_pw));
    ud.push_str("  expire: False\n");

    if !config.ssh_pubkey.is_empty() {
        // Sanitize ssh_pubkey: must be single line
        let safe_key = config.ssh_pubkey
            .lines()
            .next()
            .unwrap_or("")
            .trim();
        if !safe_key.is_empty() {
            ud.push_str("ssh_authorized_keys:\n");
            ud.push_str(&format!("  - {}\n", safe_key));
        }
    }

    // ── disable_root_ssh ──
    // Cloud-init default is disable_root: true which prepends
    // no-port-forwarding,command="echo Please login as..." to root's authorized_keys.
    // We must explicitly set false to allow normal root SSH access.
    if config.disable_root_ssh {
        ud.push_str("disable_root: true\n");
    } else {
        ud.push_str("disable_root: false\n");
    }

    // ── timezone ──
    if !config.timezone.is_empty() {
        ud.push_str(&format!("timezone: {}\n", config.timezone.trim()));
    }

    // ── locale ──
    if !config.locale.is_empty() {
        ud.push_str(&format!("locale: {}\n", config.locale.trim()));
    }

    // ── growpart ──
    if config.growpart {
        ud.push_str("growpart:\n");
        ud.push_str("  mode: auto\n");
        ud.push_str("  devices: ['/']\n");
    }

    // ── dns_nameservers ──
    if !config.dns_nameservers.is_empty() {
        ud.push_str("manage_resolv_conf: true\n");
        ud.push_str("resolv_conf:\n");
        ud.push_str("  nameservers:\n");
        for ns in config.dns_nameservers.split(',') {
            let ns = ns.trim();
            if !ns.is_empty() {
                ud.push_str(&format!("    - {}\n", ns));
            }
        }
    }

    // ── ntp_servers ──
    if !config.ntp_servers.is_empty() {
        ud.push_str("ntp:\n");
        ud.push_str("  enabled: true\n");
        ud.push_str("  servers:\n");
        for srv in config.ntp_servers.split(',') {
            let srv = srv.trim();
            if !srv.is_empty() {
                ud.push_str(&format!("    - {}\n", srv));
            }
        }
    }

    // ── swap ──
    if config.swap_size_mb > 0 {
        let size_bytes = config.swap_size_mb as u64 * 1024 * 1024;
        ud.push_str("swap:\n");
        ud.push_str("  filename: /swap.img\n");
        ud.push_str(&format!("  size: {}\n", size_bytes));
        ud.push_str(&format!("  maxsize: {}\n", size_bytes));
    }

    // ── write_files ──
    if !config.write_files.is_empty() {
        if let Ok(files) = serde_json::from_str::<Vec<serde_json::Value>>(&config.write_files) {
            if !files.is_empty() {
                ud.push_str("write_files:\n");
                for f in &files {
                    if let Some(path) = f.get("path").and_then(|v| v.as_str()) {
                        ud.push_str(&format!("  - path: {}\n", path));
                        if let Some(content) = f.get("content").and_then(|v| v.as_str()) {
                            ud.push_str("    content: |\n");
                            for line in content.lines() {
                                ud.push_str(&format!("      {}\n", line));
                            }
                        }
                        if let Some(perms) = f.get("permissions").and_then(|v| v.as_str()) {
                            ud.push_str(&format!("    permissions: '{}'\n", perms));
                        }
                        if let Some(owner) = f.get("owner").and_then(|v| v.as_str()) {
                            ud.push_str(&format!("    owner: {}\n", owner));
                        }
                    }
                }
            }
        }
    }

    // ── phone_home ──
    if !config.phone_home_url.is_empty() {
        ud.push_str("phone_home:\n");
        ud.push_str(&format!("  url: {}\n", config.phone_home_url.trim()));
        ud.push_str("  tries: 3\n");
    }

    // ── power_state ──
    if !config.power_state.is_empty() {
        let mode = config.power_state.trim();
        if mode == "reboot" || mode == "poweroff" {
            ud.push_str("power_state:\n");
            ud.push_str(&format!("  mode: {}\n", mode));
            ud.push_str("  message: \"cloud-init completed\"\n");
            ud.push_str("  timeout: 30\n");
        }
    }

    ud
}

/// Append userdata_extra and ensure trailing newline
fn append_userdata_extra(ud: &mut String, config: &MdsConfig) {
    if !config.userdata_extra.is_empty() {
        ud.push_str(&config.userdata_extra);
        if !config.userdata_extra.ends_with('\n') {
            ud.push('\n');
        }
    }
}

pub fn generate_userdata(config: &MdsConfig, vmctl_password: &str) -> String {
    let mut ud = generate_userdata_base(config, vmctl_password);

    ud.push_str("datasource:\n");
    ud.push_str("  Ec2:\n");
    ud.push_str("    strict_id: false\n");
    ud.push_str("    max_wait: 60\n");
    ud.push_str("    timeout: 30\n");
    ud.push_str("warnings:\n");
    ud.push_str("  dsid_missing_source: off\n");

    append_userdata_extra(&mut ud, config);
    ud
}

/// Generate user-data for NoCloud seed ISO (no Ec2 datasource block)
/// NOTE: datasource_list is NOT set here — cloud-init determines datasource
/// BEFORE reading user-data. Use SMBIOS hint or /etc/cloud/cloud.cfg.d/ instead.
pub fn generate_userdata_nocloud(config: &MdsConfig, hostname: &str, vmctl_password: &str) -> String {
    let base = generate_userdata_base(config, vmctl_password);
    let mut ud = base;

    // Set hostname explicitly in user-data (some distros ignore meta-data local-hostname)
    ud.push_str(&format!("hostname: {}\n", hostname));
    ud.push_str(&format!("fqdn: {}\n", hostname));
    ud.push_str("preserve_hostname: false\n");
    ud.push_str("manage_etc_hosts: true\n");

    append_userdata_extra(&mut ud, config);
    ud
}

// ──────────────────────────────────────────
// Kea DHCP integration (optional)
// ──────────────────────────────────────────

fn get_mac_from_kea(client_ip: &str, socket_path: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    if socket_path.is_empty() {
        return None;
    }

    let message = format!(
        r#"{{"command":"lease4-get","arguments":{{"ip-address":"{}"}}}}"#,
        client_ip
    );

    let mut stream = UnixStream::connect(socket_path).ok()?;
    stream.write_all(message.as_bytes()).ok()?;
    stream.shutdown(std::net::Shutdown::Write).ok()?;

    let mut data = Vec::new();
    stream.read_to_end(&mut data).ok()?;

    let response: serde_json::Value = serde_json::from_slice(&data).ok()?;
    response
        .get("arguments")?
        .get("hw-address")?
        .as_str()
        .map(|s| s.to_string())
}

fn resolve_mac(client_ip: &str, config: &MdsConfig) -> String {
    get_mac_from_kea(client_ip, &config.kea_socket_path)
        .unwrap_or_else(|| config.default_mac.clone())
}

fn get_client_ip(req: &HttpRequest, config: &MdsConfig) -> String {
    req.peer_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| config.local_ipv4.clone())
}

// ──────────────────────────────────────────
// EC2-compatible Metadata Route Handlers
// ──────────────────────────────────────────

async fn userdata_handler() -> HttpResponse {
    let config = load_mds_config();
    HttpResponse::Ok()
        .content_type("text/plain; charset=UTF-8")
        .body(generate_userdata(&config, ""))
}

async fn metadata_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("ami-id\nhostname\nlocal-hostname\npublic-hostname\nnetwork/\ninstance-id\nlocal-ipv4\npublic-keys/")
}

async fn instance_id_handler() -> HttpResponse {
    let config = load_mds_config();
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(config.instance_id)
}

async fn ami_id_handler() -> HttpResponse {
    let config = load_mds_config();
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(config.ami_id)
}

async fn hostname_handler(req: HttpRequest) -> HttpResponse {
    let config = load_mds_config();
    let ip = get_client_ip(&req, &config);
    let parts: Vec<&str> = ip.split('.').collect();
    let last = parts.last().unwrap_or(&"0");
    let hostname = format!("{}-{}.local", config.hostname_prefix, last);
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(hostname)
}

async fn local_ipv4_handler(req: HttpRequest) -> HttpResponse {
    let config = load_mds_config();
    let ip = get_client_ip(&req, &config);
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(ip)
}

async fn pubkeys_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("0=my-public-key")
}

async fn pubkeys_0() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("openssh-key")
}

async fn pubkeys_openssh() -> HttpResponse {
    let config = load_mds_config();
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(config.ssh_pubkey)
}

async fn network_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("interfaces/")
}

async fn network_interfaces_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("macs/")
}

async fn macs_list_handler(req: HttpRequest) -> HttpResponse {
    let config = load_mds_config();
    let ip = get_client_ip(&req, &config);
    let mac = resolve_mac(&ip, &config);
    let parts: Vec<&str> = mac.split(':').collect();
    let last_octet = parts.last().unwrap_or(&"00");
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(format!("{}/\n52:54:c4:ca:f1:{}/", mac, last_octet))
}

async fn mac_info_handler() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("device-number\nlocal-ipv4s")
}

async fn mac_device_number(req: HttpRequest, path: web::Path<String>) -> HttpResponse {
    let config = load_mds_config();
    let ip = get_client_ip(&req, &config);
    let mac = resolve_mac(&ip, &config);
    let req_mac = path.into_inner();
    if req_mac == mac {
        HttpResponse::Ok().content_type("text/plain").body("0")
    } else {
        HttpResponse::Ok().content_type("text/plain").body("1")
    }
}

async fn mac_local_ipv4s(req: HttpRequest, path: web::Path<String>) -> HttpResponse {
    let config = load_mds_config();
    let ip = get_client_ip(&req, &config);
    let mac = resolve_mac(&ip, &config);
    let req_mac = path.into_inner();
    if req_mac == mac {
        HttpResponse::Ok().content_type("text/plain").body(ip)
    } else {
        HttpResponse::Ok().content_type("text/plain").body("1")
    }
}

// ──────────────────────────────────────────
// Admin API Handlers
// ──────────────────────────────────────────

async fn get_mds_config_handler() -> HttpResponse {
    let config = load_mds_config();
    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: "MDS config loaded".into(),
        output: Some(serde_json::to_string_pretty(&config).unwrap_or_default()),
    })
}

async fn save_mds_config_handler(body: web::Json<MdsConfig>) -> HttpResponse {
    match save_mds_config(&body.into_inner()) {
        Ok(()) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "MDS config saved".into(),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to save: {}", e),
            output: None,
        }),
    }
}

// ──────────────────────────────────────────
// Route Configuration
// ──────────────────────────────────────────

pub fn configure_mds_routes(cfg: &mut web::ServiceConfig) {
    cfg
        // Admin API
        .route("/api/mds/config", web::get().to(get_mds_config_handler))
        .route("/api/mds/config", web::post().to(save_mds_config_handler))
        // EC2-compatible metadata endpoints
        .route("/2009-04-04/user-data", web::get().to(userdata_handler))
        .route("/2009-04-04/meta-data/", web::get().to(metadata_index))
        .route("/2009-04-04/meta-data/instance-id", web::get().to(instance_id_handler))
        .route("/2009-04-04/meta-data/ami-id", web::get().to(ami_id_handler))
        .route("/2009-04-04/meta-data/hostname", web::get().to(hostname_handler))
        .route("/2009-04-04/meta-data/local-hostname", web::get().to(hostname_handler))
        .route("/2009-04-04/meta-data/public-hostname", web::get().to(hostname_handler))
        .route("/2009-04-04/meta-data/local-ipv4", web::get().to(local_ipv4_handler))
        .route("/2009-04-04/meta-data/public-keys/", web::get().to(pubkeys_index))
        .route("/2009-04-04/meta-data/public-keys/0", web::get().to(pubkeys_0))
        .route("/2009-04-04/meta-data/public-keys/0/openssh-key", web::get().to(pubkeys_openssh))
        .route("/2009-04-04/meta-data/network/", web::get().to(network_index))
        .route("/2009-04-04/meta-data/network/interfaces/", web::get().to(network_interfaces_index))
        .route("/2009-04-04/meta-data/network/interfaces/macs/", web::get().to(macs_list_handler))
        .route("/2009-04-04/meta-data/network/interfaces/macs/{mac}/", web::get().to(mac_info_handler))
        .route("/2009-04-04/meta-data/network/interfaces/macs/{mac}/device-number", web::get().to(mac_device_number))
        .route("/2009-04-04/meta-data/network/interfaces/macs/{mac}/local-ipv4s", web::get().to(mac_local_ipv4s));
}
