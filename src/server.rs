use actix_files as fs;
use actix_web::{dev, middleware, web, App, HttpResponse, HttpServer};
use actix_web::web::Bytes;
use std::collections::HashMap;
use std::future::{ready, Future, Ready};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use crate::config::get_conf;
use crate::mds;
use crate::models::ApiResponse;
use crate::operations;

/// Shared API key state for runtime updates
pub type SharedApiKey = Arc<Mutex<String>>;

/// One-time VNC access token
#[derive(Clone)]
pub struct VncToken {
    smac: String,
    created: std::time::Instant,
}

/// Shared VNC token store
pub type VncTokenStore = Arc<Mutex<HashMap<String, VncToken>>>;

// ──────────────────────────────────────────
// API Key Authentication Middleware
// ──────────────────────────────────────────

/// Optional API key authentication.
/// Set env VMCONTROL_API_KEY to enable. If unset, all requests are allowed.
pub struct ApiKeyAuth(pub SharedApiKey);

impl<S, B> dev::Transform<S, dev::ServiceRequest> for ApiKeyAuth
where
    S: dev::Service<dev::ServiceRequest, Response = dev::ServiceResponse<B>, Error = actix_web::Error> + 'static,
    B: 'static,
{
    type Response = dev::ServiceResponse<B>;
    type Error = actix_web::Error;
    type InitError = ();
    type Transform = ApiKeyAuthMiddleware<S>;
    type Future = Ready<Result<Self::Transform, Self::InitError>>;

    fn new_transform(&self, service: S) -> Self::Future {
        ready(Ok(ApiKeyAuthMiddleware {
            service: Rc::new(service),
            api_key: self.0.clone(),
        }))
    }
}

pub struct ApiKeyAuthMiddleware<S> {
    service: Rc<S>,
    api_key: SharedApiKey,
}

impl<S, B> dev::Service<dev::ServiceRequest> for ApiKeyAuthMiddleware<S>
where
    S: dev::Service<dev::ServiceRequest, Response = dev::ServiceResponse<B>, Error = actix_web::Error> + 'static,
    B: 'static,
{
    type Response = dev::ServiceResponse<B>;
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>>>>;

    dev::forward_ready!(service);

    fn call(&self, req: dev::ServiceRequest) -> Self::Future {
        let svc = self.service.clone();
        let api_key_lock = self.api_key.clone();

        Box::pin(async move {
            let api_key = api_key_lock.lock().unwrap().clone();

            // No API key configured = no auth required
            if api_key.is_empty() {
                return svc.call(req).await;
            }

            // Skip auth for static files, EC2 metadata endpoints, and VNC token resolve
            let path = req.path().to_string();
            if !path.starts_with("/api/") {
                return svc.call(req).await;
            }
            if path.starts_with("/api/vnc/resolve/") {
                return svc.call(req).await;
            }

            // Check X-API-Key header
            let provided = req.headers()
                .get("X-API-Key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            if provided == api_key {
                svc.call(req).await
            } else {
                Err(actix_web::error::ErrorUnauthorized(
                    "Invalid or missing API key. Set X-API-Key header."
                ))
            }
        })
    }
}

// ──────────────────────────────────────────
// API Handlers
// ──────────────────────────────────────────

async fn handle_operation(
    body: web::Json<serde_json::Value>,
    op_name: &str,
    op_fn: fn(&str) -> Result<String, String>,
) -> HttpResponse {
    let json_str = body.to_string();
    let name = op_name.to_string();

    let result = web::block(move || op_fn(&json_str)).await;

    match result {
        Ok(Ok(output)) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("{} completed successfully", name),
            output: Some(output),
        }),
        Ok(Err(e)) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Internal error: {}", e),
            output: None,
        }),
    }
}

async fn start_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "start", operations::start).await
}

async fn stop_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "stop", operations::stop).await
}

async fn reset_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "reset", operations::reset).await
}

async fn powerdown_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "powerdown", operations::powerdown).await
}

async fn create_config_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "create-config", operations::create_config).await
}

async fn update_config_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "update-config", operations::update_config).await
}

async fn get_vm_handler(path: web::Path<String>) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }
    match crate::db::get_vm(&smac) {
        Ok(vm) => HttpResponse::Ok().json(vm),
        Err(e) => HttpResponse::NotFound().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

// Per-VM MDS config
async fn get_vm_mds_handler(path: web::Path<String>) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }
    match crate::db::get_vm(&smac) {
        Ok(vm) => {
            let config: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
            let mds = config.get("mds").cloned().unwrap_or_else(|| {
                // Return global defaults if VM has no MDS config
                let global = mds::load_mds_config();
                serde_json::to_value(&global).unwrap_or_default()
            });
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: "MDS config loaded".into(),
                output: Some(serde_json::to_string_pretty(&mds).unwrap_or_default()),
            })
        }
        Err(e) => HttpResponse::NotFound().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn save_vm_mds_handler(path: web::Path<String>, body: web::Json<serde_json::Value>) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }
    match crate::db::get_vm(&smac) {
        Ok(vm) => {
            let mut config: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
            let mut new_mds = body.into_inner();

            // Validate local_ipv4 format (if provided)
            let new_ip = new_mds.get("local_ipv4").and_then(|v| v.as_str()).unwrap_or("");
            if !new_ip.is_empty() {
                if let Err(e) = crate::ssh::validate_ip(new_ip) {
                    return HttpResponse::BadRequest().json(ApiResponse {
                        success: false,
                        message: format!("Invalid local_ipv4: {}", e),
                        output: None,
                    });
                }
                // Check IP uniqueness (exclude this VM)
                if let Err(e) = operations::validate_ip_unique(new_ip, Some(&smac)) {
                    return HttpResponse::BadRequest().json(ApiResponse {
                        success: false,
                        message: e,
                        output: None,
                    });
                }
            }

            // Validate internal_ip format and uniqueness (if provided)
            let new_internal = new_mds
                .get("internal_ip")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !new_internal.is_empty() {
                if let Err(e) = crate::ssh::validate_ip(new_internal) {
                    return HttpResponse::BadRequest().json(ApiResponse {
                        success: false,
                        message: format!("Invalid internal_ip: {}", e),
                        output: None,
                    });
                }
                if let Err(e) =
                    operations::validate_internal_ip_unique(new_internal, Some(&smac))
                {
                    return HttpResponse::BadRequest().json(ApiResponse {
                        success: false,
                        message: e,
                        output: None,
                    });
                }
            }

            // Validate root_password minimum length (if provided)
            let new_pw = new_mds.get("root_password").and_then(|v| v.as_str()).unwrap_or("");
            if !new_pw.is_empty() && new_pw.len() < 6 {
                return HttpResponse::BadRequest().json(ApiResponse {
                    success: false,
                    message: "Root password must be at least 6 characters".into(),
                    output: None,
                });
            }

            // If root_password is empty, preserve existing password from DB
            if new_pw.is_empty() {
                if let Some(existing_pw) = config.get("mds")
                    .and_then(|m| m.get("root_password"))
                    .and_then(|v| v.as_str())
                {
                    if !existing_pw.is_empty() {
                        new_mds["root_password"] = serde_json::json!(existing_pw);
                    }
                }
            }

            config["mds"] = new_mds;
            let config_str = serde_json::to_string(&config).unwrap_or_default();
            match crate::db::update_vm(&smac, &config_str) {
                Ok(_) => HttpResponse::Ok().json(ApiResponse {
                    success: true,
                    message: format!("MDS config saved for VM '{}'", smac),
                    output: None,
                }),
                Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Failed to save: {}", e),
                    output: None,
                }),
            }
        }
        Err(e) => HttpResponse::NotFound().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn listimage_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "listimage", operations::listimage).await
}

async fn delete_vm_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "delete", operations::delete_vm).await
}

async fn mountiso_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "mountiso", operations::mountiso).await
}

async fn unmountiso_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "unmountiso", operations::unmountiso).await
}

async fn livemigrate_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "livemigrate", operations::livemigrate).await
}

async fn backup_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "backup", operations::backup).await
}

async fn vnc_start_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "vnc_start", operations::vnc_start).await
}

async fn vnc_stop_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "vnc_stop", operations::vnc_stop).await
}

/// Query QEMU block device info — returns per-drive mount status (cd0–cd3)
async fn blockinfo_handler(path: web::Path<String>) -> HttpResponse {
    let smac = path.into_inner();
    match crate::api_helpers::qemu_monitor_cmd(&smac, "info block") {
        Ok(raw) => {
            // Parse "info block" output to extract cd0–cd3 status
            // Format: "cd0 (#block123): /path/to/file.iso (raw, read-only)" or "cd0: [not inserted]"
            let mut drives = serde_json::Map::new();
            for i in 0..4 {
                let drive_id = format!("cd{}", i);
                let mut mounted = false;
                let mut file = String::new();
                for line in raw.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with(&format!("{} ", drive_id)) || trimmed.starts_with(&format!("{}:", drive_id)) {
                        if !trimmed.contains("[not inserted]") && !trimmed.contains("not inserted") {
                            mounted = true;
                            // Extract filename from path
                            if let Some(colon_pos) = trimmed.find(": ") {
                                let rest = &trimmed[colon_pos + 2..];
                                let path_str = rest.split(' ').next().unwrap_or("");
                                file = path_str.rsplit('/').next().unwrap_or(path_str).to_string();
                            }
                        }
                    }
                }
                drives.insert(drive_id, serde_json::json!({ "mounted": mounted, "file": file }));
            }
            HttpResponse::Ok().json(drives)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to query block info: {}", e),
            output: None,
        }),
    }
}

/// Generate a one-time VNC access token for a VM
async fn vnc_token_handler(
    body: web::Json<serde_json::Value>,
    store: web::Data<VncTokenStore>,
) -> HttpResponse {
    let smac = match body.get("smac").and_then(|v| v.as_str()) {
        Some(s) => s.to_string(),
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'smac' field".into(),
                output: None,
            });
        }
    };

    // Verify VM exists
    if let Err(e) = crate::db::get_vm(&smac) {
        return HttpResponse::NotFound().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        });
    }

    // Generate random token
    let token = {
        use std::io::Read;
        let mut bytes = [0u8; 24];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut bytes);
        } else {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            bytes[..16].copy_from_slice(&ts.to_le_bytes());
        }
        bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    };

    // Purge expired tokens (older than 5 minutes) and store new one
    {
        let mut map = store.lock().unwrap();
        let now = std::time::Instant::now();
        map.retain(|_, v| now.duration_since(v.created).as_secs() < 300);
        map.insert(token.clone(), VncToken {
            smac: smac.clone(),
            created: now,
        });
    }

    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "token": token,
    }))
}

/// Resolve and consume a one-time VNC token — returns VM info for connecting
async fn vnc_resolve_handler(
    path: web::Path<String>,
    store: web::Data<VncTokenStore>,
) -> HttpResponse {
    let token = path.into_inner();

    // Remove token (one-time use)
    let vnc_token = {
        let mut map = store.lock().unwrap();
        map.remove(&token)
    };

    let vnc_token = match vnc_token {
        Some(t) => {
            // Check expiry (5 minutes)
            if t.created.elapsed().as_secs() > 300 {
                return HttpResponse::Unauthorized().json(ApiResponse {
                    success: false,
                    message: "Token expired".into(),
                    output: None,
                });
            }
            t
        }
        None => {
            return HttpResponse::Unauthorized().json(ApiResponse {
                success: false,
                message: "Invalid or already used token".into(),
                output: None,
            });
        }
    };

    // Get VM info
    let vm = match crate::db::get_vm(&vnc_token.smac) {
        Ok(v) => v,
        Err(e) => {
            return HttpResponse::NotFound().json(ApiResponse {
                success: false,
                message: e,
                output: None,
            });
        }
    };

    let vnc_port = if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
        cfg.get("vnc_port").and_then(|v| v.as_u64()).unwrap_or(0)
    } else {
        0
    };

    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "smac": vnc_token.smac,
        "vnc_port": vnc_port,
        "status": vm.status,
    }))
}

/// List VMs — auto-backfill VNC ports in a single pass
async fn list_vms_handler() -> HttpResponse {
    match crate::db::list_vms() {
        Ok(mut vms) => {
            // Auto-backfill VNC ports for VMs that don't have one
            let mut used_ports: Vec<u16> = Vec::new();
            let mut need_port: Vec<usize> = Vec::new();
            for (i, vm) in vms.iter().enumerate() {
                if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                    if let Some(p) = cfg.get("vnc_port").and_then(|v| v.as_u64()) {
                        used_ports.push(p as u16);
                    } else {
                        need_port.push(i);
                    }
                } else {
                    need_port.push(i);
                }
            }

            // Assign missing VNC ports — update DB and patch in-memory records
            if !need_port.is_empty() {
                let mut next_port: u16 = operations::VNC_PORT_MIN;
                for &idx in &need_port {
                    while used_ports.contains(&next_port) && next_port < operations::VNC_PORT_MAX {
                        next_port += operations::VNC_PORT_STEP;
                    }
                    let mut cfg: serde_json::Value = serde_json::from_str(&vms[idx].config).unwrap_or_default();
                    cfg["vnc_port"] = serde_json::json!(next_port);
                    let new_config = serde_json::to_string(&cfg).unwrap_or_default();
                    let _ = crate::db::update_vm(&vms[idx].smac, &new_config);
                    vms[idx].config = new_config;
                    used_ports.push(next_port);
                    next_port += operations::VNC_PORT_STEP;
                }
            }

            HttpResponse::Ok().json(vms)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list VMs: {}", e),
            output: None,
        }),
    }
}

async fn list_isos_handler() -> HttpResponse {
    let iso_path = get_conf("iso_path");
    let _ = std::fs::create_dir_all(&iso_path);
    match std::fs::read_dir(&iso_path) {
        Ok(entries) => {
            let mut isos: Vec<serde_json::Value> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension() {
                    if ext == "iso" {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        isos.push(serde_json::json!({
                            "name": name,
                            "size": size,
                        }));
                    }
                }
            }
            isos.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            HttpResponse::Ok().json(isos)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list ISOs: {}", e),
            output: None,
        }),
    }
}

async fn upload_iso_handler(
    req: actix_web::HttpRequest,
    body: Bytes,
) -> HttpResponse {
    // Get filename from X-Filename header
    let filename = match req.headers().get("X-Filename") {
        Some(v) => v.to_str().unwrap_or("upload.iso").to_string(),
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing X-Filename header".into(),
                output: None,
            });
        }
    };

    // Sanitize filename - only allow alphanumeric, dash, underscore, dot
    let safe_name: String = filename
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if safe_name.is_empty() || !safe_name.ends_with(".iso") || safe_name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid filename (must end with .iso, no '..' allowed)".into(),
            output: None,
        });
    }

    let iso_path = get_conf("iso_path");
    let _ = std::fs::create_dir_all(&iso_path);
    let dest = format!("{}/{}", iso_path, safe_name);

    match std::fs::write(&dest, &body) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Uploaded {} ({} bytes)", safe_name, body.len()),
            output: Some(dest),
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to save ISO: {}", e),
            output: None,
        }),
    }
}

// List PCI devices currently bound to vfio-pci driver (Linux only)
async fn list_vfio_devices() -> HttpResponse {
    let mut devices: Vec<serde_json::Value> = Vec::new();

    #[cfg(target_os = "linux")]
    {
        let vfio_path = std::path::Path::new("/sys/bus/pci/drivers/vfio-pci");
        if vfio_path.exists() {
            if let Ok(entries) = std::fs::read_dir(vfio_path) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    // PCI addresses look like 0000:01:00.0
                    if !name.contains(':') { continue; }

                    // Read vendor and device IDs from sysfs
                    let base = format!("/sys/bus/pci/devices/{}", name);
                    let vendor = std::fs::read_to_string(format!("{}/vendor", base))
                        .unwrap_or_default().trim().to_string();
                    let device = std::fs::read_to_string(format!("{}/device", base))
                        .unwrap_or_default().trim().to_string();
                    let class = std::fs::read_to_string(format!("{}/class", base))
                        .unwrap_or_default().trim().to_string();

                    // Try to get device description via lspci
                    let desc = std::process::Command::new("lspci")
                        .args(&["-s", &name, "-mm"])
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .unwrap_or_default()
                        .trim().to_string();

                    devices.push(serde_json::json!({
                        "address": name,
                        "vendor": vendor,
                        "device": device,
                        "class": class,
                        "description": desc,
                    }));
                }
            }
        }
    }

    HttpResponse::Ok().json(devices)
}

async fn list_disks_handler() -> HttpResponse {
    let disk_path = get_conf("disk_path");

    // Auto-sync: register any .qcow2 files on disk that are not in DB
    if let Ok(entries) = std::fs::read_dir(&disk_path) {
        if let Ok(db_disks) = crate::db::list_disks() {
            let db_names: std::collections::HashSet<String> = db_disks.iter().map(|d| d.name.clone()).collect();
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".qcow2") {
                    let base = fname.trim_end_matches(".qcow2");
                    if !base.is_empty() && !db_names.contains(base) {
                        let _ = crate::db::insert_disk(base, "");
                    }
                }
            }
        }
    }

    match crate::db::list_disks() {
        Ok(disks) => {
            let result: Vec<serde_json::Value> = disks.iter().map(|d| {
                // Get actual file size from filesystem
                let file_path = format!("{}/{}.qcow2", disk_path, d.name);
                let file_size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
                serde_json::json!({
                    "name": d.name,
                    "filename": format!("{}.qcow2", d.name),
                    "disk_size": d.size,
                    "size": file_size,
                    "owner": d.owner,
                })
            }).collect();
            HttpResponse::Ok().json(result)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list disks: {}", e),
            output: None,
        }),
    }
}

async fn create_disk_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "create-disk", operations::create_disk).await
}

async fn resize_disk_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "resize-disk", operations::resize_disk).await
}

async fn delete_disk_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'name' field".into(),
                output: None,
            });
        }
    };

    // Sanitize
    if name.contains('/') || name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid disk name".into(),
            output: None,
        });
    }

    // Check if disk is assigned to any VM (via DB owner field)
    if let Ok(disks) = crate::db::list_disks() {
        for d in &disks {
            if d.name == name && !d.owner.is_empty() {
                return HttpResponse::BadRequest().json(ApiResponse {
                    success: false,
                    message: format!("Disk '{}' is assigned to VM '{}'. Remove it from the VM first.", name, d.owner),
                    output: None,
                });
            }
        }
    }

    let disk_path = get_conf("disk_path");
    let path = format!("{}/{}.qcow2", disk_path, name);

    // Delete file
    let _ = std::fs::remove_file(&path);
    // Delete from DB
    let _ = crate::db::delete_disk(name);

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: format!("Deleted disk '{}'", name),
        output: None,
    })
}

async fn clone_disk_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let source = match body.get("source").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'source' field".into(),
                output: None,
            });
        }
    };
    let new_name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'name' field".into(),
                output: None,
            });
        }
    };

    // Sanitize
    if source.contains('/') || source.contains("..") || new_name.contains('/') || new_name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid disk name".into(),
            output: None,
        });
    }
    if new_name.chars().any(|c| !c.is_alphanumeric() && c != '-' && c != '_' && c != '.') {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid name (alphanumeric, dash, underscore, dot only)".into(),
            output: None,
        });
    }

    let disk_path = get_conf("disk_path");
    let src_file = format!("{}/{}.qcow2", disk_path, source);
    let dst_file = format!("{}/{}.qcow2", disk_path, new_name);

    if !std::path::Path::new(&src_file).exists() {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Source disk '{}' not found", source),
            output: None,
        });
    }
    if let Err(e) = operations::check_disk_not_in_use(&source) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        });
    }
    if std::path::Path::new(&dst_file).exists() {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Disk '{}' already exists", new_name),
            output: None,
        });
    }

    // Copy file (blocking)
    let src = src_file.clone();
    let dst = dst_file.clone();
    let nn = new_name.clone();
    let result = web::block(move || {
        std::fs::copy(&src, &dst)
            .map_err(|e| format!("Copy failed: {}", e))?;
        // Get file size for DB
        let size = std::fs::metadata(&dst)
            .map(|m| {
                let mb = m.len() / 1024 / 1024;
                if mb >= 1024 { format!("{}G", mb / 1024) } else { format!("{}M", mb) }
            })
            .unwrap_or_else(|_| "0".into());
        crate::db::insert_disk(&nn, &size)
            .map_err(|e| format!("DB insert error: {}", e))?;
        Ok::<String, String>(format!("Cloned '{}' -> '{}'", src, dst))
    })
    .await;

    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Disk '{}' cloned to '{}'", source, new_name),
            output: Some(msg),
        }),
        Ok(Err(e)) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e.to_string(),
            output: None,
        }),
    }
}

async fn delete_iso_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'name' field".into(),
                output: None,
            });
        }
    };

    // Sanitize
    if name.contains('/') || name.contains("..") || !name.ends_with(".iso") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid filename".into(),
            output: None,
        });
    }

    // Check if ISO is mounted by any running VM
    if let Err(e) = operations::check_iso_not_mounted(name) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        });
    }

    let iso_path = get_conf("iso_path");
    let path = format!("{}/{}", iso_path, name);

    match std::fs::remove_file(&path) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Deleted {}", name),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to delete ISO: {}", e),
            output: None,
        }),
    }
}

async fn list_images_handler() -> HttpResponse {
    let disk_path = get_conf("disk_path");
    let _ = std::fs::create_dir_all(&disk_path);
    match std::fs::read_dir(&disk_path) {
        Ok(entries) => {
            let mut images: Vec<serde_json::Value> = Vec::new();
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if ["qcow2", "img", "raw", "vmdk"].contains(&ext_str.as_str()) {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        images.push(serde_json::json!({
                            "name": name,
                            "size": size,
                        }));
                    }
                }
            }
            images.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            HttpResponse::Ok().json(images)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list images: {}", e),
            output: None,
        }),
    }
}

/// Detect qemu-img input format from file extension
fn detect_image_format(filename: &str) -> Option<&'static str> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".vmdk") { Some("vmdk") }
    else if lower.ends_with(".vdi") { Some("vdi") }
    else if lower.ends_with(".vhdx") { Some("vhdx") }
    else if lower.ends_with(".raw") || lower.ends_with(".img") { Some("raw") }
    else if lower.ends_with(".qcow2") { Some("qcow2") }
    else { None }
}

async fn upload_image_handler(
    req: actix_web::HttpRequest,
    body: Bytes,
) -> HttpResponse {
    let filename = match req.headers().get("X-Filename") {
        Some(v) => v.to_str().unwrap_or("upload.qcow2").to_string(),
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing X-Filename header".into(),
                output: None,
            });
        }
    };

    let safe_name: String = filename
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if safe_name.is_empty() || safe_name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid filename".into(),
            output: None,
        });
    }

    let src_format = match detect_image_format(&safe_name) {
        Some(f) => f,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Unsupported format. Use: qcow2, vmdk, vdi, vhdx, raw, img".into(),
                output: None,
            });
        }
    };

    let disk_path = get_conf("disk_path");
    let _ = std::fs::create_dir_all(&disk_path);
    let upload_path = format!("{}/{}", disk_path, safe_name);

    // Save uploaded file
    if let Err(e) = std::fs::write(&upload_path, &body) {
        return HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to save file: {}", e),
            output: None,
        });
    }

    let file_size = body.len();

    // If already qcow2, register in DB and done
    if src_format == "qcow2" {
        let base = safe_name.trim_end_matches(".qcow2");
        let _ = crate::db::insert_disk(base, "");
        return HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Uploaded {} ({} bytes)", safe_name, file_size),
            output: Some(upload_path),
        });
    }

    // Convert to qcow2 using safe command execution
    let qcow2_name = safe_name.rsplit_once('.')
        .map(|(base, _)| format!("{}.qcow2", base))
        .unwrap_or_else(|| format!("{}.qcow2", safe_name));
    let qcow2_path = format!("{}/{}", disk_path, qcow2_name);
    let qemu_img = get_conf("qemu_img_path");
    let src_fmt = src_format.to_string();
    let up_path = upload_path.clone();
    let out_path = qcow2_path.clone();
    let out_name = qcow2_name.clone();

    let convert_result = web::block(move || {
        use std::process::Command;
        let output = Command::new(&qemu_img)
            .args(["convert", "-f", &src_fmt, "-O", "qcow2", &up_path, &out_path])
            .output()
            .map_err(|e| format!("Failed to run qemu-img: {}", e))?;
        if output.status.success() {
            // Remove original uploaded file after successful conversion
            let _ = std::fs::remove_file(&up_path);
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            // Cleanup on failure
            let _ = std::fs::remove_file(&up_path);
            let _ = std::fs::remove_file(&out_path);
            Err(format!("Conversion failed: {}", stderr))
        }
    }).await;

    match convert_result {
        Ok(Ok(out)) => {
            // Register converted qcow2 in DB
            let base = out_name.trim_end_matches(".qcow2");
            let _ = crate::db::insert_disk(base, "");
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: format!("Uploaded & converted {} -> {} ({} bytes)", safe_name, out_name, file_size),
                output: Some(out),
            })
        },
        Ok(Err(e)) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Internal error: {}", e),
            output: None,
        }),
    }
}

async fn delete_image_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'name' field".into(),
                output: None,
            });
        }
    };

    if name.contains('/') || name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid filename".into(),
            output: None,
        });
    }

    // Check if this image is a disk owned by a VM
    if let Ok(disks) = crate::db::list_disks() {
        let base_name = name.rsplit('.').skip(1).collect::<Vec<&str>>().into_iter().rev().collect::<Vec<&str>>().join(".");
        for d in &disks {
            if d.name == base_name && !d.owner.is_empty() {
                return HttpResponse::BadRequest().json(ApiResponse {
                    success: false,
                    message: format!("Image '{}' is assigned to VM '{}'. Remove it from the VM first.", name, d.owner),
                    output: None,
                });
            }
        }
    }

    let disk_path = get_conf("disk_path");
    let path = format!("{}/{}", disk_path, name);

    match std::fs::remove_file(&path) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Deleted {}", name),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to delete image: {}", e),
            output: None,
        }),
    }
}

/// Export a disk image — supports qcow2 (direct download) or convert to raw/vmdk/vdi/vhdx
async fn export_disk_handler(
    req: actix_web::HttpRequest,
    path: web::Path<String>,
) -> HttpResponse {
    let name = path.into_inner();

    // Sanitize
    if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid disk name".into(),
            output: None,
        });
    }

    let disk_path = get_conf("disk_path");
    let qcow2_file = format!("{}/{}.qcow2", disk_path, name);

    if !std::path::Path::new(&qcow2_file).exists() {
        return HttpResponse::NotFound().json(ApiResponse {
            success: false,
            message: format!("Disk '{}' not found", name),
            output: None,
        });
    }

    if let Err(e) = operations::check_disk_not_in_use(&name) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        });
    }

    // Check requested format from query param ?format=raw|vmdk|vdi|vhdx|qcow2
    let query = web::Query::<std::collections::HashMap<String, String>>::from_query(req.query_string())
        .unwrap_or_else(|_| web::Query(std::collections::HashMap::new()));
    let format = query.get("format").map(|s| s.as_str()).unwrap_or("qcow2");

    match format {
        "qcow2" => {
            // Direct download — stream the qcow2 file
            let download_name = format!("{}.qcow2", name);
            match actix_files::NamedFile::open_async(&qcow2_file).await {
                Ok(f) => f
                    .set_content_disposition(actix_web::http::header::ContentDisposition {
                        disposition: actix_web::http::header::DispositionType::Attachment,
                        parameters: vec![
                            actix_web::http::header::DispositionParam::Filename(download_name),
                        ],
                    })
                    .into_response(&req),
                Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Failed to open disk file: {}", e),
                    output: None,
                }),
            }
        }
        "raw" | "vmdk" | "vdi" | "vhdx" => {
            // Convert via qemu-img then stream
            let qemu_img = get_conf("qemu_img_path");
            let ext = format.to_string();
            let download_name = format!("{}.{}", name, ext);
            let tmp_file = format!("{}/{}_export_{}.{}", disk_path, name, std::process::id(), ext);
            let src = qcow2_file.clone();
            let dst = tmp_file.clone();
            let fmt = ext.clone();

            let convert = web::block(move || {
                use std::process::Command;
                let output = Command::new(&qemu_img)
                    .args(["convert", "-f", "qcow2", "-O", &fmt, &src, &dst])
                    .output()
                    .map_err(|e| format!("Failed to run qemu-img: {}", e))?;
                if output.status.success() {
                    Ok(())
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                    let _ = std::fs::remove_file(&dst);
                    Err(format!("Conversion failed: {}", stderr))
                }
            }).await;

            match convert {
                Ok(Ok(())) => {
                    match actix_files::NamedFile::open_async(&tmp_file).await {
                        Ok(f) => {
                            // Schedule cleanup after response — allow 10 min for large file downloads
                            let cleanup_path = tmp_file.clone();
                            actix_web::rt::spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                                let _ = std::fs::remove_file(&cleanup_path);
                            });
                            f.set_content_disposition(actix_web::http::header::ContentDisposition {
                                disposition: actix_web::http::header::DispositionType::Attachment,
                                parameters: vec![
                                    actix_web::http::header::DispositionParam::Filename(download_name),
                                ],
                            })
                            .into_response(&req)
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&tmp_file);
                            HttpResponse::InternalServerError().json(ApiResponse {
                                success: false,
                                message: format!("Failed to open converted file: {}", e),
                                output: None,
                            })
                        }
                    }
                }
                Ok(Err(e)) => HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: e,
                    output: None,
                }),
                Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Internal error: {}", e),
                    output: None,
                }),
            }
        }
        _ => HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Unsupported format '{}'. Use: qcow2, raw, vmdk, vdi, vhdx", format),
            output: None,
        }),
    }
}

async fn list_backups_handler() -> HttpResponse {
    let live_path = get_conf("live_path");
    let _ = std::fs::create_dir_all(&live_path);
    match std::fs::read_dir(&live_path) {
        Ok(entries) => {
            let mut backups: Vec<serde_json::Value> = Vec::new();
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".gz") {
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    // Parse VM name and timestamp from filename: vmname_YYYYMMDD_HHMMSS.gz
                    let base = fname.trim_end_matches(".gz");
                    let parts: Vec<&str> = base.rsplitn(3, '_').collect();
                    let (vm_name, datetime) = if parts.len() >= 3
                        && parts[1].len() >= 8
                        && parts[0].len() >= 6
                    {
                        // parts[0]=HHMMSS, parts[1]=YYYYMMDD, parts[2]=vmname
                        let dt = format!("{}-{}-{} {}:{}:{}",
                            &parts[1][0..4], &parts[1][4..6], &parts[1][6..8],
                            &parts[0][0..2], &parts[0][2..4], &parts[0][4..6]);
                        (parts[2].to_string(), dt)
                    } else {
                        // Old format or unrecognized: use whole base as name
                        (base.to_string(), String::new())
                    };
                    backups.push(serde_json::json!({
                        "filename": fname,
                        "vm_name": vm_name,
                        "datetime": datetime,
                        "size": size,
                    }));
                }
            }
            backups.sort_by(|a, b| b["filename"].as_str().cmp(&a["filename"].as_str()));
            HttpResponse::Ok().json(backups)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list backups: {}", e),
            output: None,
        }),
    }
}

async fn delete_backup_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let filename = match body.get("filename").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'filename' field".into(),
                output: None,
            });
        }
    };

    if filename.contains('/') || filename.contains("..") || !filename.ends_with(".gz") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid filename".into(),
            output: None,
        });
    }

    let live_path = get_conf("live_path");
    let path = format!("{}/{}", live_path, filename);

    match std::fs::remove_file(&path) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Deleted backup '{}'", filename),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to delete backup: {}", e),
            output: None,
        }),
    }
}

// ======== Group Management ========

async fn list_groups_handler() -> HttpResponse {
    match crate::db::list_groups() {
        Ok(groups) => HttpResponse::Ok().json(groups),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list groups: {}", e),
            output: None,
        }),
    }
}

async fn set_vm_group_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let smac = match body.get("smac").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'smac' field".into(),
                output: None,
            });
        }
    };
    let group_name = body.get("group_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match crate::db::set_vm_group(&smac, &group_name) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("VM '{}' group set to '{}'", smac, if group_name.is_empty() { "(ungrouped)" } else { &group_name }),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

// ======== Switch Management ========

async fn list_switches_handler() -> HttpResponse {
    match crate::db::list_switches() {
        Ok(switches) => HttpResponse::Ok().json(switches),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list switches: {}", e),
            output: None,
        }),
    }
}

async fn create_switch_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'name' field".into(),
                output: None,
            });
        }
    };
    // Sanitize: alphanumeric, dash, underscore only
    if !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Switch name must be alphanumeric, dash, or underscore only".into(),
            output: None,
        });
    }
    match crate::db::insert_switch(&name) {
        Ok(id) => {
            // Create OVS bridge
            let bridge_name = format!("vs-{}", name);
            let ovs = crate::config::get_conf("ovs_vsctl_path");
            if !ovs.is_empty() {
                let _ = crate::ssh::run_cmd(&ovs, &["--may-exist", "add-br", &bridge_name]);
            }
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: format!("Switch '{}' created (ID: {}, bridge: {})", name, id, bridge_name),
                output: Some(id.to_string()),
            })
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn delete_switch_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let id = match body.get("id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'id' field".into(),
                output: None,
            });
        }
    };
    // Delete OVS bridge before DB record
    if let Ok(sw) = crate::db::get_switch_by_id(id) {
        let bridge_name = format!("vs-{}", sw.name);
        let ovs = crate::config::get_conf("ovs_vsctl_path");
        if !ovs.is_empty() {
            let _ = crate::ssh::run_cmd(&ovs, &["--if-exists", "del-br", &bridge_name]);
        }
    }
    match crate::db::delete_switch(id) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Switch {} deleted", id),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn rename_switch_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let id = match body.get("id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'id' field".into(),
                output: None,
            });
        }
    };
    let new_name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'name' field".into(),
                output: None,
            });
        }
    };
    if !new_name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Switch name must be alphanumeric, dash, or underscore only".into(),
            output: None,
        });
    }
    match crate::db::rename_switch(id, &new_name) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Switch {} renamed to '{}'", id, new_name),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

// ======== Template Image Mappings ========

async fn list_template_images_handler() -> HttpResponse {
    match crate::db::list_template_images() {
        Ok(mappings) => {
            let map: serde_json::Map<String, serde_json::Value> = mappings
                .into_iter()
                .map(|(k, v)| (k, serde_json::json!(v)))
                .collect();
            HttpResponse::Ok().json(map)
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list template images: {}", e),
            output: None,
        }),
    }
}

async fn set_template_image_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let template_key = match body.get("template_key").and_then(|v| v.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'template_key' field".into(),
                output: None,
            });
        }
    };
    let disk_name = body.get("disk_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match crate::db::set_template_image(&template_key, &disk_name) {
        Ok(_) => {
            let msg = if disk_name.is_empty() {
                format!("Template '{}' image cleared", template_key)
            } else {
                format!("Template '{}' → {}", template_key, disk_name)
            };
            HttpResponse::Ok().json(ApiResponse {
                success: true,
                message: msg,
                output: None,
            })
        }
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

// ── OS Templates CRUD ──

async fn list_os_templates_handler() -> HttpResponse {
    match crate::db::list_os_templates() {
        Ok(templates) => HttpResponse::Ok().json(templates),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list os templates: {}", e),
            output: None,
        }),
    }
}

async fn create_os_template_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let key = match body.get("key").and_then(|v| v.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'key'".into(), output: None,
        }),
    };
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or(&key).to_string();
    let vcpus = body.get("vcpus").and_then(|v| v.as_str()).unwrap_or("2").to_string();
    let memory = body.get("memory").and_then(|v| v.as_str()).unwrap_or("2048").to_string();
    let is_windows = body.get("is_windows").and_then(|v| v.as_str()).unwrap_or("0").to_string();
    let arch = body.get("arch").and_then(|v| v.as_str()).unwrap_or("x86_64").to_string();
    let image = body.get("image").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match crate::db::create_os_template(&key, &name, &vcpus, &memory, &is_windows, &arch, &image) {
        Ok(id) => HttpResponse::Ok().json(serde_json::json!({
            "success": true, "message": format!("Template '{}' created", name), "id": id
        })),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: e, output: None,
        }),
    }
}

async fn update_os_template_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let id = match body.get("id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        None => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'id'".into(), output: None,
        }),
    };
    let key = match body.get("key").and_then(|v| v.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'key'".into(), output: None,
        }),
    };
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or(&key).to_string();
    let vcpus = body.get("vcpus").and_then(|v| v.as_str()).unwrap_or("2").to_string();
    let memory = body.get("memory").and_then(|v| v.as_str()).unwrap_or("2048").to_string();
    let is_windows = body.get("is_windows").and_then(|v| v.as_str()).unwrap_or("0").to_string();
    let arch = body.get("arch").and_then(|v| v.as_str()).unwrap_or("x86_64").to_string();
    let image = body.get("image").and_then(|v| v.as_str()).unwrap_or("").to_string();
    match crate::db::update_os_template(id, &key, &name, &vcpus, &memory, &is_windows, &arch, &image) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true, message: format!("Template '{}' updated", name), output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: e, output: None,
        }),
    }
}

async fn delete_os_template_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let id = match body.get("id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        None => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'id'".into(), output: None,
        }),
    };
    match crate::db::delete_os_template(id) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true, message: "Template deleted".into(), output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: e, output: None,
        }),
    }
}

// ======== SSH Key Management ========

async fn list_ssh_keys_handler() -> HttpResponse {
    match crate::db::list_ssh_keys() {
        Ok(keys) => HttpResponse::Ok().json(keys),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to list SSH keys: {}", e),
            output: None,
        }),
    }
}

async fn create_ssh_key_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'name' field".into(),
                output: None,
            });
        }
    };
    let pubkey = match body.get("pubkey").and_then(|v| v.as_str()) {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'pubkey' field".into(),
                output: None,
            });
        }
    };
    match crate::db::insert_ssh_key(&name, &pubkey) {
        Ok(id) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("SSH key '{}' saved (ID: {})", name, id),
            output: Some(id.to_string()),
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn delete_ssh_key_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let id = match body.get("id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing 'id' field".into(),
                output: None,
            });
        }
    };
    match crate::db::delete_ssh_key(id) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("SSH key {} deleted", id),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

// ======== DHCP Lease Management ========

async fn list_dhcp_handler() -> HttpResponse {
    // Collect DHCP leases from DB
    let leases = crate::db::list_dhcp_leases().unwrap_or_default();

    // Also collect VM MAC/IP info from MDS configs for a merged view
    let vms = crate::db::list_vms().unwrap_or_default();
    let mut vm_entries: Vec<serde_json::Value> = Vec::new();
    for vm in &vms {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
            let mds = cfg.get("mds");
            let ip = mds.and_then(|m| m.get("local_ipv4")).and_then(|v| v.as_str()).unwrap_or("");
            let hostname = mds.and_then(|m| m.get("hostname_prefix")).and_then(|v| v.as_str()).unwrap_or("");
            if let Some(adapters) = cfg.get("network_adapters").and_then(|v| v.as_array()) {
                for adapter in adapters {
                    let mac = adapter.get("mac").and_then(|v| v.as_str()).unwrap_or("");
                    let vlan = adapter.get("vlan").and_then(|v| v.as_str()).unwrap_or("0");
                    if !mac.is_empty() {
                        vm_entries.push(serde_json::json!({
                            "mac": mac,
                            "ip": ip,
                            "hostname": hostname,
                            "vm_name": vm.smac,
                            "vlan": vlan,
                            "source": "vm",
                        }));
                    }
                }
            }
        }
    }

    // Merge: DB leases + VM-derived entries
    let lease_json: Vec<serde_json::Value> = leases.iter().map(|l| {
        serde_json::json!({
            "mac": l.mac,
            "ip": l.ip,
            "hostname": l.hostname,
            "vm_name": l.vm_name,
            "vlan": "",
            "source": "static",
            "created_at": l.created_at,
        })
    }).collect();

    // Combine: static leases first, then VM-derived (skip duplicates by MAC)
    let mut seen_macs: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result: Vec<serde_json::Value> = Vec::new();
    for entry in &lease_json {
        let mac = entry["mac"].as_str().unwrap_or("").to_string();
        seen_macs.insert(mac);
        result.push(entry.clone());
    }
    for entry in &vm_entries {
        let mac = entry["mac"].as_str().unwrap_or("").to_string();
        if !seen_macs.contains(&mac) {
            seen_macs.insert(mac);
            result.push(entry.clone());
        }
    }

    HttpResponse::Ok().json(result)
}

async fn add_dhcp_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let mac = match body.get("mac").and_then(|v| v.as_str()) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'mac' field".into(),
                output: None,
            });
        }
    };
    let ip = body.get("ip").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let hostname = body.get("hostname").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let vm_name = body.get("vm_name").and_then(|v| v.as_str()).unwrap_or("").to_string();

    match crate::db::upsert_dhcp_lease(&mac, &ip, &hostname, &vm_name) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("DHCP lease saved: {} -> {}", mac, ip),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn delete_dhcp_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let mac = match body.get("mac").and_then(|v| v.as_str()) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: "Missing or empty 'mac' field".into(),
                output: None,
            });
        }
    };
    match crate::db::delete_dhcp_lease(&mac) {
        Ok(_) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("DHCP lease deleted: {}", mac),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

/// Auto-populate DHCP leases from all VM network adapters + MDS config
async fn sync_dhcp_handler() -> HttpResponse {
    let vms = crate::db::list_vms().unwrap_or_default();
    let mut count = 0;
    for vm in &vms {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
            let mds = cfg.get("mds");
            let ip = mds.and_then(|m| m.get("local_ipv4")).and_then(|v| v.as_str()).unwrap_or("");
            let hostname = mds.and_then(|m| m.get("hostname_prefix")).and_then(|v| v.as_str()).unwrap_or("");
            if let Some(adapters) = cfg.get("network_adapters").and_then(|v| v.as_array()) {
                for adapter in adapters {
                    let mac = adapter.get("mac").and_then(|v| v.as_str()).unwrap_or("");
                    if !mac.is_empty() && !ip.is_empty() {
                        let _ = crate::db::upsert_dhcp_lease(mac, ip, hostname, &vm.smac);
                        count += 1;
                    }
                }
            }
        }
    }
    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: format!("Synced {} DHCP leases from VM configs", count),
        output: None,
    })
}

// ── MAC address listing ──
async fn list_macs_handler() -> HttpResponse {
    let vms = crate::db::list_vms().unwrap_or_default();
    let mut macs: Vec<serde_json::Value> = Vec::new();
    for vm in &vms {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
            if let Some(adapters) = cfg.get("network_adapters").and_then(|v| v.as_array()) {
                for adapter in adapters {
                    let mac = adapter.get("mac").and_then(|v| v.as_str()).unwrap_or("");
                    if !mac.is_empty() {
                        macs.push(serde_json::json!({
                            "mac": mac.to_lowercase(),
                            "vm_name": vm.smac,
                        }));
                    }
                }
            }
        }
    }
    HttpResponse::Ok().json(serde_json::json!({ "macs": macs }))
}

// ── Port Forwarding ──

async fn get_port_forwards_handler(path: web::Path<String>) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }
    match crate::db::get_vm(&smac) {
        Ok(vm) => {
            let config: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
            let forwards = config.get("port_forwards").cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            HttpResponse::Ok().json(serde_json::json!({
                "success": true,
                "vm_name": smac,
                "port_forwards": forwards,
            }))
        }
        Err(e) => HttpResponse::NotFound().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
}

async fn add_port_forward_handler(
    path: web::Path<String>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }

    let protocol = body.get("protocol").and_then(|v| v.as_str()).unwrap_or("tcp").to_string();
    let host_port = body.get("host_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let guest_port = body.get("guest_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;

    if host_port == 0 || guest_port == 0 {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "host_port and guest_port are required (non-zero)".into(),
            output: None,
        });
    }

    let result = web::block(move || {
        operations::add_port_forward(&smac, &protocol, host_port, guest_port)
    }).await;

    match result {
        Ok(Ok(output)) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "Port forward added".into(),
            output: Some(output),
        }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Internal error: {}", e),
            output: None,
        }),
    }
}

async fn delete_port_forward_handler(
    path: web::Path<String>,
    body: web::Json<serde_json::Value>,
) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }

    let protocol = body.get("protocol").and_then(|v| v.as_str()).unwrap_or("tcp").to_string();
    let host_port = body.get("host_port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;

    if host_port == 0 {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "host_port is required (non-zero)".into(),
            output: None,
        });
    }

    let result = web::block(move || {
        operations::remove_port_forward(&smac, &protocol, host_port)
    }).await;

    match result {
        Ok(Ok(output)) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: "Port forward removed".into(),
            output: Some(output),
        }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Internal error: {}", e),
            output: None,
        }),
    }
}

// ── IP Pool ──
async fn list_ip_pool_handler() -> HttpResponse {
    let vms = crate::db::list_vms().unwrap_or_default();
    let mut assignments: Vec<serde_json::Value> = Vec::new();
    for vm in &vms {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
            let ip = cfg.get("mds")
                .and_then(|m| m.get("local_ipv4"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let hostname = cfg.get("mds")
                .and_then(|m| m.get("hostname_prefix"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !ip.is_empty() {
                assignments.push(serde_json::json!({
                    "ip": ip,
                    "vm_name": vm.smac,
                    "hostname": hostname,
                    "status": vm.status,
                }));
            }
        }
    }
    // Sort by IP numerically
    assignments.sort_by(|a, b| {
        let ip_a = a["ip"].as_str().unwrap_or("");
        let ip_b = b["ip"].as_str().unwrap_or("");
        let parse_ip = |ip: &str| -> u32 {
            ip.split('.').enumerate().fold(0u32, |acc, (i, p)| {
                acc | ((p.parse::<u32>().unwrap_or(0)) << (8 * (3 - i)))
            })
        };
        parse_ip(ip_a).cmp(&parse_ip(ip_b))
    });

    let next = operations::next_ipv4();

    HttpResponse::Ok().json(serde_json::json!({
        "assignments": assignments,
        "total_assigned": assignments.len(),
        "next_available": next,
    }))
}

async fn list_internal_network_handler() -> HttpResponse {
    let vms = crate::db::list_vms().unwrap_or_default();
    let mut members: Vec<serde_json::Value> = Vec::new();
    for vm in &vms {
        if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
            let ip = cfg
                .get("mds")
                .and_then(|m| m.get("internal_ip"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !ip.is_empty() {
                let internal_mac = operations::derive_internal_mac(ip);
                let hostname = cfg
                    .get("mds")
                    .and_then(|m| m.get("hostname_prefix"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                members.push(serde_json::json!({
                    "vm_name": vm.smac,
                    "internal_ip": ip,
                    "internal_mac": internal_mac,
                    "hostname": hostname,
                    "status": vm.status,
                }));
            }
        }
    }
    // Sort by internal IP numerically
    members.sort_by(|a, b| {
        let ip_a = a["internal_ip"].as_str().unwrap_or("");
        let ip_b = b["internal_ip"].as_str().unwrap_or("");
        let parse_ip = |ip: &str| -> u32 {
            ip.split('.')
                .enumerate()
                .fold(0u32, |acc, (i, p)| {
                    acc | ((p.parse::<u32>().unwrap_or(0)) << (8 * (3 - i)))
                })
        };
        parse_ip(ip_a).cmp(&parse_ip(ip_b))
    });

    let next = operations::next_internal_ip();

    HttpResponse::Ok().json(serde_json::json!({
        "members": members,
        "total": members.len(),
        "next_available": next,
        "subnet": "192.168.100.0/24",
        "multicast_group": "230.0.100.1",
    }))
}

async fn host_ram_handler() -> HttpResponse {
    let host_ram = operations::host_total_ram_mb();
    let used_ram = operations::running_vms_ram_mb(None);
    let reserved: u64 = 1024;
    let usable = host_ram.saturating_sub(reserved);
    let available = usable.saturating_sub(used_ram);

    HttpResponse::Ok().json(serde_json::json!({
        "host_total_mb": host_ram,
        "reserved_mb": reserved,
        "usable_mb": usable,
        "running_vms_mb": used_ram,
        "available_mb": available,
    }))
}

// ──────────────────────────────────────────
// API Key Management Handlers
// ──────────────────────────────────────────

async fn get_apikey_handler(key_state: web::Data<SharedApiKey>) -> HttpResponse {
    let key = key_state.lock().unwrap().clone();
    HttpResponse::Ok().json(serde_json::json!({
        "api_key": key,
        "enabled": !key.is_empty(),
    }))
}

async fn generate_apikey_handler(key_state: web::Data<SharedApiKey>) -> HttpResponse {
    // Generate 64-char hex key
    use std::io::Read;
    let mut bytes = [0u8; 32];
    let new_key = if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
        bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    } else {
        // Fallback: use timestamp + random-ish data
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{:064x}", ts)
    };

    // Update shared state
    {
        let mut key = key_state.lock().unwrap();
        *key = new_key.clone();
    }

    // Save to .api_key file
    let pctl_path = get_conf("pctl_path");
    let key_file = format!("{}/.api_key", pctl_path);
    if let Err(e) = std::fs::write(&key_file, &new_key) {
        eprintln!("Failed to save API key to {}: {}", key_file, e);
    }

    // Also update env var for this process
    std::env::set_var("VMCONTROL_API_KEY", &new_key);

    println!("API key regenerated");

    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "api_key": new_key,
        "message": "API key generated. Update your client with the new key.",
    }))
}

pub async fn start_server(bind_addr: &str) -> std::io::Result<()> {
    env_logger::init();

    // Read API key from environment or .api_key file
    let mut api_key = std::env::var("VMCONTROL_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        // Try reading from .api_key file
        let pctl_path = get_conf("pctl_path");
        let key_file = format!("{}/.api_key", pctl_path);
        if let Ok(key) = std::fs::read_to_string(&key_file) {
            let key = key.trim().to_string();
            if !key.is_empty() {
                api_key = key;
                println!("API key loaded from {}", key_file);
            }
        }
    }
    if api_key.is_empty() {
        println!("WARNING: No VMCONTROL_API_KEY set — API is unauthenticated");
    } else {
        println!("API key authentication enabled");
    }

    // Repair VMs missing mds IPs (from old update_config bug)
    operations::repair_missing_mds_ips();

    // Shared API key state for runtime updates
    let shared_api_key: SharedApiKey = Arc::new(Mutex::new(api_key));

    // Shared VNC token store (one-time tokens)
    let vnc_tokens: VncTokenStore = Arc::new(Mutex::new(HashMap::new()));

    // Cleanup stale VM statuses — if DB says "running" but QEMU is not actually running, set to "stopped"
    {
        let pctl_path = get_conf("pctl_path");
        if let Ok(vms) = crate::db::list_vms() {
            for vm in &vms {
                if vm.status == "running" {
                    let sock_path = format!("{}/{}", pctl_path, vm.smac);
                    let alive = std::os::unix::net::UnixStream::connect(&sock_path).is_ok();
                    if !alive {
                        println!("Stale VM '{}': marked running but QEMU not found — setting to stopped", vm.smac);
                        let _ = crate::db::set_vm_status(&vm.smac, "stopped");
                    }
                }
            }
        }
    }

    let static_path = get_conf("static_path");
    let mds_bind = "169.254.169.254:80";

    println!("VM Control API server starting on http://{}", bind_addr);
    println!("MDS metadata server starting on http://{}", mds_bind);

    // MDS-only server on 169.254.169.254:80
    let mds_server = HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .configure(mds::configure_mds_routes)
    })
    .bind(mds_bind);

    match mds_server {
        Ok(srv) => {
            println!("MDS bound to {} OK", mds_bind);
            tokio::spawn(srv.run());
        }
        Err(e) => {
            eprintln!(
                "WARNING: Cannot bind MDS to {} ({}). MDS still available on {}",
                mds_bind, e, bind_addr
            );
        }
    }

    // Main control panel + MDS on main port
    let api_key_for_server = shared_api_key.clone();
    let vnc_tokens_for_server = vnc_tokens.clone();
    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .wrap(ApiKeyAuth(api_key_for_server.clone()))
            .app_data(web::Data::new(api_key_for_server.clone()))
            .app_data(web::Data::new(vnc_tokens_for_server.clone()))
            // Allow up to 4GB uploads for ISO files
            .app_data(web::PayloadConfig::new(4_294_967_296))
            // API key management routes
            .route("/api/apikey", web::get().to(get_apikey_handler))
            .route("/api/apikey/generate", web::post().to(generate_apikey_handler))
            // API routes
            .route("/api/vm/start", web::post().to(start_vm))
            .route("/api/vm/stop", web::post().to(stop_vm))
            .route("/api/vm/reset", web::post().to(reset_vm))
            .route("/api/vm/powerdown", web::post().to(powerdown_vm))
            .route("/api/vm/create-config", web::post().to(create_config_vm))
            .route("/api/vm/update-config", web::post().to(update_config_vm))
            .route("/api/vm/get/{smac}", web::get().to(get_vm_handler))
            .route("/api/vm/{smac}/mds", web::get().to(get_vm_mds_handler))
            .route("/api/vm/{smac}/mds", web::post().to(save_vm_mds_handler))
            .route("/api/vm/listimage", web::post().to(listimage_vm))
            .route("/api/vm/delete", web::post().to(delete_vm_handler))
            .route("/api/vm/mountiso", web::post().to(mountiso_vm))
            .route("/api/vm/unmountiso", web::post().to(unmountiso_vm))
            .route("/api/vm/livemigrate", web::post().to(livemigrate_vm))
            .route("/api/vm/backup", web::post().to(backup_vm))
            .route("/api/vm/list", web::get().to(list_vms_handler))
            // Device routes
            .route("/api/devices/vfio", web::get().to(list_vfio_devices))
            // Disk routes
            .route("/api/disk/list", web::get().to(list_disks_handler))
            .route("/api/disk/create", web::post().to(create_disk_handler))
            .route("/api/disk/delete", web::post().to(delete_disk_handler))
            .route("/api/disk/clone", web::post().to(clone_disk_handler))
            .route("/api/disk/resize", web::post().to(resize_disk_handler))
            // Image routes
            .route("/api/image/list", web::get().to(list_images_handler))
            .route("/api/image/upload", web::post().to(upload_image_handler))
            .route("/api/image/delete", web::post().to(delete_image_handler))
            // Disk export route
            .route("/api/disk/export/{name}", web::get().to(export_disk_handler))
            // ISO routes
            .route("/api/iso/list", web::get().to(list_isos_handler))
            .route("/api/iso/upload", web::post().to(upload_iso_handler))
            .route("/api/iso/delete", web::post().to(delete_iso_handler))
            // Backup routes
            .route("/api/backup/list", web::get().to(list_backups_handler))
            .route("/api/backup/delete", web::post().to(delete_backup_handler))
            // Group routes
            .route("/api/group/list", web::get().to(list_groups_handler))
            .route("/api/vm/set-group", web::post().to(set_vm_group_handler))
            // MAC address routes
            .route("/api/mac/list", web::get().to(list_macs_handler))
            // Port forwarding routes
            .route("/api/vm/{smac}/portforward", web::get().to(get_port_forwards_handler))
            .route("/api/vm/{smac}/portforward", web::post().to(add_port_forward_handler))
            .route("/api/vm/{smac}/portforward/delete", web::post().to(delete_port_forward_handler))
            // IP pool routes
            .route("/api/ip/list", web::get().to(list_ip_pool_handler))
            // Internal network routes (VM-to-VM in NAT)
            .route("/api/internal-network", web::get().to(list_internal_network_handler))
            // Host resource info
            .route("/api/host/ram", web::get().to(host_ram_handler))
            // DHCP routes
            .route("/api/dhcp/list", web::get().to(list_dhcp_handler))
            .route("/api/dhcp/add", web::post().to(add_dhcp_handler))
            .route("/api/dhcp/delete", web::post().to(delete_dhcp_handler))
            .route("/api/dhcp/sync", web::post().to(sync_dhcp_handler))
            // Switch routes
            .route("/api/switch/list", web::get().to(list_switches_handler))
            .route("/api/switch/create", web::post().to(create_switch_handler))
            .route("/api/switch/delete", web::post().to(delete_switch_handler))
            .route("/api/switch/rename", web::post().to(rename_switch_handler))
            // SSH key routes
            .route("/api/sshkey/list", web::get().to(list_ssh_keys_handler))
            .route("/api/sshkey/create", web::post().to(create_ssh_key_handler))
            .route("/api/sshkey/delete", web::post().to(delete_ssh_key_handler))
            // Template image mapping routes
            .route("/api/template-images", web::get().to(list_template_images_handler))
            .route("/api/template-images/set", web::post().to(set_template_image_handler))
            // OS template CRUD routes
            .route("/api/os-templates", web::get().to(list_os_templates_handler))
            .route("/api/os-templates/create", web::post().to(create_os_template_handler))
            .route("/api/os-templates/update", web::post().to(update_os_template_handler))
            .route("/api/os-templates/delete", web::post().to(delete_os_template_handler))
            // Block info route (per-drive mount status)
            .route("/api/vm/blockinfo/{smac}", web::get().to(blockinfo_handler))
            // VNC routes
            .route("/api/vnc/start", web::post().to(vnc_start_handler))
            .route("/api/vnc/stop", web::post().to(vnc_stop_handler))
            .route("/api/vnc/token", web::post().to(vnc_token_handler))
            .route("/api/vnc/resolve/{token}", web::get().to(vnc_resolve_handler))
            // MDS routes
            .configure(mds::configure_mds_routes)
            // Static files (must be last - catch-all)
            .service(
                fs::Files::new("/", &static_path)
                    .index_file("index.html")
                    .use_last_modified(true)
                    .use_etag(true)
            )
    })
    .bind(bind_addr)?
    .run()
    .await
}
