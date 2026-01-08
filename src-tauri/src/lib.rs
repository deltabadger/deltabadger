use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

const RAILS_PORT: u16 = 3000;
const RAILS_HOST: &str = "127.0.0.1";

struct RailsServer(Mutex<Option<Child>>);

fn find_available_port(start: u16) -> u16 {
    for port in start..start + 100 {
        if port_check::is_port_reachable_with_timeout(
            format!("{}:{}", RAILS_HOST, port),
            Duration::from_millis(100),
        ) == false
        {
            return port;
        }
    }
    start
}

fn wait_for_server(port: u16, timeout_secs: u64) -> bool {
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(timeout_secs);

    while start.elapsed() < timeout {
        if port_check::is_port_reachable_with_timeout(
            format!("{}:{}", RAILS_HOST, port),
            Duration::from_millis(500),
        ) {
            // Give Rails a moment to fully initialize
            thread::sleep(Duration::from_millis(500));
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

fn start_rails_server(app_dir: &std::path::Path, port: u16) -> Result<Child, String> {
    log::info!("Starting Rails server from: {:?}", app_dir);
    log::info!("Rails will listen on port: {}", port);

    // Set up environment for Rails
    let rails_env = if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
    };

    // Try to find the rails executable
    let rails_cmd = if cfg!(target_os = "windows") {
        "ruby"
    } else {
        "bundle"
    };

    let mut cmd = Command::new(rails_cmd);

    if cfg!(target_os = "windows") {
        cmd.args(["bin/rails", "server"]);
    } else {
        cmd.args(["exec", "rails", "server"]);
    }

    cmd.args(["-p", &port.to_string(), "-b", RAILS_HOST])
        .current_dir(app_dir)
        .env("RAILS_ENV", rails_env)
        .env("PORT", port.to_string())
        .env("RAILS_LOG_TO_STDOUT", "true")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    // On Windows, prevent console window
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    cmd.spawn().map_err(|e| format!("Failed to start Rails server: {}", e))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(RailsServer(Mutex::new(None)))
        .setup(|app| {
            // Set up logging in debug mode
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // Get the app directory (where Rails app lives)
            let app_dir = if cfg!(debug_assertions) {
                // In development, use the project root
                std::env::current_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
            } else {
                // In production, resources are bundled
                app.path()
                    .resource_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
            };

            log::info!("App directory: {:?}", app_dir);

            // Find an available port
            let port = find_available_port(RAILS_PORT);
            log::info!("Using port: {}", port);

            // Start the Rails server
            match start_rails_server(&app_dir, port) {
                Ok(child) => {
                    log::info!("Rails server process started with PID: {}", child.id());

                    // Store the child process handle
                    let state: tauri::State<RailsServer> = app.state();
                    *state.0.lock().unwrap() = Some(child);

                    // Wait for server to be ready
                    log::info!("Waiting for Rails server to be ready...");
                    if wait_for_server(port, 60) {
                        log::info!("Rails server is ready!");

                        // Create the main window pointing to Rails
                        let url = format!("http://{}:{}", RAILS_HOST, port);
                        WebviewWindowBuilder::new(
                            app,
                            "main",
                            WebviewUrl::External(url.parse().unwrap()),
                        )
                        .title("Deltabadger")
                        .title_bar_style(tauri::TitleBarStyle::Overlay)
                        .hidden_title(true)
                        .inner_size(1280.0, 800.0)
                        .min_inner_size(320.0, 600.0)
                        .center()
                        .devtools(true)
                        .initialization_script("window.__TAURI_INTERNALS__ = true; window.__IS_TAURI__ = true;")
                        .build()?;
                    } else {
                        log::error!("Rails server failed to start within timeout");
                        return Err("Rails server failed to start".into());
                    }
                }
                Err(e) => {
                    log::error!("Failed to start Rails server: {}", e);
                    return Err(e.into());
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                // Shutdown Rails server when window closes
                let app = window.app_handle();
                let state: tauri::State<RailsServer> = app.state();
                let mut guard = state.0.lock().unwrap();
                if let Some(ref mut child) = *guard {
                    log::info!("Shutting down Rails server...");
                    let _ = child.kill();
                    let _ = child.wait();
                    log::info!("Rails server shut down");
                }
                *guard = None;
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
