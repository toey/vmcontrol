// Desktop wrapper that hosts the vm_ctl web UI in a native WebView
// window — so users get a VMware-style app instead of having to open
// a browser tab pointed at localhost:8080. On Windows the backing
// engine is Edge WebView2 (shipped with Win10/11), on macOS it's
// WKWebView, on Linux it's WebKitGTK.
//
// The binary also starts `vm_ctl server` as a background child process
// if nothing is already listening on the target port, so a single
// double-click boots the stack.

#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

const BIND_HOST: &str = "127.0.0.1";
const BIND_PORT: u16 = 8080;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

fn port_open(host: &str, port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("{}:{}", host, port).parse().expect("socket addr"),
        Duration::from_millis(200),
    )
    .is_ok()
}

fn wait_for_port(host: &str, port: u16, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if port_open(host, port) {
            return true;
        }
        sleep(Duration::from_millis(250));
    }
    false
}

fn spawn_server() -> Option<Child> {
    // Find the vm_ctl binary next to our own exe.
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    let server_bin = {
        #[cfg(windows)]
        {
            dir.join("vm_ctl.exe")
        }
        #[cfg(not(windows))]
        {
            dir.join("vm_ctl")
        }
    };
    if !server_bin.exists() {
        eprintln!(
            "vm_ctl_app: server binary not found at {} — aborting spawn",
            server_bin.display()
        );
        return None;
    }

    let bind = format!("{}:{}", BIND_HOST, BIND_PORT);
    let mut cmd = Command::new(&server_bin);
    cmd.arg("server")
        .arg(&bind)
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        // Detach so the child keeps running if the app process later dies,
        // and hide the console window that would otherwise flash up.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    match cmd.spawn() {
        Ok(child) => {
            eprintln!(
                "vm_ctl_app: spawned {} server {} (pid {})",
                server_bin.display(),
                bind,
                child.id()
            );
            Some(child)
        }
        Err(e) => {
            eprintln!("vm_ctl_app: failed to spawn server: {}", e);
            None
        }
    }
}

fn main() -> wry::Result<()> {
    // If nothing is already listening on :8080, boot the server ourselves.
    let mut _server_child: Option<Child> = None;
    if !port_open(BIND_HOST, BIND_PORT) {
        _server_child = spawn_server();
        if !wait_for_port(BIND_HOST, BIND_PORT, STARTUP_TIMEOUT) {
            eprintln!(
                "vm_ctl_app: server did not start within {}s — opening window anyway",
                STARTUP_TIMEOUT.as_secs()
            );
        }
    }

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("VM Control Panel")
        .with_inner_size(LogicalSize::new(1440.0, 900.0))
        .with_min_inner_size(LogicalSize::new(900.0, 600.0))
        .build(&event_loop)
        .expect("window");

    let url = format!("http://{}:{}/", BIND_HOST, BIND_PORT);
    let _webview = WebViewBuilder::new()
        .with_url(&url)
        .with_accept_first_mouse(true)
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        if let Event::WindowEvent {
            event: WindowEvent::CloseRequested,
            ..
        } = event
        {
            // On close: leave the background server running so a re-launch
            // is instant. User can stop it via stop.bat / status.bat.
            *control_flow = ControlFlow::Exit;
        }
    });
}
