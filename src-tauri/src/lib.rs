use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    ActivationPolicy, Manager, WebviewUrl, WebviewWindowBuilder,
};

const RAILS_PORT: u16 = 3000;
const RAILS_HOST: &str = "127.0.0.1";
const SECRET_KEY_BASE: &str = "SECRET_KEY_BASE";
const AR_PRIMARY_KEY: &str = "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY";
const AR_DERIVATION_SALT: &str = "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT";
const AR_EXTERNAL_MARKER: &str = "ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL";

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
    File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .map_err(|error| format!("Failed to read secure random bytes: {error}"))?;

    let mut encoded = String::with_capacity(byte_count * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
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

    let bundled_ruby = resource_dir.join("ruby/bin/ruby");
    let (app_dir, command) = if bundled_ruby.is_file() {
        let bundled_app = resource_dir.join("app");
        if !bundled_app.join("bin/rails").is_file() {
            return Err(format!(
                "Bundled Ruby exists but Rails app is missing from {:?}",
                bundled_app
            ));
        }
        log::info!("Using bundled Ruby at {:?}", bundled_ruby);
        (bundled_app, RailsCommand::Bundled(bundled_ruby))
    } else {
        log::info!("Bundled Ruby not found; using the system bundle exec fallback");
        (fallback_app_dir, RailsCommand::System)
    };

    let rails_env = if cfg!(debug_assertions) {
        "development"
    } else {
        "production"
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

    // Development keeps using its existing tmp/local_secret.txt and development database.
    // Generating independent keys there could make an existing dev database unreadable.
    if !cfg!(debug_assertions) {
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
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
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Failed to resolve app data directory: {error}"))?;
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
