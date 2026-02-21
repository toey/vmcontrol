use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};

use crate::models::ApiResponse;

const MDS_CONFIG_PATH: &str = "/tmp/vmcontrol/mds.json";

// ──────────────────────────────────────────
// MDS Config Model
// ──────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MdsConfig {
    pub instance_id: String,
    pub ami_id: String,
    pub hostname_prefix: String,
    pub public_ipv4: String,
    pub local_ipv4: String,
    pub ssh_pubkey: String,
    pub root_password: String,
    pub userdata_extra: String,
    pub default_mac: String,
    pub kea_socket_path: String,
}

impl Default for MdsConfig {
    fn default() -> Self {
        MdsConfig {
            instance_id: "i-0000000000000001".into(),
            ami_id: "ami-00000001".into(),
            hostname_prefix: "vm".into(),
            public_ipv4: "10.0.0.1".into(),
            local_ipv4: "10.0.0.1".into(),
            ssh_pubkey: "".into(),
            root_password: "changeme".into(),
            userdata_extra: "".into(),
            default_mac: "52:54:00:00:00:01".into(),
            kea_socket_path: "".into(),
        }
    }
}

pub fn load_mds_config() -> MdsConfig {
    match std::fs::read_to_string(MDS_CONFIG_PATH) {
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
    let _ = std::fs::create_dir_all("/tmp/vmcontrol");
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("JSON serialize error: {}", e))?;
    std::fs::write(MDS_CONFIG_PATH, json)
        .map_err(|e| format!("File write error: {}", e))
}

// ──────────────────────────────────────────
// Cloud-init userdata generation
// ──────────────────────────────────────────

fn generate_userdata(config: &MdsConfig) -> String {
    let mut ud = String::from("#cloud-config\n");
    ud.push_str("ssh_pwauth: true\n");
    ud.push_str("users:\n");
    ud.push_str("  - name: root\n");
    ud.push_str("    primary_group: root\n");
    ud.push_str("    groups: root\n");
    ud.push_str("    lock_passwd: false\n");
    ud.push_str("    shell: /bin/bash\n");
    ud.push_str("resize_rootfs: True\n");
    ud.push_str("chpasswd:\n");
    ud.push_str("  list: |\n");
    ud.push_str(&format!("    root:{}\n", config.root_password));
    ud.push_str("  expire: False\n");

    if !config.ssh_pubkey.is_empty() {
        ud.push_str("ssh_authorized_keys:\n");
        ud.push_str(&format!("  - {}\n", config.ssh_pubkey));
    }

    ud.push_str("datasource:\n");
    ud.push_str("  Ec2:\n");
    ud.push_str("    strict_id: false\n");
    ud.push_str("    max_wait: 60\n");
    ud.push_str("    timeout: 30\n");
    ud.push_str("warnings:\n");
    ud.push_str("  dsid_missing_source: off\n");

    if !config.userdata_extra.is_empty() {
        ud.push_str(&config.userdata_extra);
        if !config.userdata_extra.ends_with('\n') {
            ud.push('\n');
        }
    }

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
        .body(generate_userdata(&config))
}

async fn metadata_index() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/plain")
        .body("ami-id\nhostname\nlocal-hostname\npublic-hostname\nnetwork/\ninstance-id\nlocal-ipv4\npublic-ipv4\npublic-keys/")
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

async fn public_ipv4_handler() -> HttpResponse {
    let config = load_mds_config();
    HttpResponse::Ok()
        .content_type("text/plain")
        .body(config.public_ipv4)
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
        .route("/2009-04-04/meta-data/public-ipv4", web::get().to(public_ipv4_handler))
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
