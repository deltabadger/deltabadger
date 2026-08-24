use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager, WebviewUrl, WebviewWindowBuilder,
};
// Tauri re-exports this only for macOS, and all three call sites are already cfg-gated — an
// unconditional import is what breaks the Windows and Linux builds.
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons};
use tauri_plugin_updater::UpdaterExt;

const RAILS_PORT: u16 = 3000;
const RAILS_HOST: &str = "127.0.0.1";
const SECRET_KEY_BASE: &str = "SECRET_KEY_BASE";
const AR_PRIMARY_KEY: &str = "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY";
const AR_DERIVATION_SALT: &str = "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT";
const AR_EXTERNAL_MARKER: &str = "ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL";

// Injected at document start in the desktop webview, before any page script and before the
// stylesheet applies, so the chrome never paints in its browser form first.
//
// The desktop class has to come from here rather than from the page. script-src carries no
// 'unsafe-inline', so a view cannot write the guard inline, and shipping it as an asset put a
// desktop-only file in front of every browser page. A user script is not governed by the page
// policy — the two flags below have always been set this way.
//
// The observer is not belt-and-braces. WKWebView and WebKitGTK inject at document start with
// <html> already created, so the first call marks it; WebView2 injects before the HTML is
// parsed at all, where documentElement is null. Waiting for DOMContentLoaded there would land
// after the stylesheet, which is the flash this script exists to prevent — so the observer
// takes the class the instant <html> is appended, still ahead of <head>.
const INITIALIZATION_SCRIPT: &str = concat!(
    "window.__TAURI_INTERNALS__ = true;",
    "window.__IS_TAURI__ = true;",
    "(function () {",
    "  function mark() {",
    "    if (!document.documentElement) { return false; }",
    "    document.documentElement.classList.add('tauri');",
    "    return true;",
    "  }",
    "  if (!mark()) {",
    "    new MutationObserver(function (records, observer) {",
    "      if (mark()) { observer.disconnect(); }",
    "    }).observe(document, { childList: true });",
    "  }",
    "})();"
);

struct RailsServer(Mutex<Option<Child>>);

enum RailsCommand {
    Bundled(PathBuf),
    System,
}

struct RailsLaunch {
    app_dir: PathBuf,
    command: RailsCommand,
    env: HashMap<String, String>,
}

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
    let url = format!("http://{}:{}/health-check", RAILS_HOST, port);
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(750))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };

    while start.elapsed() < timeout {
        if client
            .get(&url)
            .send()
            .map(|response| response.status().is_success())
            .unwrap_or(false)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

fn nonblank_environment_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn random_hex(byte_count: usize) -> Result<String, String> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Failed to generate secure random bytes: {error}"))?;

    let mut encoded = String::with_capacity(byte_count * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

#[cfg(windows)]
fn bundled_ruby_path(resource_dir: &Path) -> PathBuf {
    resource_dir.join("ruby").join("bin").join("rubyw.exe")
}

#[cfg(not(windows))]
fn bundled_ruby_path(resource_dir: &Path) -> PathBuf {
    resource_dir.join("ruby/bin/ruby")
}

#[cfg(windows)]
fn desktop_app_data_dir(_default: PathBuf) -> Result<PathBuf, String> {
    std::env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("Deltabadger"))
        .ok_or_else(|| {
            "APPDATA is not set; cannot resolve the Deltabadger data directory".to_string()
        })
}

#[cfg(not(windows))]
fn desktop_app_data_dir(default: PathBuf) -> Result<PathBuf, String> {
    Ok(default)
}

#[cfg(unix)]
fn set_secret_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Failed to chmod {:?}: {error}", path))
}

#[cfg(not(unix))]
fn set_secret_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn install_may_have_database(database_path: &Path) -> bool {
    nonblank_environment_value("DATABASE_URL").is_some()
        || nonblank_environment_value("PRIMARY_DATABASE_URL").is_some()
        || database_path.exists()
}

fn write_secrets_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Secrets path has no parent: {:?}", path))?;
    let mut temporary_path = None;

    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(".secrets.{}.{}", std::process::id(), attempt));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(contents.as_bytes())
                    .and_then(|_| file.sync_all())
                    .map_err(|error| format!("Failed to write {:?}: {error}", candidate))?;
                set_secret_permissions(&candidate)?;
                temporary_path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Failed to create a temporary secrets file: {error}"
                ))
            }
        }
    }

    let temporary_path = temporary_path
        .ok_or_else(|| "Failed to allocate a temporary secrets filename".to_string())?;
    let link_result = fs::hard_link(&temporary_path, path);
    let _ = fs::remove_file(&temporary_path);

    match link_result {
        Ok(()) => {
            log::info!("Generated secrets at {:?}", path);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            log::info!("Another process created {:?}; adopting its secrets", path);
            Ok(())
        }
        Err(error) => Err(format!(
            "Failed to install secrets file {:?}: {error}",
            path
        )),
    }
}

fn read_secrets(path: &Path) -> Result<HashMap<String, String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read secrets file {:?}: {error}", path))?;
    let mut secrets = HashMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            secrets.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Ok(secrets)
}

fn setup_secrets(
    app_data_dir: &Path,
    database_path: &Path,
) -> Result<HashMap<String, String>, String> {
    let secrets_path = app_data_dir.join(".secrets");
    let external_secret = nonblank_environment_value(SECRET_KEY_BASE);
    let external_primary = nonblank_environment_value(AR_PRIMARY_KEY);
    let external_salt = nonblank_environment_value(AR_DERIVATION_SALT);
    let encryption_pair_supplied = external_primary.is_some() || external_salt.is_some();

    if !secrets_path.exists() {
        let mut contents = String::from(
            "# Auto-generated secrets for Deltabadger\n# DO NOT DELETE - required to read encrypted data\n",
        );
        if external_secret.is_none() {
            contents.push_str(&format!("{SECRET_KEY_BASE}={}\n", random_hex(64)?));
        }

        if encryption_pair_supplied {
            contents.push_str(&format!("{AR_EXTERNAL_MARKER}=true\n"));
        } else if !install_may_have_database(database_path) {
            contents.push_str(&format!("{AR_PRIMARY_KEY}={}\n", random_hex(32)?));
            contents.push_str(&format!("{AR_DERIVATION_SALT}={}\n", random_hex(32)?));
        }

        write_secrets_atomically(&secrets_path, &contents)?;
    } else {
        log::info!("Loading existing secrets from {:?}", secrets_path);
    }
    set_secret_permissions(&secrets_path)?;

    let stored = read_secrets(&secrets_path)?;
    let mut resolved = HashMap::new();
    let secret = external_secret
        .or_else(|| {
            stored
                .get(SECRET_KEY_BASE)
                .filter(|v| !v.trim().is_empty())
                .cloned()
        })
        .ok_or_else(|| {
            format!(
                "{SECRET_KEY_BASE} is not supplied and is absent from {:?}",
                secrets_path
            )
        })?;
    resolved.insert(SECRET_KEY_BASE.to_string(), secret);

    if encryption_pair_supplied {
        if let Some(value) = external_primary {
            resolved.insert(AR_PRIMARY_KEY.to_string(), value);
        }
        if let Some(value) = external_salt {
            resolved.insert(AR_DERIVATION_SALT.to_string(), value);
        }
    } else {
        for key in [AR_PRIMARY_KEY, AR_DERIVATION_SALT] {
            if let Some(value) = stored.get(key).filter(|value| !value.trim().is_empty()) {
                resolved.insert(key.to_string(), value.clone());
            }
        }
    }

    if let Some(value) = stored.get(AR_EXTERNAL_MARKER) {
        resolved.insert(AR_EXTERNAL_MARKER.to_string(), value.clone());
    }
    // Old desktop secrets may contain this migration-only key. Preserve it, but never mint it.
    if nonblank_environment_value("APP_ENCRYPTION_KEY").is_none() {
        if let Some(value) = stored.get("APP_ENCRYPTION_KEY") {
            resolved.insert("APP_ENCRYPTION_KEY".to_string(), value.clone());
        }
    }

    Ok(resolved)
}

fn prepare_launch(
    fallback_app_dir: PathBuf,
    resource_dir: PathBuf,
    app_data_dir: PathBuf,
    port: u16,
) -> Result<RailsLaunch, String> {
    let database_dir = app_data_dir.join("db");
    let temporary_dir = app_data_dir.join("tmp");
    let cache_dir = temporary_dir.join("cache");
    for directory in [&app_data_dir, &database_dir, &temporary_dir, &cache_dir] {
        fs::create_dir_all(directory).map_err(|error| {
            format!(
                "Failed to create writable directory {:?}: {error}",
                directory
            )
        })?;
    }

    let bundled_ruby = bundled_ruby_path(&resource_dir);
    let (app_dir, command, rails_env) = if bundled_ruby.is_file() {
        let bundled_app = resource_dir.join("app");
        if !bundled_app.join("bin/rails").is_file() {
            return Err(format!(
                "Bundled Ruby exists but Rails app is missing from {:?}",
                bundled_app
            ));
        }
        log::info!("Using bundled Ruby at {:?}", bundled_ruby);
        (
            bundled_app,
            RailsCommand::Bundled(bundled_ruby),
            "production",
        )
    } else {
        log::info!("Bundled Ruby not found; using the system bundle exec fallback");
        (fallback_app_dir, RailsCommand::System, "development")
    };

    let database_path = database_dir.join("production.sqlite3");
    let mut env = HashMap::from([
        ("RAILS_ENV".to_string(), rails_env.to_string()),
        ("PORT".to_string(), port.to_string()),
        ("RAILS_LOG_TO_STDOUT".to_string(), "true".to_string()),
        ("RAILS_SERVE_STATIC_FILES".to_string(), "1".to_string()),
        ("SOLID_QUEUE_IN_PUMA".to_string(), "true".to_string()),
        ("RAILS_MAX_THREADS".to_string(), "1".to_string()),
        (
            "APP_ROOT_URL".to_string(),
            format!("http://{}:{}", RAILS_HOST, port),
        ),
        (
            "DATABASE_PATH".to_string(),
            database_path.to_string_lossy().into_owned(),
        ),
        (
            "QUEUE_DATABASE_PATH".to_string(),
            database_dir
                .join("production_queue.sqlite3")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "CACHE_DATABASE_PATH".to_string(),
            database_dir
                .join("production_cache.sqlite3")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "CABLE_DATABASE_PATH".to_string(),
            database_dir
                .join("production_cable.sqlite3")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "PIDFILE".to_string(),
            temporary_dir
                .join("server.pid")
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "BOOTSNAP_CACHE_DIR".to_string(),
            cache_dir.to_string_lossy().into_owned(),
        ),
        (
            "APP_TMP_DIR".to_string(),
            temporary_dir.to_string_lossy().into_owned(),
        ),
    ]);

    // The system fallback is the development loop: it keeps using its existing
    // tmp/local_secret.txt and development database. Generating independent keys there could
    // make an existing dev database unreadable. A bundled Ruby is production by construction,
    // including when the desktop executable was compiled with debug assertions.
    if rails_env == "production" {
        env.extend(setup_secrets(&app_data_dir, &database_path)?);
    }

    if matches!(&command, RailsCommand::Bundled(_)) {
        let bundle_path = app_dir.join("vendor/bundle").to_string_lossy().into_owned();
        env.insert(
            "BUNDLE_GEMFILE".to_string(),
            app_dir.join("Gemfile").to_string_lossy().into_owned(),
        );
        env.insert("GEM_HOME".to_string(), bundle_path.clone());
        env.insert("BUNDLE_PATH".to_string(), bundle_path);
    }

    Ok(RailsLaunch {
        app_dir,
        command,
        env,
    })
}

fn rails_command(launch: &RailsLaunch, arguments: &[&str]) -> Command {
    let mut command = match &launch.command {
        RailsCommand::Bundled(ruby) => {
            let mut command = Command::new(ruby);
            command.arg(launch.app_dir.join("bin/rails"));
            command
        }
        RailsCommand::System if cfg!(target_os = "windows") => {
            let mut command = Command::new("ruby");
            command.arg("bin/rails");
            command
        }
        RailsCommand::System => {
            let mut command = Command::new("bundle");
            command.args(["exec", "rails"]);
            command
        }
    };
    command
        .args(arguments)
        .current_dir(&launch.app_dir)
        .envs(&launch.env)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    command
}

fn run_migrations(launch: &RailsLaunch) -> Result<(), String> {
    log::info!("Preparing databases...");

    let status = rails_command(launch, &["db:prepare"])
        .status()
        .map_err(|e| format!("Failed to prepare databases: {}", e))?;

    if status.success() {
        log::info!("Database preparation completed successfully");
        Ok(())
    } else {
        Err(format!(
            "Database preparation failed with exit code: {}",
            status.code().unwrap_or(-1)
        ))
    }
}

fn start_rails_server(launch: &RailsLaunch, port: u16) -> Result<Child, String> {
    log::info!("Starting Rails server from: {:?}", launch.app_dir);
    log::info!("Rails will listen on port: {}", port);

    let port = port.to_string();
    let mut cmd = rails_command(launch, &["server", "-p", &port, "-b", RAILS_HOST]);

    // On Windows, prevent console window
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to start Rails server: {}", e))
}

fn stop_rails_server<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let state: tauri::State<RailsServer> = app.state();
    let Ok(mut guard) = state.0.lock() else {
        log::error!("Rails server state lock is poisoned; unable to stop it cleanly");
        return;
    };
    let Some(mut child) = guard.take() else {
        return;
    };

    log::info!("Shutting down Rails server (PID {})...", child.id());
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        for _ in 0..25 {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}

async fn check_for_updates(app: tauri::AppHandle) -> Result<(), String> {
    let Some(update) = app
        .updater()
        .map_err(|error| format!("Failed to initialize updater: {error}"))?
        .check()
        .await
        .map_err(|error| format!("Failed to check for updates: {error}"))?
    else {
        log::info!("Deltabadger is up to date");
        return Ok(());
    };

    let version = update.version.clone();
    let (answer, answered) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(format!(
            "Deltabadger {version} is available. Install it now and restart?"
        ))
        .title("Update available")
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install and restart".to_string(),
            "Later".to_string(),
        ))
        .show(move |install| {
            let _ = answer.send(install);
        });

    if !answered
        .await
        .map_err(|error| format!("Update prompt was closed unexpectedly: {error}"))?
    {
        log::info!("Update {version} deferred by the user");
        return Ok(());
    }

    log::info!("Downloading and installing update {version}");
    update
        .download_and_install(
            |chunk_length, content_length| {
                log::debug!(
                    "Downloaded {chunk_length} update bytes (total size: {content_length:?})"
                );
            },
            || log::info!("Update download finished"),
        )
        .await
        .map_err(|error| format!("Failed to install update {version}: {error}"))?;

    log::info!("Update {version} installed; restarting");
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
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
                // In development, find project root relative to executable
                // When run via .app bundle: exe is in target/debug/bundle/macos/Deltabadger.app/Contents/MacOS/
                // When run via cargo/tauri dev: current_dir is project root
                let exe_path = std::env::current_exe().ok();
                let from_bundle = exe_path.as_ref().map(|p| {
                    p.ancestors()
                        .find(|a| a.ends_with("src-tauri"))
                        .map(|p| p.parent().unwrap().to_path_buf())
                }).flatten();

                from_bundle.unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                })
            } else {
                // In production, resources are bundled
                app.path()
                    .resource_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."))
            };

            // Find an available port
            let port = find_available_port(RAILS_PORT);
            log::info!("Using port: {}", port);

            let resource_dir = app
                .path()
                .resource_dir()
                .map_err(|error| format!("Failed to resolve resource directory: {error}"))?;
            let default_app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
            let app_data_dir = desktop_app_data_dir(default_app_data_dir)?;
            let launch = prepare_launch(app_dir, resource_dir, app_data_dir, port)?;
            log::info!("App directory: {:?}", launch.app_dir);

            // Run pending migrations before starting the server
            if let Err(e) = run_migrations(&launch) {
                log::error!("Migration error: {}", e);
                return Err(e.into());
            }

            // Start the Rails server
            match start_rails_server(&launch, port) {
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
                        let window_builder = WebviewWindowBuilder::new(
                            app,
                            "main",
                            WebviewUrl::External(url.parse().unwrap()),
                        )
                        .title("Deltabadger")
                        .inner_size(1280.0, 800.0)
                        .min_inner_size(320.0, 600.0)
                        .center()
                        .devtools(true)
                        .initialization_script(INITIALIZATION_SCRIPT);

                        // The overlay title bar is the macOS look; both builder methods exist only
                        // on macOS in Tauri 2, so calling them unconditionally does not compile
                        // anywhere else. Other platforms get their native title bar.
                        #[cfg(target_os = "macos")]
                        let window_builder = window_builder
                            .title_bar_style(tauri::TitleBarStyle::Overlay)
                            .hidden_title(true);

                        window_builder.build()?;

                        // Set up system tray
                        let show_item = MenuItemBuilder::with_id("show", "Show Deltabadger").build(app)?;
                        let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
                        let tray_menu = MenuBuilder::new(app)
                            .item(&show_item)
                            .separator()
                            .item(&quit_item)
                            .build()?;

                        let tray_icon = Image::from_path("icons/tray-icon.png")
                            .unwrap_or_else(|_| Image::from_bytes(include_bytes!("../icons/tray-icon.png")).unwrap());

                        let _tray = TrayIconBuilder::new()
                            .icon(tray_icon)
                            .icon_as_template(true)
                            .menu(&tray_menu)
                            .tooltip("Deltabadger")
                            .on_menu_event(|app, event| {
                                match event.id().as_ref() {
                                    "show" => {
                                        // Show in Dock when window is shown
                                        #[cfg(target_os = "macos")]
                                        let _ = app.set_activation_policy(ActivationPolicy::Regular);

                                        if let Some(window) = app.get_webview_window("main") {
                                            let _ = window.show();
                                            let _ = window.set_focus();
                                        }
                                    }
                                    "quit" => {
                                        stop_rails_server(app);
                                        app.exit(0);
                                    }
                                    _ => {}
                                }
                            })
                            .on_tray_icon_event(|tray, event| {
                                if let tauri::tray::TrayIconEvent::Click { button, .. } = event {
                                    if button == tauri::tray::MouseButton::Left {
                                        let app = tray.app_handle();

                                        // Show in Dock when window is shown
                                        #[cfg(target_os = "macos")]
                                        let _ = app.set_activation_policy(ActivationPolicy::Regular);

                                        if let Some(window) = app.get_webview_window("main") {
                                            let _ = window.show();
                                            let _ = window.set_focus();
                                        }
                                    }
                                }
                            })
                            .build(app)?;

                        log::info!("System tray initialized");
                    } else {
                        log::error!("Rails server failed to start within timeout");
                        stop_rails_server(app.handle());
                        return Err("Rails server failed to start".into());
                    }
                }
                Err(e) => {
                    log::error!("Failed to start Rails server: {}", e);
                    return Err(e.into());
                }
            }

            // Packaged apps check after startup so development never contacts the
            // production update endpoint or requires a real updater public key.
            if !cfg!(debug_assertions) {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = check_for_updates(app_handle).await {
                        log::error!("{error}");
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hide the window instead of closing (app stays in tray)
                let _ = window.hide();
                api.prevent_close();

                // Hide from Dock when window is closed
                #[cfg(target_os = "macos")]
                let _ = window.app_handle().set_activation_policy(ActivationPolicy::Accessory);

                log::info!("Window hidden, app running in tray");
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                stop_rails_server(app);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock should be after the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "deltabadger-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn create_file(path: &Path) {
        fs::create_dir_all(path.parent().expect("test file should have a parent")).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn bundled_ruby_launches_rails_in_production_even_in_a_debug_build() {
        let root = temporary_directory("bundled-launch");
        let fallback_app_dir = root.join("fallback-app");
        let resource_dir = root.join("resources");
        let app_data_dir = root.join("data");
        create_file(&bundled_ruby_path(&resource_dir));
        create_file(&resource_dir.join("app/bin/rails"));

        let launch = prepare_launch(fallback_app_dir, resource_dir.clone(), app_data_dir.clone(), 3010)
            .expect("bundled launch should be prepared");

        assert!(matches!(launch.command, RailsCommand::Bundled(_)));
        assert_eq!(launch.app_dir, resource_dir.join("app"));
        assert_eq!(launch.env.get("RAILS_ENV").map(String::as_str), Some("production"));
        assert_eq!(
            launch.env.get("DATABASE_PATH").map(String::as_str),
            Some(app_data_dir.join("db/production.sqlite3").to_string_lossy().as_ref())
        );
        assert!(app_data_dir.join(".secrets").is_file());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn system_bundle_fallback_keeps_the_development_environment() {
        let root = temporary_directory("system-launch");
        let fallback_app_dir = root.join("fallback-app");
        let resource_dir = root.join("resources");
        let app_data_dir = root.join("data");

        let launch = prepare_launch(fallback_app_dir.clone(), resource_dir, app_data_dir.clone(), 3011)
            .expect("system launch should be prepared");

        assert!(matches!(launch.command, RailsCommand::System));
        assert_eq!(launch.app_dir, fallback_app_dir);
        assert_eq!(launch.env.get("RAILS_ENV").map(String::as_str), Some("development"));
        assert!(!app_data_dir.join(".secrets").exists());

        fs::remove_dir_all(root).unwrap();
    }

    // The desktop chrome hangs off html.tauri, and this script is the only thing that sets it:
    // the web layouts ship no tauri asset, and the page policy forbids an inline block. If this
    // string loses the class, the packaged app silently paints as a browser page.
    #[test]
    fn the_initialization_script_marks_the_document_as_desktop() {
        assert!(INITIALIZATION_SCRIPT.contains("window.__IS_TAURI__ = true;"));
        assert!(INITIALIZATION_SCRIPT.contains("window.__TAURI_INTERNALS__ = true;"));
        assert!(INITIALIZATION_SCRIPT.contains("classList.add('tauri')"));
        // The null-documentElement path is the Windows one, and it is the path that cannot be
        // exercised from here — so what it hangs on is pinned instead.
        assert!(INITIALIZATION_SCRIPT.contains("new MutationObserver"));
        assert!(!INITIALIZATION_SCRIPT.contains("DOMContentLoaded"));
    }
}
