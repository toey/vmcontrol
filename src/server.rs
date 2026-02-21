use actix_files as fs;
use actix_web::{middleware, web, App, HttpResponse, HttpServer};

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

async fn create_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "create", operations::create).await
}

async fn copyimage_vm(body: web::Json<serde_json::Value>) -> HttpResponse {
    handle_operation(body, "copyimage", operations::copyimage).await
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
            // API routes
            .route("/api/vm/start", web::post().to(start_vm))
            .route("/api/vm/stop", web::post().to(stop_vm))
            .route("/api/vm/reset", web::post().to(reset_vm))
            .route("/api/vm/powerdown", web::post().to(powerdown_vm))
            .route("/api/vm/create", web::post().to(create_vm))
            .route("/api/vm/copyimage", web::post().to(copyimage_vm))
            .route("/api/vm/listimage", web::post().to(listimage_vm))
            .route("/api/vm/delete", web::post().to(delete_vm_handler))
            .route("/api/vm/mountiso", web::post().to(mountiso_vm))
            .route("/api/vm/unmountiso", web::post().to(unmountiso_vm))
            .route("/api/vm/livemigrate", web::post().to(livemigrate_vm))
            .route("/api/vm/backup", web::post().to(backup_vm))
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
