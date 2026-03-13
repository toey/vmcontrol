use actix_files as fs;
use actix_web::{dev, middleware, web, App, HttpResponse, HttpServer};
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
            let api_key = match api_key_lock.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            };

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
            // Allow phone-home callback from VMs without API key
            if path.ends_with("/phone-home") && path.starts_with("/api/vm/") {
                return svc.call(req).await;
            }
            // Allow /api/apikey/generate without auth when .api_key file has been
            // deleted (e.g., by install.sh before rebuild). Even if the server still
            // has an old key in memory, the missing file signals a fresh install.
            // Only bypasses if the key did NOT come from VMCONTROL_API_KEY env var.
            if path == "/api/apikey/generate"
                && req.method() == actix_web::http::Method::POST
                && std::env::var("VMCONTROL_API_KEY").unwrap_or_default().is_empty()
            {
                let pctl_path = crate::config::get_conf("pctl_path");
                let key_file = format!("{}/.api_key", pctl_path);
                if !std::path::Path::new(&key_file).exists() {
                    return svc.call(req).await;
                }
            }

            // Check X-API-Key header
            let provided = req.headers()
                .get("X-API-Key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            // Constant-time comparison to prevent timing attacks
            let key_match = constant_time_eq(provided.as_bytes(), api_key.as_bytes());
            if key_match {
                svc.call(req).await
            } else {
                Err(actix_web::error::ErrorUnauthorized(
                    "Invalid or missing API key. Set X-API-Key header."
                ))
            }
        })
    }
}

/// Constant-time byte comparison to prevent timing attacks on API key validation.
/// Returns false immediately if lengths differ (length is not secret), but compares
/// all bytes when lengths match to avoid leaking content via timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

async fn rename_vm_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let old_name = body.get("old_name").and_then(|v| v.as_str()).unwrap_or("");
    let new_name = body.get("new_name").and_then(|v| v.as_str()).unwrap_or("");
    match operations::rename_vm(old_name, new_name) {
        Ok(msg) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: msg,
            output: None,
        }),
        Err(e) => HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: e,
            output: None,
        }),
    }
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

// Phone-home callback: VMs call this to report cloud-init completion (no auth required)
async fn phone_home_handler(path: web::Path<String>) -> HttpResponse {
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
            let ts = chrono::Utc::now().to_rfc3339();
            config["cloud_init_completed"] = serde_json::json!(ts);
            let config_str = serde_json::to_string(&config).unwrap_or_default();
            match crate::db::update_vm(&smac, &config_str) {
                Ok(_) => {
                    log::info!("Phone-home received from VM '{}' at {}", smac, ts);
                    HttpResponse::Ok().json(ApiResponse {
                        success: true,
                        message: format!("Phone-home recorded for VM '{}' at {}", smac, ts),
                        output: None,
                    })
                }
                Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Failed to record phone-home: {}", e),
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
    // Wrap blocking QEMU monitor I/O in web::block to avoid blocking the async runtime
    let result = web::block(move || {
        crate::api_helpers::qemu_monitor_cmd(&smac, "info block")
    }).await;
    match result {
        Ok(Ok(raw)) => {
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
        Ok(Err(e)) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to query block info: {}", e),
            output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Internal error: {}", e),
            output: None,
        }),
    }
}

/// Send files to VM — upload multipart files, create ISO, auto-mount on free CD drive
async fn sendfiles_handler(
    path: web::Path<String>,
    mut payload: actix_multipart::Multipart,
) -> HttpResponse {
    use futures_util::StreamExt;

    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }

    // Create temp directory for uploaded files
    let pctl_path = crate::config::get_conf("pctl_path");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let temp_dir = format!("{}/sendfiles_{}_{}", pctl_path, smac, timestamp);
    if let Err(e) = std::fs::create_dir_all(&temp_dir) {
        return HttpResponse::InternalServerError().json(ApiResponse {
            success: false,
            message: format!("Failed to create temp dir: {}", e),
            output: None,
        });
    }

    // Process multipart fields — stream each file to disk
    let mut file_count = 0u32;
    let mut total_size = 0u64;
    const MAX_TOTAL: u64 = 4 * 1024 * 1024 * 1024; // 4GB
    const MAX_FILES: u32 = 500;

    while let Some(item) = payload.next().await {
        if file_count >= MAX_FILES {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: format!("Too many files (max {})", MAX_FILES),
                output: None,
            });
        }
        let mut field = match item {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return HttpResponse::BadRequest().json(ApiResponse {
                    success: false,
                    message: format!("Multipart error: {}", e),
                    output: None,
                });
            }
        };

        // Get filename — sanitize
        let filename = field
            .content_disposition()
            .and_then(|cd| cd.get_filename().map(|s| s.to_string()))
            .unwrap_or_else(|| "file".to_string());
        // Basic sanitize: keep alphanumeric, dash, underscore, dot
        let safe_name: String = filename
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe_name = if safe_name.is_empty() {
            format!("file_{}", file_count)
        } else {
            safe_name
        };

        let file_path = format!("{}/{}", temp_dir, safe_name);
        let mut file = match std::fs::File::create(&file_path) {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Failed to create file: {}", e),
                    output: None,
                });
            }
        };

        // Stream chunks to file
        while let Some(chunk) = field.next().await {
            match chunk {
                Ok(data) => {
                    total_size += data.len() as u64;
                    if total_size > MAX_TOTAL {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        return HttpResponse::BadRequest().json(ApiResponse {
                            success: false,
                            message: "Total file size exceeds 4GB limit".into(),
                            output: None,
                        });
                    }
                    use std::io::Write;
                    if let Err(e) = file.write_all(&data) {
                        let _ = std::fs::remove_dir_all(&temp_dir);
                        return HttpResponse::InternalServerError().json(ApiResponse {
                            success: false,
                            message: format!("Write error: {}", e),
                            output: None,
                        });
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&temp_dir);
                    return HttpResponse::BadRequest().json(ApiResponse {
                        success: false,
                        message: format!("Upload stream error: {}", e),
                        output: None,
                    });
                }
            }
        }
        file_count += 1;
    }

    if file_count == 0 {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "No files uploaded".into(),
            output: None,
        });
    }

    // Create ISO and mount (blocking I/O)
    let smac_clone = smac.clone();
    let temp_dir_clone = temp_dir.clone();
    match actix_web::web::block(move || {
        crate::operations::create_and_mount_sendfiles_iso(&smac_clone, &temp_dir_clone)
    })
    .await
    {
        Ok(Ok((drive, iso_name))) => HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": format!("{} file(s) mounted on {} as {}", file_count, drive, iso_name),
            "drive": drive,
            "iso_name": iso_name,
            "file_count": file_count,
            "total_size": total_size,
        })),
        Ok(Err(e)) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: e,
                output: None,
            })
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: format!("Internal error: {}", e),
                output: None,
            })
        }
    }
}

/// Cleanup sendfiles ISO — unmount drive and delete temp ISO
async fn cleanup_sendfiles_handler(
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

    let drive = body
        .get("drive")
        .and_then(|v| v.as_str())
        .unwrap_or("cd0");

    // Validate drive is cd0-cd3
    if !matches!(drive, "cd0" | "cd1" | "cd2" | "cd3") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid drive: {}", drive),
            output: None,
        });
    }

    let iso_name = body
        .get("iso_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Unmount drive
    let unmount_arg = format!("{} {}", smac, drive);
    let _ = crate::api_helpers::send_cmd_pctl("unmountiso", &unmount_arg);

    // Delete temp ISO file — validate: must start with sendfiles_, no path separators
    if !iso_name.is_empty()
        && iso_name.starts_with("sendfiles_")
        && iso_name.ends_with(".iso")
        && !iso_name.contains('/')
        && !iso_name.contains('\\')
        && !iso_name.contains("..")
    {
        let iso_path = format!("{}/{}", crate::config::get_conf("iso_path"), iso_name);
        let _ = std::fs::remove_file(&iso_path);
    }

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: format!("Cleaned up {} on {}", iso_name, drive),
        output: None,
    })
}

/// Check if QEMU Guest Agent is available for a VM
async fn guest_agent_status_handler(path: web::Path<String>) -> HttpResponse {
    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }
    let smac_clone = smac.clone();
    let available = actix_web::web::block(move || crate::guest_agent::guest_ping(&smac_clone))
        .await
        .unwrap_or(false);
    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "available": available,
    }))
}

/// Write a file to VM filesystem via QEMU Guest Agent
async fn guest_file_write_handler(
    path: web::Path<String>,
    req: actix_web::HttpRequest,
    mut payload: web::Payload,
) -> HttpResponse {
    use futures_util::StreamExt;

    let smac = path.into_inner();
    if let Err(e) = crate::ssh::sanitize_name(&smac) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid VM name: {}", e),
            output: None,
        });
    }

    // Get target path and filename from headers
    let guest_path = req
        .headers()
        .get("X-Guest-Path")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("/tmp/");
    let filename = req
        .headers()
        .get("X-Filename")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("upload");

    // Validate filename: no path separators or traversal
    if filename.contains('/')
        || filename.contains('\\')
        || filename.contains("..")
        || filename.is_empty()
    {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: format!("Invalid filename: {}", filename),
            output: None,
        });
    }

    // Validate guest path: must be absolute, no traversal
    if guest_path.contains("..") || (!guest_path.starts_with('/') && !guest_path.contains(':')) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Guest path must be absolute and not contain '..'".into(),
            output: None,
        });
    }

    // Build full guest file path
    let full_path = if guest_path.ends_with('/') || guest_path.ends_with('\\') {
        format!("{}{}", guest_path, filename)
    } else {
        format!("{}/{}", guest_path, filename)
    };

    // Stream payload into memory (QGA requires full file for base64 encoding)
    // Limit to 256MB for guest agent transfers
    let mut data = Vec::new();
    const MAX_QGA_SIZE: usize = 256 * 1024 * 1024;
    while let Some(chunk) = payload.next().await {
        match chunk {
            Ok(bytes) => {
                data.extend_from_slice(&bytes);
                if data.len() > MAX_QGA_SIZE {
                    return HttpResponse::BadRequest().json(ApiResponse {
                        success: false,
                        message: format!(
                            "File too large for guest agent (max {} MB). Use ISO method instead.",
                            MAX_QGA_SIZE / (1024 * 1024)
                        ),
                        output: None,
                    });
                }
            }
            Err(e) => {
                return HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Stream error: {}", e),
                    output: None,
                });
            }
        }
    }

    let data_len = data.len();
    let smac_clone = smac.clone();
    let path_clone = full_path.clone();
    match actix_web::web::block(move || {
        crate::guest_agent::guest_file_write(&smac_clone, &path_clone, &data)
    })
    .await
    {
        Ok(Ok(())) => HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": format!("File written to {} ({} bytes)", full_path, data_len),
            "path": full_path,
            "size": data_len,
        })),
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

    // Generate random token using the shared urandom helper
    let token = {
        let bytes = operations::read_urandom_bytes(24);
        bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    };

    // Purge expired tokens (older than 5 minutes) and store new one
    {
        let mut map = match store.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
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
        let mut map = match store.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
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

    let (vnc_port, is_windows, arch, vmctl_password) = if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
        let port = cfg.get("vnc_port").and_then(|v| v.as_u64()).unwrap_or(0);
        let win = cfg.pointer("/features/is_windows")
            .and_then(|v| v.as_str())
            .map(|s| s == "1")
            .unwrap_or(false);
        let a = cfg.get("arch")
            .and_then(|v| v.as_str())
            .unwrap_or("x86_64")
            .to_string();
        let pw = cfg.get("vmctl_password")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        (port, win, a, pw)
    } else {
        (0, false, "x86_64".to_string(), String::new())
    };

    HttpResponse::Ok().json(serde_json::json!({
        "success": true,
        "smac": vnc_token.smac,
        "vnc_port": vnc_port,
        "status": vm.status,
        "is_windows": is_windows,
        "arch": arch,
        "vmctl_password": vmctl_password,
    }))
}

/// List VMs — auto-backfill VNC ports + health-check stale "running" status
async fn list_vms_handler() -> HttpResponse {
    match crate::db::list_vms() {
        Ok(mut vms) => {
            // Health check: detect stale "running" status by checking monitor socket
            let pctl_path = crate::config::get_conf("pctl_path");
            for vm in vms.iter_mut() {
                if vm.status == "running" {
                    let sock_path = format!("{}/{}", pctl_path, vm.smac);
                    if !std::path::Path::new(&sock_path).exists() {
                        // QEMU monitor socket gone = VM crashed or was killed externally
                        log::warn!("VM '{}' marked running but monitor socket missing — marking stopped", vm.smac);
                        vm.status = "stopped".to_string();
                        let _ = crate::db::set_vm_status(&vm.smac, "stopped");
                    }
                }
            }

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
    mut payload: web::Payload,
) -> HttpResponse {
    use futures_util::StreamExt;

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

    // Stream payload to file (no RAM buffering)
    let mut file = match std::fs::File::create(&dest) {
        Ok(f) => f,
        Err(e) => {
            return HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: format!("Failed to create file: {}", e),
                output: None,
            });
        }
    };

    let mut total_size: u64 = 0;
    while let Some(chunk) = payload.next().await {
        match chunk {
            Ok(data) => {
                total_size += data.len() as u64;
                if let Err(e) = std::io::Write::write_all(&mut file, &data) {
                    drop(file);
                    let _ = std::fs::remove_file(&dest);
                    return HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        message: format!("Failed to write data: {}", e),
                        output: None,
                    });
                }
            }
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&dest);
                return HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Upload stream error: {}", e),
                    output: None,
                });
            }
        }
    }
    drop(file);

    HttpResponse::Ok().json(ApiResponse {
        success: true,
        message: format!("Uploaded {} ({} bytes)", safe_name, total_size),
        output: Some(dest),
    })
}

// List PCI devices currently bound to vfio-pci driver (Linux only)
async fn list_vfio_devices() -> HttpResponse {
    let devices: Vec<serde_json::Value> = Vec::new();

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
                        // Detect backing file from qcow2 header and save to DB
                        if let Ok(Some(backing)) = operations::get_disk_backing_info(base) {
                            let _ = crate::db::set_disk_backing(base, &backing);
                        }
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
                let clone_count = crate::db::count_linked_clones(&d.name).unwrap_or(0);
                serde_json::json!({
                    "name": d.name,
                    "filename": format!("{}.qcow2", d.name),
                    "disk_size": d.size,
                    "size": file_size,
                    "owner": d.owner,
                    "backing_file": d.backing_file,
                    "is_template": d.is_template,
                    "clone_count": clone_count,
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

    // Check if disk has linked clones depending on it
    if let Ok(clone_count) = crate::db::count_linked_clones(name) {
        if clone_count > 0 {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: format!("Cannot delete '{}': {} linked clone(s) depend on it. Flatten or delete them first.", name, clone_count),
                output: None,
            });
        }
    }

    // Check if disk is a locked template
    if let Ok(disks) = crate::db::list_disks() {
        for d in &disks {
            if d.name == name && d.is_template == "1" {
                return HttpResponse::BadRequest().json(ApiResponse {
                    success: false,
                    message: format!("Disk '{}' is locked as a template. Unset template first.", name),
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

    // Linked clone (default) or full copy
    let linked = body.get("linked").and_then(|v| v.as_bool()).unwrap_or(true);

    let src = src_file.clone();
    let dst = dst_file.clone();
    let nn = new_name.clone();
    let sn = source.clone();
    let result = web::block(move || {
        if linked {
            // Linked clone: qemu-img create -b source -F qcow2 dest
            let qemu_img = get_conf("qemu_img_path");
            crate::ssh::run_cmd(&qemu_img, &[
                "create", "-f", "qcow2", "-b", &src, "-F", "qcow2", &dst
            ]).map_err(|e| format!("Linked clone failed: {}", e))?;
            crate::db::insert_disk_with_backing(&nn, "", &sn)
                .map_err(|e| format!("DB insert error: {}", e))?;
            Ok::<String, String>(format!("Linked clone '{}' -> '{}' (backing: {})", sn, nn, sn))
        } else {
            // Full copy: use qemu-img convert to flatten any backing chain
            let qemu_img = get_conf("qemu_img_path");
            crate::ssh::run_cmd(&qemu_img, &[
                "convert", "-O", "qcow2", &src, &dst
            ]).map_err(|e| format!("Full copy failed: {}", e))?;
            let size = std::fs::metadata(&dst)
                .map(|m| {
                    let mb = m.len() / 1024 / 1024;
                    if mb >= 1024 { format!("{}G", mb / 1024) } else { format!("{}M", mb) }
                })
                .unwrap_or_else(|_| "0".into());
            crate::db::insert_disk(&nn, &size)
                .map_err(|e| format!("DB insert error: {}", e))?;

            // Copy UEFI NVRAM from source disk's owner VM (if exists)
            // This preserves Windows/UEFI boot entries for template disks
            let pctl_path = get_conf("pctl_path");
            let disk_path = get_conf("disk_path");
            if let Ok(disks) = crate::db::list_disks() {
                let owner = disks.iter()
                    .find(|d| d.name == sn)
                    .map(|d| d.owner.clone())
                    .unwrap_or_default();
                if !owner.is_empty() {
                    let src_nvram = format!("{}/{}_efivars.fd", pctl_path, owner);
                    let dst_nvram = format!("{}/{}_efivars.fd", disk_path, nn);
                    if std::path::Path::new(&src_nvram).exists() {
                        match std::fs::copy(&src_nvram, &dst_nvram) {
                            Ok(_) => log::info!("Copied NVRAM {} -> {}", src_nvram, dst_nvram),
                            Err(e) => log::warn!("Failed to copy NVRAM: {}", e),
                        }
                    }
                }
            }

            Ok::<String, String>(format!("Full copy '{}' -> '{}' (standalone)", sn, nn))
        }
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

async fn flatten_disk_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false, message: "Missing 'name' field".into(), output: None,
            });
        }
    };
    if name.contains('/') || name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Invalid disk name".into(), output: None,
        });
    }

    // Must not be in use by a running VM
    if let Err(e) = operations::check_disk_not_in_use(&name) {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: e, output: None,
        });
    }

    let disk_path = get_conf("disk_path");
    let src = format!("{}/{}.qcow2", disk_path, name);
    let tmp = format!("{}/{}_flatten_tmp.qcow2", disk_path, name);
    let nn = name.clone();

    let result = web::block(move || {
        // Convert (flatten) to temp file
        let qemu_img = get_conf("qemu_img_path");
        crate::ssh::run_cmd(&qemu_img, &["convert", "-O", "qcow2", &src, &tmp])
            .map_err(|e| format!("Flatten failed: {}", e))?;
        // Replace original
        std::fs::rename(&tmp, &src)
            .map_err(|e| format!("Rename failed: {}", e))?;
        // Update DB: clear backing_file
        let _ = crate::db::set_disk_backing(&nn, "");
        // Update size in DB
        if let Ok(meta) = std::fs::metadata(&src) {
            let mb = meta.len() / 1024 / 1024;
            let size = if mb >= 1024 { format!("{}G", mb / 1024) } else { format!("{}M", mb) };
            let _ = crate::db::update_disk_size(&nn, &size);
        }
        Ok::<String, String>(format!("Flattened '{}'", nn))
    })
    .await;

    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse {
            success: true, message: format!("Disk '{}' flattened to standalone", name), output: Some(msg),
        }),
        Ok(Err(e)) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: e, output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: e.to_string(), output: None,
        }),
    }
}

async fn set_template_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => {
            return HttpResponse::BadRequest().json(ApiResponse {
                success: false, message: "Missing 'name' field".into(), output: None,
            });
        }
    };
    let is_template = body.get("is_template").and_then(|v| v.as_str()).unwrap_or("0");

    match crate::db::set_disk_template(name, is_template) {
        Ok(()) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: format!("Disk '{}' template = {}", name, if is_template == "1" { "locked" } else { "unlocked" }),
            output: None,
        }),
        Err(e) => HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: e, output: None,
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
    mut payload: web::Payload,
) -> HttpResponse {
    use futures_util::StreamExt;

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

    // Stream payload to file (no RAM buffering for large files)
    let mut file = match std::fs::File::create(&upload_path) {
        Ok(f) => f,
        Err(e) => {
            return HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: format!("Failed to create file: {}", e),
                output: None,
            });
        }
    };

    let mut file_size: u64 = 0;
    while let Some(chunk) = payload.next().await {
        match chunk {
            Ok(data) => {
                file_size += data.len() as u64;
                if let Err(e) = std::io::Write::write_all(&mut file, &data) {
                    drop(file);
                    let _ = std::fs::remove_file(&upload_path);
                    return HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        message: format!("Failed to write data: {}", e),
                        output: None,
                    });
                }
            }
            Err(e) => {
                drop(file);
                let _ = std::fs::remove_file(&upload_path);
                return HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Upload stream error: {}", e),
                    output: None,
                });
            }
        }
    }
    drop(file);

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

/// Export a complete VM (config + disk files) as a ZIP archive
async fn export_vm_handler(
    req: actix_web::HttpRequest,
    path: web::Path<String>,
) -> HttpResponse {
    let smac = path.into_inner();

    // Sanitize
    if smac.is_empty() || smac.contains('/') || smac.contains('\\') || smac.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid VM name".into(),
            output: None,
        });
    }

    // Look up VM
    let vm = match crate::db::get_vm(&smac) {
        Ok(v) => v,
        Err(_) => {
            return HttpResponse::NotFound().json(ApiResponse {
                success: false,
                message: format!("VM '{}' not found", smac),
                output: None,
            });
        }
    };

    // Must be stopped
    if vm.status == "running" {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Cannot export a running VM. Stop it first.".into(),
            output: None,
        });
    }

    // Build export metadata
    let config_val: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
    let export_meta = serde_json::json!({
        "version": 1,
        "smac": vm.smac,
        "mac": vm.mac,
        "disk_size": vm.disk_size,
        "config": config_val,
        "group_name": vm.group_name,
        "created_at": vm.created_at,
    });

    // Collect disk names from config
    let config_disk_names: Vec<String> = config_val
        .get("disks")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|d| d.get("diskname").and_then(|v| v.as_str()))
                .filter(|n| !n.is_empty())
                .map(|n| n.to_string())
                .collect()
        })
        .unwrap_or_default();

    let disk_path = get_conf("disk_path");
    let tmp_zip = format!("{}/{}_export_{}.zip", disk_path, smac, std::process::id());
    let zip_path = tmp_zip.clone();
    let meta_json = serde_json::to_string_pretty(&export_meta).unwrap_or_default();
    let dp = disk_path.clone();

    // Build ZIP in blocking thread
    let build_result = web::block(move || {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;

        let file = std::fs::File::create(&zip_path)
            .map_err(|e| format!("Failed to create zip: {}", e))?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);

        // Write vm-config.json
        zip.start_file("vm-config.json", options)
            .map_err(|e| format!("ZIP error: {}", e))?;
        zip.write_all(meta_json.as_bytes())
            .map_err(|e| format!("ZIP write error: {}", e))?;

        // Write each disk file into disks/ folder
        // For linked clones, auto-flatten to standalone before adding to ZIP
        let qemu_img = get_conf("qemu_img_path");
        let mut tmp_flattened: Vec<String> = Vec::new();

        for disk_name in &config_disk_names {
            let qcow2_path = format!("{}/{}.qcow2", dp, disk_name);
            if !std::path::Path::new(&qcow2_path).exists() {
                continue;
            }

            // Check if this disk has a backing file (linked clone)
            let has_backing = crate::operations::get_disk_backing_info(disk_name)
                .unwrap_or(None)
                .is_some();

            let source_path = if has_backing {
                // Flatten to a temp file for export
                let tmp_path = format!("{}/{}_export_flat_{}.qcow2", dp, disk_name, std::process::id());
                let result = crate::ssh::run_cmd(
                    &qemu_img,
                    &["convert", "-O", "qcow2", &qcow2_path, &tmp_path],
                );
                if let Err(e) = result {
                    // Clean up any temp files created so far
                    for p in &tmp_flattened {
                        let _ = std::fs::remove_file(p);
                    }
                    return Err(format!("Failed to flatten linked clone {}: {}", disk_name, e));
                }
                tmp_flattened.push(tmp_path.clone());
                tmp_path
            } else {
                qcow2_path.clone()
            };

            zip.start_file(format!("disks/{}.qcow2", disk_name), options)
                .map_err(|e| format!("ZIP error: {}", e))?;
            let mut disk_file = std::fs::File::open(&source_path)
                .map_err(|e| format!("Failed to read disk {}: {}", disk_name, e))?;
            std::io::copy(&mut disk_file, &mut zip)
                .map_err(|e| format!("ZIP write error for {}: {}", disk_name, e))?;
        }

        // Clean up temp flattened files
        for p in &tmp_flattened {
            let _ = std::fs::remove_file(p);
        }

        zip.finish().map_err(|e| format!("ZIP finish error: {}", e))?;
        Ok::<(), String>(())
    })
    .await;

    // Stream the ZIP file
    match build_result {
        Ok(Ok(())) => {
            match actix_files::NamedFile::open_async(&tmp_zip).await {
                Ok(f) => {
                    let cleanup_path = tmp_zip.clone();
                    actix_web::rt::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
                        let _ = std::fs::remove_file(&cleanup_path);
                    });
                    f.set_content_disposition(actix_web::http::header::ContentDisposition {
                        disposition: actix_web::http::header::DispositionType::Attachment,
                        parameters: vec![
                            actix_web::http::header::DispositionParam::Filename(
                                format!("{}.zip", smac),
                            ),
                        ],
                    })
                    .into_response(&req)
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp_zip);
                    HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        message: format!("Failed to open zip file: {}", e),
                        output: None,
                    })
                }
            }
        }
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&tmp_zip);
            HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: e,
                output: None,
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_zip);
            HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: format!("Internal error: {}", e),
                output: None,
            })
        }
    }
}

/// Import a VM from a ZIP archive (config + disk files)
async fn import_vm_handler(
    _req: actix_web::HttpRequest,
    mut payload: web::Payload,
) -> HttpResponse {
    use futures_util::StreamExt;

    let disk_path = get_conf("disk_path");
    let _ = std::fs::create_dir_all(&disk_path);
    let tmp_zip = format!("{}/vm_import_{}.zip", disk_path, std::process::id());

    // Stream uploaded ZIP to temp file (no RAM buffering)
    {
        let mut file = match std::fs::File::create(&tmp_zip) {
            Ok(f) => f,
            Err(e) => {
                return HttpResponse::InternalServerError().json(ApiResponse {
                    success: false,
                    message: format!("Failed to create temp file: {}", e),
                    output: None,
                });
            }
        };
        while let Some(chunk) = payload.next().await {
            match chunk {
                Ok(data) => {
                    if let Err(e) = std::io::Write::write_all(&mut file, &data) {
                        drop(file);
                        let _ = std::fs::remove_file(&tmp_zip);
                        return HttpResponse::InternalServerError().json(ApiResponse {
                            success: false,
                            message: format!("Failed to write data: {}", e),
                            output: None,
                        });
                    }
                }
                Err(e) => {
                    drop(file);
                    let _ = std::fs::remove_file(&tmp_zip);
                    return HttpResponse::InternalServerError().json(ApiResponse {
                        success: false,
                        message: format!("Upload stream error: {}", e),
                        output: None,
                    });
                }
            }
        }
    }

    let zip_path = tmp_zip.clone();
    let dp = disk_path.clone();

    let import_result = web::block(move || {
        use std::io::Read;

        let file = std::fs::File::open(&zip_path)
            .map_err(|e| format!("Failed to open zip: {}", e))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("Invalid ZIP file: {}", e))?;

        // Read vm-config.json
        let meta_json = {
            let mut entry = archive
                .by_name("vm-config.json")
                .map_err(|_| "ZIP missing vm-config.json".to_string())?;
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| format!("Failed to read vm-config.json: {}", e))?;
            buf
        };

        let meta: serde_json::Value = serde_json::from_str(&meta_json)
            .map_err(|e| format!("Invalid vm-config.json: {}", e))?;

        let smac = meta
            .get("smac")
            .and_then(|v| v.as_str())
            .ok_or("vm-config.json missing 'smac'")?
            .to_string();

        // Validate VM name
        operations::validate_vm_name(&smac)?;

        // Check name uniqueness
        if crate::db::get_vm(&smac).is_ok() {
            return Err(format!(
                "VM name '{}' already exists. Rename or delete it first.",
                smac
            ));
        }

        let mut config = meta.get("config").cloned().unwrap_or(serde_json::json!({}));
        let group_name = meta
            .get("group_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let disk_size = meta
            .get("disk_size")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Generate new MAC addresses for all network adapters
        if let Some(adapters) = config
            .get_mut("network_adapters")
            .and_then(|a| a.as_array_mut())
        {
            for adapter in adapters.iter_mut() {
                if let Some(obj) = adapter.as_object_mut() {
                    // Generate unique MAC (retry up to 100 times)
                    let mut new_mac = operations::generate_random_mac();
                    for _ in 0..100 {
                        // Quick check: build a temp config to validate
                        let test_config = serde_json::json!({
                            "network_adapters": [{"mac": new_mac}]
                        });
                        if operations::validate_mac_uniqueness(&test_config, None).is_ok() {
                            break;
                        }
                        // Add a small delay for entropy
                        std::thread::sleep(std::time::Duration::from_millis(1));
                        new_mac = operations::generate_random_mac();
                    }
                    obj.insert("mac".to_string(), serde_json::json!(new_mac));
                }
            }
        }

        // Auto-assign new VNC port
        let port = operations::next_vnc_port()?;
        config
            .as_object_mut()
            .unwrap()
            .insert("vnc_port".to_string(), serde_json::json!(port));

        // Auto-assign new IPs
        let new_ipv4 = operations::next_ipv4();
        let new_internal = operations::next_internal_ip();
        if let Some(mds) = config.get_mut("mds").and_then(|m| m.as_object_mut()) {
            mds.insert("local_ipv4".to_string(), serde_json::json!(new_ipv4));
            mds.insert("internal_ip".to_string(), serde_json::json!(new_internal));
        }

        // Validate MAC uniqueness for the full config
        operations::validate_mac_uniqueness(&config, None)?;

        // Collect disk file entries from ZIP
        let mut disk_entries: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            let entry = archive
                .by_index(i)
                .map_err(|e| format!("ZIP entry error: {}", e))?;
            let entry_name = entry.name().to_string();
            if entry_name.starts_with("disks/") && entry_name.ends_with(".qcow2") {
                disk_entries.push(entry_name);
            }
        }

        // Extract disk files
        let mut disk_names: Vec<String> = Vec::new();
        for entry_name in &disk_entries {
            let filename = entry_name.strip_prefix("disks/").unwrap();
            // Sanitize filename
            let safe_name: String = filename
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
                .collect();
            if safe_name.is_empty() || safe_name.contains("..") {
                continue;
            }

            let dest = format!("{}/{}", dp, safe_name);
            if std::path::Path::new(&dest).exists() {
                return Err(format!(
                    "Disk file '{}' already exists on this host",
                    safe_name
                ));
            }

            let mut entry = archive
                .by_name(entry_name)
                .map_err(|e| format!("ZIP entry error: {}", e))?;
            let mut out_file = std::fs::File::create(&dest)
                .map_err(|e| format!("Failed to create {}: {}", safe_name, e))?;
            std::io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("Failed to extract {}: {}", safe_name, e))?;

            let base = safe_name.trim_end_matches(".qcow2");
            disk_names.push(base.to_string());
        }

        // Create DB entries
        let config_str = serde_json::to_string(&config).unwrap_or_default();
        let mac_str = config
            .get("network_adapters")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .and_then(|a| a.get("mac"))
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        crate::db::insert_vm(&smac, &mac_str, &disk_size, &config_str)?;
        if !group_name.is_empty() {
            let _ = crate::db::set_vm_group(&smac, &group_name);
        }

        // Register disks and set ownership
        for disk_name in &disk_names {
            let _ = crate::db::insert_disk(disk_name, "");
            let _ = crate::db::set_disk_owner(disk_name, &smac);
        }

        // Cleanup temp zip
        let _ = std::fs::remove_file(&zip_path);

        Ok(format!(
            "VM '{}' imported successfully with {} disk(s)",
            smac,
            disk_names.len()
        ))
    })
    .await;

    match import_result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse {
            success: true,
            message: msg,
            output: None,
        }),
        Ok(Err(e)) => {
            let _ = std::fs::remove_file(&tmp_zip);
            HttpResponse::BadRequest().json(ApiResponse {
                success: false,
                message: e,
                output: None,
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_zip);
            HttpResponse::InternalServerError().json(ApiResponse {
                success: false,
                message: format!("Internal error: {}", e),
                output: None,
            })
        }
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

// ======== Full Backup Management ========

async fn create_full_backup_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let vm_name = match body.get("vm_name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'vm_name'".into(), output: None,
        }),
    };
    let note = body.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let result = web::block(move || operations::create_full_backup(&vm_name, &note)).await;
    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse { success: true, message: msg, output: None }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse { success: false, message: e, output: None }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse { success: false, message: e.to_string(), output: None }),
    }
}

async fn list_full_backups_handler() -> HttpResponse {
    match crate::db::list_backups() {
        Ok(backups) => HttpResponse::Ok().json(backups),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: format!("Failed to list backups: {}", e), output: None,
        }),
    }
}

async fn restore_full_backup_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let backup_id = match body.get("backup_id").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'backup_id'".into(), output: None,
        }),
    };
    let vm_name = match body.get("vm_name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'vm_name'".into(), output: None,
        }),
    };
    let result = web::block(move || operations::restore_full_backup(&backup_id, &vm_name)).await;
    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse { success: true, message: msg, output: None }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse { success: false, message: e, output: None }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse { success: false, message: e.to_string(), output: None }),
    }
}

async fn delete_full_backup_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let backup_id = match body.get("backup_id").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'backup_id'".into(), output: None,
        }),
    };
    let result = web::block(move || operations::delete_full_backup(&backup_id)).await;
    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse { success: true, message: msg, output: None }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse { success: false, message: e, output: None }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse { success: false, message: e.to_string(), output: None }),
    }
}

// ======== Snapshot Management ========

async fn create_snapshot_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let vm_name = match body.get("vm_name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'vm_name'".into(), output: None,
        }),
    };
    let note = body.get("note").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let result = web::block(move || operations::create_snapshot(&vm_name, &note)).await;
    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse { success: true, message: msg, output: None }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse { success: false, message: e, output: None }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse { success: false, message: e.to_string(), output: None }),
    }
}

async fn list_snapshots_handler(path: web::Path<String>) -> HttpResponse {
    let vm_name = path.into_inner();
    if vm_name.is_empty() || vm_name.contains('/') || vm_name.contains("..") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Invalid VM name".into(), output: None,
        });
    }
    match operations::list_vm_snapshots(&vm_name) {
        Ok(snapshots) => HttpResponse::Ok().json(snapshots),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: e, output: None,
        }),
    }
}

async fn revert_snapshot_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let vm_name = match body.get("vm_name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'vm_name'".into(), output: None,
        }),
    };
    let snapshot_id = match body.get("snapshot_id").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'snapshot_id'".into(), output: None,
        }),
    };
    let result = web::block(move || operations::revert_snapshot(&vm_name, &snapshot_id)).await;
    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse { success: true, message: msg, output: None }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse { success: false, message: e, output: None }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse { success: false, message: e.to_string(), output: None }),
    }
}

async fn delete_snapshot_handler(body: web::Json<serde_json::Value>) -> HttpResponse {
    let vm_name = match body.get("vm_name").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'vm_name'".into(), output: None,
        }),
    };
    let snapshot_id = match body.get("snapshot_id").and_then(|v| v.as_str()) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Missing 'snapshot_id'".into(), output: None,
        }),
    };
    let result = web::block(move || operations::delete_snapshot(&vm_name, &snapshot_id)).await;
    match result {
        Ok(Ok(msg)) => HttpResponse::Ok().json(ApiResponse { success: true, message: msg, output: None }),
        Ok(Err(e)) => HttpResponse::BadRequest().json(ApiResponse { success: false, message: e, output: None }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse { success: false, message: e.to_string(), output: None }),
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
    let key = match key_state.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    HttpResponse::Ok().json(serde_json::json!({
        "api_key": key,
        "enabled": !key.is_empty(),
    }))
}

async fn generate_apikey_handler(key_state: web::Data<SharedApiKey>) -> HttpResponse {
    // Generate 64-char hex key using shared random source
    let bytes = operations::read_urandom_bytes(32);
    let new_key = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();

    // Update shared state
    {
        let mut key = match key_state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *key = new_key.clone();
    }

    // Save to .api_key file with restrictive permissions
    let pctl_path = get_conf("pctl_path");
    let key_file = format!("{}/.api_key", pctl_path);
    if let Err(e) = std::fs::write(&key_file, &new_key) {
        eprintln!("Failed to save API key to {}: {}", key_file, e);
    } else {
        // Set permissions to owner-only (0600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600));
        }
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

// ──────────────────────────────────────────
// Disk File Editor handlers
// ──────────────────────────────────────────

async fn disk_edit_supported_handler() -> HttpResponse {
    let supported = cfg!(target_os = "linux") || cfg!(target_os = "macos");
    HttpResponse::Ok().json(serde_json::json!({
        "supported": supported,
        "platform": std::env::consts::OS,
    }))
}

async fn mount_disk_handler(
    body: web::Json<serde_json::Value>,
    store: web::Data<crate::disk_edit::MountedDiskStore>,
) -> HttpResponse {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Disk name required".into(), output: None,
        });
    }
    let s = store.get_ref().clone();
    let n = name.to_string();
    match web::block(move || crate::disk_edit::mount_disk(&n, &s)).await {
        Ok(Ok(info)) => HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "message": format!("Disk '{}' mounted at {}", name, info.mount_point),
            "mount_point": info.mount_point,
            "read_only": info.read_only,
        })),
        Ok(Err(e)) => HttpResponse::Ok().json(ApiResponse {
            success: false, message: e, output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: format!("Internal error: {}", e), output: None,
        }),
    }
}

async fn unmount_disk_handler(
    body: web::Json<serde_json::Value>,
    store: web::Data<crate::disk_edit::MountedDiskStore>,
) -> HttpResponse {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false, message: "Disk name required".into(), output: None,
        });
    }
    let s = store.get_ref().clone();
    let n = name.to_string();
    match web::block(move || crate::disk_edit::unmount_disk(&n, &s)).await {
        Ok(Ok(())) => HttpResponse::Ok().json(ApiResponse {
            success: true, message: format!("Disk '{}' unmounted", name), output: None,
        }),
        Ok(Err(e)) => HttpResponse::Ok().json(ApiResponse {
            success: false, message: e, output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: format!("Internal error: {}", e), output: None,
        }),
    }
}

async fn list_mounted_disks_handler(
    store: web::Data<crate::disk_edit::MountedDiskStore>,
) -> HttpResponse {
    let locked = match store.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let list: Vec<_> = locked.values().cloned().collect();
    HttpResponse::Ok().json(list)
}

async fn browse_disk_handler(
    path: web::Path<String>,
    req: actix_web::HttpRequest,
    store: web::Data<crate::disk_edit::MountedDiskStore>,
) -> HttpResponse {
    let name = path.into_inner();
    let query = web::Query::<HashMap<String, String>>::from_query(req.query_string())
        .unwrap_or_else(|_| web::Query(HashMap::new()));
    let dir = query.get("path").map(|s| s.as_str()).unwrap_or("/");
    let s = store.get_ref().clone();
    let n = name.clone();
    let d = dir.to_string();
    match web::block(move || crate::disk_edit::list_files(&n, &d, &s)).await {
        Ok(Ok(entries)) => HttpResponse::Ok().json(entries),
        Ok(Err(e)) => HttpResponse::Ok().json(ApiResponse {
            success: false, message: e, output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: format!("Internal error: {}", e), output: None,
        }),
    }
}

async fn read_disk_file_handler(
    path: web::Path<String>,
    req: actix_web::HttpRequest,
    store: web::Data<crate::disk_edit::MountedDiskStore>,
) -> HttpResponse {
    let name = path.into_inner();
    let query = web::Query::<HashMap<String, String>>::from_query(req.query_string())
        .unwrap_or_else(|_| web::Query(HashMap::new()));
    let file_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
    let s = store.get_ref().clone();
    let n = name.clone();
    let fp = file_path.to_string();
    match web::block(move || crate::disk_edit::read_file(&n, &fp, &s)).await {
        Ok(Ok(content)) => HttpResponse::Ok().json(serde_json::json!({
            "success": true,
            "content": content,
        })),
        Ok(Err(e)) => HttpResponse::Ok().json(ApiResponse {
            success: false, message: e, output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: format!("Internal error: {}", e), output: None,
        }),
    }
}

async fn write_disk_file_handler(
    path: web::Path<String>,
    body: web::Json<serde_json::Value>,
    store: web::Data<crate::disk_edit::MountedDiskStore>,
) -> HttpResponse {
    let name = path.into_inner();
    let file_path = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let s = store.get_ref().clone();
    let n = name.clone();
    let fp = file_path.to_string();
    let c = content.to_string();
    match web::block(move || crate::disk_edit::write_file(&n, &fp, &c, &s)).await {
        Ok(Ok(())) => HttpResponse::Ok().json(ApiResponse {
            success: true, message: "File saved".into(), output: None,
        }),
        Ok(Err(e)) => HttpResponse::Ok().json(ApiResponse {
            success: false, message: e, output: None,
        }),
        Err(e) => HttpResponse::InternalServerError().json(ApiResponse {
            success: false, message: format!("Internal error: {}", e), output: None,
        }),
    }
}

/// Cleanup stale seed ISOs and directories from deleted VMs
fn cleanup_stale_seed_isos() {
    let pctl_path = get_conf("pctl_path");
    let vm_names: std::collections::HashSet<String> =
        crate::db::list_vms().unwrap_or_default().iter().map(|v| v.smac.clone()).collect();

    if let Ok(entries) = std::fs::read_dir(&pctl_path) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Cleanup seed ISO files for VMs that no longer exist
            if name.starts_with("seed_") && name.ends_with(".iso") {
                let vm_name = name.trim_start_matches("seed_").trim_end_matches(".iso");
                if !vm_names.contains(vm_name) {
                    log::info!("Cleaning up stale seed ISO: {}", name);
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            // Cleanup stale seed directories
            if name.starts_with("seed_") && entry.path().is_dir() {
                let vm_name = name.trim_start_matches("seed_");
                if !vm_names.contains(vm_name) {
                    log::info!("Cleaning up stale seed directory: {}", name);
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
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

    // Cleanup stale seed ISOs from deleted VMs
    cleanup_stale_seed_isos();

    // Shared API key state for runtime updates
    let shared_api_key: SharedApiKey = Arc::new(Mutex::new(api_key));

    // Shared VNC token store (one-time tokens)
    let vnc_tokens: VncTokenStore = Arc::new(Mutex::new(HashMap::new()));

    // Shared mounted-disk store for disk file editor
    let mounted_disks: crate::disk_edit::MountedDiskStore =
        Arc::new(Mutex::new(HashMap::new()));
    // Cleanup stale mounts from previous run
    crate::disk_edit::cleanup_stale_mounts();

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
    let mounted_disks_for_server = mounted_disks.clone();
    HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default())
            .wrap(ApiKeyAuth(api_key_for_server.clone()))
            .app_data(web::Data::new(api_key_for_server.clone()))
            .app_data(web::Data::new(vnc_tokens_for_server.clone()))
            .app_data(web::Data::new(mounted_disks_for_server.clone()))
            // Allow up to 16GB uploads for large disk images and ISOs
            .app_data(web::PayloadConfig::new(17_179_869_184))
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
            .route("/api/vm/rename", web::post().to(rename_vm_handler))
            .route("/api/vm/get/{smac}", web::get().to(get_vm_handler))
            .route("/api/vm/{smac}/mds", web::get().to(get_vm_mds_handler))
            .route("/api/vm/{smac}/mds", web::post().to(save_vm_mds_handler))
            .route("/api/vm/{smac}/phone-home", web::post().to(phone_home_handler))
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
            .route("/api/disk/flatten", web::post().to(flatten_disk_handler))
            .route("/api/disk/set-template", web::post().to(set_template_handler))
            // Disk file editor routes
            .route("/api/disk/edit-supported", web::get().to(disk_edit_supported_handler))
            .route("/api/disk/mount", web::post().to(mount_disk_handler))
            .route("/api/disk/unmount", web::post().to(unmount_disk_handler))
            .route("/api/disk/mounted", web::get().to(list_mounted_disks_handler))
            .route("/api/disk/browse/{name}", web::get().to(browse_disk_handler))
            .route("/api/disk/readfile/{name}", web::get().to(read_disk_file_handler))
            .route("/api/disk/writefile/{name}", web::post().to(write_disk_file_handler))
            // Image routes
            .route("/api/image/list", web::get().to(list_images_handler))
            .route("/api/image/upload", web::post().to(upload_image_handler))
            .route("/api/image/delete", web::post().to(delete_image_handler))
            // Disk export route
            .route("/api/disk/export/{name}", web::get().to(export_disk_handler))
            // VM export/import routes
            .route("/api/vm/export/{smac}", web::get().to(export_vm_handler))
            .route("/api/vm/import", web::post().to(import_vm_handler))
            // ISO routes
            .route("/api/iso/list", web::get().to(list_isos_handler))
            .route("/api/iso/upload", web::post().to(upload_iso_handler))
            .route("/api/iso/delete", web::post().to(delete_iso_handler))
            // Backup routes
            .route("/api/backup/list", web::get().to(list_backups_handler))
            .route("/api/backup/delete", web::post().to(delete_backup_handler))
            // Full Backup routes
            .route("/api/fullbackup/create", web::post().to(create_full_backup_handler))
            .route("/api/fullbackup/list", web::get().to(list_full_backups_handler))
            .route("/api/fullbackup/restore", web::post().to(restore_full_backup_handler))
            .route("/api/fullbackup/delete", web::post().to(delete_full_backup_handler))
            // Snapshot routes
            .route("/api/snapshot/create", web::post().to(create_snapshot_handler))
            .route("/api/snapshot/list/{vm_name}", web::get().to(list_snapshots_handler))
            .route("/api/snapshot/revert", web::post().to(revert_snapshot_handler))
            .route("/api/snapshot/delete", web::post().to(delete_snapshot_handler))
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
            // Send files to VM routes
            .route("/api/vm/sendfiles/{smac}", web::post().to(sendfiles_handler))
            .route("/api/vm/sendfiles-cleanup/{smac}", web::post().to(cleanup_sendfiles_handler))
            .route("/api/vm/guest-agent/{smac}", web::get().to(guest_agent_status_handler))
            .route("/api/vm/guestfile/{smac}", web::post().to(guest_file_write_handler))
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
