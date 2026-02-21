use actix_files as fs;
use actix_web::{middleware, web, App, HttpResponse, HttpServer};
use actix_web::web::Bytes;

use crate::config::get_conf;
use crate::mds;
use crate::models::ApiResponse;
use crate::operations;

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
    match crate::db::get_vm(&smac) {
        Ok(vm) => {
            let mut config: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
            config["mds"] = body.into_inner();
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

async fn list_vms_handler() -> HttpResponse {
    // Auto-backfill VNC ports for VMs that don't have one
    if let Ok(vms) = crate::db::list_vms() {
        let mut used_ports: Vec<u16> = Vec::new();
        let mut need_port: Vec<String> = Vec::new();
        for vm in &vms {
            if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&vm.config) {
                if let Some(p) = cfg.get("vnc_port").and_then(|v| v.as_u64()) {
                    used_ports.push(p as u16);
                } else {
                    need_port.push(vm.smac.clone());
                }
            } else {
                need_port.push(vm.smac.clone());
            }
        }
        let mut next_port: u16 = 12001;
        for smac in &need_port {
            while used_ports.contains(&next_port) {
                next_port += 1;
            }
            // Update config with new vnc_port
            if let Ok(vm) = crate::db::get_vm(smac) {
                let mut cfg: serde_json::Value = serde_json::from_str(&vm.config).unwrap_or_default();
                cfg["vnc_port"] = serde_json::json!(next_port);
                let _ = crate::db::update_vm(smac, &serde_json::to_string(&cfg).unwrap_or_default());
            }
            used_ports.push(next_port);
            next_port += 1;
        }
    }

    match crate::db::list_vms() {
        Ok(vms) => HttpResponse::Ok().json(vms),
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
    if safe_name.is_empty() || !safe_name.ends_with(".iso") {
        return HttpResponse::BadRequest().json(ApiResponse {
            success: false,
            message: "Invalid filename (must end with .iso)".into(),
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
    if safe_name.is_empty() {
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

    // Convert to qcow2
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
                    // Find the timestamp part (last 2 underscore-separated segments)
                    let parts: Vec<&str> = base.rsplitn(3, '_').collect();
                    let (vm_name, datetime) = if parts.len() >= 3 {
                        // parts[0]=HHMMSS, parts[1]=YYYYMMDD, parts[2]=vmname
                        let dt = format!("{}-{}-{} {}:{}:{}",
                            &parts[1][0..4], &parts[1][4..6], &parts[1][6..8],
                            &parts[0][0..2], &parts[0][2..4], &parts[0][4..6]);
                        (parts[2].to_string(), dt)
                    } else {
                        // Old format: vmname.gz (no timestamp)
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

pub async fn start_server(bind_addr: &str) -> std::io::Result<()> {
    env_logger::init();

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
    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            // Allow up to 4GB uploads for ISO files
            .app_data(web::PayloadConfig::new(4_294_967_296))
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
            // Disk routes
            .route("/api/disk/list", web::get().to(list_disks_handler))
            .route("/api/disk/create", web::post().to(create_disk_handler))
            .route("/api/disk/delete", web::post().to(delete_disk_handler))
            // Image routes
            .route("/api/image/list", web::get().to(list_images_handler))
            .route("/api/image/upload", web::post().to(upload_image_handler))
            .route("/api/image/delete", web::post().to(delete_image_handler))
            // ISO routes
            .route("/api/iso/list", web::get().to(list_isos_handler))
            .route("/api/iso/upload", web::post().to(upload_iso_handler))
            .route("/api/iso/delete", web::post().to(delete_iso_handler))
            // Backup routes
            .route("/api/backup/list", web::get().to(list_backups_handler))
            .route("/api/backup/delete", web::post().to(delete_backup_handler))
            // VNC routes
            .route("/api/vnc/start", web::post().to(vnc_start_handler))
            .route("/api/vnc/stop", web::post().to(vnc_stop_handler))
            // MDS routes
            .configure(mds::configure_mds_routes)
            // Static files (must be last - catch-all)
            .service(fs::Files::new("/", "./static").index_file("index.html"))
    })
    .bind(bind_addr)?
    .run()
    .await
}
