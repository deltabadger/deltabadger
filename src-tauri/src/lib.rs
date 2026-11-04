//! The desktop app: a window, a tray, and the engine (`deltabadger-engine serve`) as its child (PLAN-B6). `setup`
//! shows the window on the launcher's status page (ui/index.html) and returns at once; everything that can fail runs
//! on the supervisor's thread (supervisor.rs), which moves the window to the engine's page once it answers.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};
// Tauri re-exports this only for macOS, and all three call sites are already cfg-gated — an
// unconditional import is what breaks the Windows and Linux builds.
#[cfg(target_os = "macos")]
use tauri::ActivationPolicy;

mod secrets;
pub mod supervisor;
pub mod update;

use supervisor::{stop_engine, Engine, Launch, Page, View};
use update::{Agent, Snapshot};

/// The released app's identifier: the only one that runs on the 2.x data dir (PLAN-B6 r2.2). Test builds use
/// `com.deltabadger.app.test`, `tauri dev` `com.deltabadger.app.dev`.
pub const RELEASE_IDENTIFIER: &str = "com.deltabadger.app";

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

/// The identifier a build runs under (PLAN-B6 r1.12): a debug build never runs as the released app, however it was
/// started (`tauri dev` without bin/tauri-dev included), so it never meets an installed app's single instance.
pub fn run_identifier(configured: &str, debug: bool) -> String {
    if debug && configured == RELEASE_IDENTIFIER { format!("{RELEASE_IDENTIFIER}.dev") } else { configured.to_string() }
}

/// The data dir (PLAN-B6 §1.2, r2.2, r4.3): `DELTABADGER_DESKTOP_DATA_DIR` for any build but the released app's
/// identifier; else a debug build's `<repo>/storage/desktop-dev/`; else the 2.x desktop's dir for the released app,
/// and the identifier's own app data dir for every other one. With the lines the log gets about the choice.
pub fn choose_data_dir(
    identifier: &str,
    override_dir: Option<&str>,
    debug: bool,
    identifier_dir: Result<PathBuf, String>,
) -> Result<(PathBuf, Vec<String>), String> {
    let mut notes = Vec::new();
    if let Some(dir) = override_dir.filter(|d| !d.trim().is_empty()) {
        if identifier == RELEASE_IDENTIFIER {
            notes.push("desktop: DELTABADGER_DESKTOP_DATA_DIR is ignored by the released app".to_string());
        } else {
            let dir = PathBuf::from(dir);
            if !dir.is_absolute() {
                return Err(format!("DELTABADGER_DESKTOP_DATA_DIR must be an absolute path: {}", dir.display()));
            }
            return Ok((dir, notes));
        }
    }
    let dir = if debug {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or(Path::new("."))
            .join("storage")
            .join("desktop-dev")
    } else if identifier == RELEASE_IDENTIFIER {
        desktop_app_data_dir(identifier_dir?)?
    } else {
        identifier_dir?
    };
    Ok((dir, notes))
}

/// The sidecar next to the app's executable (Tauri's externalBin). Debug builds: `DELTABADGER_TEST_ENGINE` names
/// another program (a test fixture served in its place).
fn engine_exe() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(exe) = std::env::var_os("DELTABADGER_TEST_ENGINE").filter(|e| !e.is_empty()) {
        return Ok(exe.into());
    }
    let exe = std::env::current_exe().map_err(|error| format!("Failed to find the app's executable: {error}"))?;
    let dir = exe.parent().ok_or_else(|| format!("{:?} has no folder", exe))?;
    Ok(dir.join(format!("deltabadger-engine{}", std::env::consts::EXE_SUFFIX)))
}

/// Where a download from the engine's pages goes (PLAN-B6 §7, r5.1): the Downloads folder, under the name the page
/// gave it, numbered when that name is taken (`report (1).csv`). WKWebView saves nothing without a destination.
pub fn download_destination(downloads: &Path, suggested: &Path) -> PathBuf {
    let name = suggested.file_name().map_or_else(|| "download".into(), |n| n.to_string_lossy().into_owned());
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_string(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    let mut path = downloads.join(&name);
    let mut n = 1;
    while path.exists() {
        path = downloads.join(format!("{stem} ({n}){extension}"));
        n += 1;
    }
    path
}

/// The log file, once the data dir is known.
struct LogFile(PathBuf);

fn log_line<R: tauri::Runtime>(app: &tauri::AppHandle<R>, line: &str) {
    if let Some(log) = app.try_state::<LogFile>() {
        let _ = supervisor::note(&log.0, line);
    }
}

/// Every quit (B6's path): an install that hasn't begun its replacement step gives way (PLAN-B7 r1.2), then the
/// engine's stdin is closed.
fn quit(app: &tauri::AppHandle) {
    if let Some(updates) = app.try_state::<Updates>() {
        updates.agent.quit();
    }
    stop_engine(&app.state::<Arc<Engine>>());
}

fn show_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    // Show in Dock when window is shown
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(ActivationPolicy::Regular);

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let visible = window.is_visible().unwrap_or(false);
        log_line(app, &format!("desktop: window shown (visible: {visible})"));
    }
}

/// The status page's URL: the app's own origin, which serves ui/index.html at its root (frontendDist). Not read from the
/// window, which may still be at about:blank right after it was created (WebView2 navigates asynchronously).
fn status_page() -> tauri::Url {
    let origin = if cfg!(windows) { "http://tauri.localhost/" } else { "tauri://localhost/" };
    origin.parse().expect("a fixed URL")
}

/// The main window, for the supervisor.
struct WindowView {
    app: tauri::AppHandle,
    /// The status page's URL, without a fragment.
    status: tauri::Url,
    /// The engine page the status page replaced (r2.3).
    last: Mutex<Option<tauri::Url>>,
}

impl WindowView {
    fn navigate(&self, url: tauri::Url) {
        if let Some(window) = self.app.get_webview_window("main") {
            let _ = window.navigate(url);
        }
    }
}

impl View for WindowView {
    fn status(&self, fragment: &str) {
        let Some(window) = self.app.get_webview_window("main") else { return };
        if let Ok(current) = window.url() {
            if current.scheme() == "http" && current.host_str() == Some(supervisor::HOST) {
                *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(current);
            }
        }
        let mut url = self.status.clone();
        url.set_fragment(Some(fragment));
        let _ = window.navigate(url);
    }

    fn open(&self, url: &str) {
        if let Ok(url) = url.parse() {
            *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
            self.navigate(url);
        }
    }

    fn back(&self) {
        let last = self.last.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(url) = last {
            self.navigate(url);
        }
    }
}

/// Where the main window may go (PLAN-B7 r1.8): the status page and this launch's engine; any other web page opens in
/// the browser; nothing else loads.
#[derive(Debug, PartialEq, Eq)]
pub enum Navigation {
    Allow,
    Browser,
    Refuse,
}

pub fn navigation(url: &tauri::Url, engine_port: Option<u16>) -> Navigation {
    let status = url.scheme() == "tauri" || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"));
    let engine = url.scheme() == "http" && url.host_str() == Some(supervisor::HOST) && engine_port.is_some_and(|p| url.port() == Some(p));
    if status || engine || url.as_str() == "about:blank" {
        Navigation::Allow
    } else if matches!(url.scheme(), "http" | "https") {
        Navigation::Browser
    } else {
        Navigation::Refuse
    }
}

/// The guard on every command (PLAN-B7 r1.8, r3.4): the caller is the main window on this launch's engine origin, and
/// its document carries this launch's token (`<meta name="desktop-launch">`, which only the engine this launch started
/// renders). A page on another port, or another listener on the engine's port, has neither.
pub fn allowed(label: &str, url: &tauri::Url, launch: Option<(u16, String)>, token: &str) -> Result<(), &'static str> {
    let Some((port, own)) = launch.filter(|_| label == "main") else { return Err("no engine page") };
    if url.scheme() != "http" || url.host_str() != Some(supervisor::HOST) || url.port() != Some(port) {
        return Err("not this engine's origin");
    }
    if token.is_empty() || token != own {
        return Err("not this launch's token");
    }
    Ok(())
}

fn guard(app: &tauri::AppHandle, webview: &tauri::Webview, token: &str) -> Result<(), String> {
    let url = webview.url().map_err(|e| e.to_string())?;
    allowed(webview.label(), &url, app.state::<Arc<Engine>>().launch(), token).map_err(|why| {
        let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
        log_line(app, &format!("desktop: refused a command from {}://{}{port}: {why}", url.scheme(), url.host_str().unwrap_or("")));
        "refused".to_string()
    })
}

/// The update agent and what an install needs (the host exists once the data dir is known).
struct Updates {
    agent: Arc<Agent>,
    config: update::Config,
    host: Option<Arc<AppHost>>,
}

/// The app, as the install sees it.
struct AppHost {
    app: tauri::AppHandle,
    engine: Arc<Engine>,
    launch: Arc<Launch>,
    view: Arc<WindowView>,
    agent: Arc<Agent>,
    fault: Option<String>,
    /// The supervisor thread of the engine started last: a respawn waits for it to end first, or two supervisors would
    /// follow one engine.
    supervisor: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl update::Host for AppHost {
    fn show(&self, page: &str) { self.view.status(page) }

    fn stop_and_wait(&self, cancelled: &dyn Fn() -> bool) -> bool { supervisor::stop_and_wait(&self.engine, cancelled) }

    fn bundle_lock(&self) -> Result<Option<Box<dyn std::any::Any>>, String> {
        let env = |k: &str| std::env::var(k).ok();
        let exe = std::env::current_exe().and_then(|e| e.canonicalize()).map_err(|e| e.to_string())?;
        let app = update::install::installed_app(&exe, &env).ok_or("not an installed app")?;
        let dir = update::install::lock_dir(&env).ok_or("no folder for the update lock")?;
        let file = update::install::open_lock(&dir.join(update::install::lock_name(&app)))?;
        Ok(update::install::try_exclusive(file)?.map(|f| Box::new(f) as Box<dyn std::any::Any>))
    }

    fn install(&self, package: &update::Package) -> Result<(), update::install::Failure> {
        let bytes = &package.bytes[..];
        let exe = std::env::current_exe().and_then(|e| e.canonicalize())?;
        let app = update::install::installed_app(&exe, &|k| std::env::var(k).ok())
            .ok_or_else(|| update::install::Failure { reason: "not an installed app".into(), space: false })?;
        let log = |line: &str| self.log(line);
        #[cfg(target_os = "macos")]
        return update::install::install_macos(&app, bytes, self.fault.as_deref(), &log);
        #[cfg(target_os = "linux")]
        return update::install::install_appimage(&app, bytes, self.fault.as_deref(), &log);
        #[cfg(windows)]
        {
            let _ = (app, log);
            return update::install::launch_windows_installer(bytes, &self.launch.data, &package.version, true, self.fault.as_deref());
        }
    }

    fn respawn(&self) {
        let mut supervisor = self.supervisor.lock().unwrap_or_else(|e| e.into_inner());
        let view: Arc<dyn View> = self.view.clone();
        *supervisor = supervisor::restart(self.launch.clone(), self.engine.clone(), view, supervisor.take(), &|| self.agent.quitting());
    }

    fn relaunch(&self) {
        // Windows: the installer relaunches the app itself (/R).
        if cfg!(windows) { self.app.exit(0) } else { self.app.request_restart() }
    }

    fn log(&self, line: &str) { log_line(&self.app, line) }
}

fn updates(app: &tauri::AppHandle) -> (Arc<Agent>, update::Config, Option<Arc<AppHost>>) {
    let updates = app.state::<Updates>();
    (updates.agent.clone(), updates.config.clone(), updates.host.clone())
}

async fn run_check(app: &tauri::AppHandle, button: bool) -> Snapshot {
    let (agent, config, _) = updates(app);
    let log = |line: &str| log_line(app, line);
    update::check(app, &agent, &config, button, &log).await
}

fn start_install(app: &tauri::AppHandle) -> Result<Snapshot, String> {
    let (agent, _, host) = updates(app);
    let host = host.ok_or("the app can't install here")?;
    let package = agent.begin_install()?;
    log_line(app, "desktop: update: install asked for");
    std::thread::spawn(move || host.agent.run_install(&*host, &package, &update::Timing::APP));
    Ok(agent.snapshot())
}

#[tauri::command]
fn update_state(app: tauri::AppHandle, webview: tauri::Webview, token: String) -> Result<Snapshot, String> {
    guard(&app, &webview, &token)?;
    Ok(updates(&app).0.snapshot())
}

/// The Settings button: a check now; the state after it.
#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle, webview: tauri::Webview, token: String) -> Result<Snapshot, String> {
    guard(&app, &webview, &token)?;
    Ok(run_check(&app, true).await)
}

/// "Restart to update".
#[tauri::command]
fn install_update(app: tauri::AppHandle, webview: tauri::Webview, token: String) -> Result<Snapshot, String> {
    guard(&app, &webview, &token)?;
    start_install(&app)
}

/// "Try again" (r1.11): the install again after an install failure, else a new check and download.
#[tauri::command]
async fn retry_update(app: tauri::AppHandle, webview: tauri::Webview, token: String) -> Result<Snapshot, String> {
    guard(&app, &webview, &token)?;
    if updates(&app).0.retry_installs() {
        start_install(&app)
    } else {
        Ok(run_check(&app, true).await)
    }
}

/// Debug builds: a page script's report into the log (the proofs read what the window saw from there).
#[cfg(debug_assertions)]
#[tauri::command]
fn test_report(app: tauri::AppHandle, text: String) {
    log_line(&app, &format!("test: {text}"));
}

/// The document-start script: the desktop marks, and in debug builds `DELTABADGER_TEST_EVAL` after them.
fn initialization_script() -> String {
    #[cfg(debug_assertions)]
    if let Ok(eval) = std::env::var("DELTABADGER_TEST_EVAL") {
        return format!("{INITIALIZATION_SCRIPT}\n{eval}");
    }
    INITIALIZATION_SCRIPT.to_string()
}

/// Everything the supervisor needs, or why it can't run.
fn launch(app: &tauri::App) -> Result<Launch, String> {
    let identifier = app.config().identifier.clone();
    let override_dir = std::env::var("DELTABADGER_DESKTOP_DATA_DIR").ok();
    let identifier_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"));
    let (data, notes) = choose_data_dir(&identifier, override_dir.as_deref(), cfg!(debug_assertions), identifier_dir)?;
    let cwd = app
        .path()
        .resource_dir()
        .map_err(|error| format!("Failed to resolve resource directory: {error}"))?;
    Ok(Launch { exe: engine_exe()?, extra_args: Vec::new(), cwd, data, notes, first_port: None })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut context = tauri::generate_context!();
    // Before the single-instance plugin reads it.
    let identifier = run_identifier(&context.config().identifier, cfg!(debug_assertions));
    context.config_mut().identifier = identifier;
    // First: a second launch hands over to the running app and exits before it touches anything (r1.12).
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log_line(app, "desktop: the app was opened again");
            show_window(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build());
    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(tauri::generate_handler![update_state, check_for_updates, install_update, retry_update, test_report]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(tauri::generate_handler![update_state, check_for_updates, install_update, retry_update]);
    builder
        .manage(Arc::new(Engine::default()))
        .setup(|app| {
            // Set up logging in debug mode
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // The window first, on the status page: whatever happens next is shown in it (r1.11).
            let (engine, handle) = (app.state::<Arc<Engine>>().inner().clone(), app.handle().clone());
            let window_builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Deltabadger")
                .inner_size(1280.0, 800.0)
                .min_inner_size(320.0, 600.0)
                .center()
                .devtools(true)
                .initialization_script(initialization_script())
                .on_navigation(move |url| match navigation(url, engine.launch().map(|(port, _)| port)) {
                    Navigation::Allow => true,
                    Navigation::Browser => {
                        #[allow(deprecated)] // the plugin tauri.js already opens links with
                        let _ = tauri_plugin_shell::ShellExt::shell(&handle).open(url.as_str(), None);
                        false
                    }
                    Navigation::Refuse => false,
                })
                .on_download(|webview, event| {
                    match event {
                        tauri::webview::DownloadEvent::Requested { destination, .. } => {
                            match webview.app_handle().path().download_dir() {
                                Ok(downloads) => *destination = download_destination(&downloads, destination),
                                Err(error) => log_line(webview.app_handle(), &format!("desktop: download: no Downloads folder: {error}")),
                            }
                        }
                        tauri::webview::DownloadEvent::Finished { success, .. } => {
                            log_line(webview.app_handle(), &format!("desktop: download {}", if success { "saved" } else { "failed" }))
                        }
                        _ => {}
                    }
                    true
                });

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
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_window(app),
                    "quit" => {
                        quit(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click { button, .. } = event {
                        if button == tauri::tray::MouseButton::Left {
                            show_window(tray.app_handle());
                        }
                    }
                })
                .build(app)?;

            let view = Arc::new(WindowView { app: app.handle().clone(), status: status_page(), last: Mutex::new(None) });
            let handle = app.handle().clone();
            let agent = Arc::new(Agent::new(move |state: &Snapshot| {
                let _ = handle.emit_to("main", "update-state", state);
                // The log names every state (download progress in quarters): support and the proofs read it there.
                if !matches!(state.state, update::State::Downloading { percent: Some(p), .. } if p % 25 != 0) {
                    let json = serde_json::to_value(state).unwrap_or_default();
                    log_line(&handle, &format!("desktop: update: state {}", json.as_object().map(|o| o.iter().filter(|(k, _)| *k != "seq").map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" ")).unwrap_or_default()));
                }
            }));
            let config = update::Config::read(&app.config().identifier, cfg!(debug_assertions), &|k| std::env::var(k).ok());
            let host = match launch(app) {
                Ok(launch) => {
                    app.manage(LogFile(launch.log_file()));
                    let (launch, engine) = (Arc::new(launch), app.state::<Arc<Engine>>().inner().clone());
                    let (l, e, v) = (launch.clone(), engine.clone(), view.clone());
                    let supervisor = Mutex::new(Some(std::thread::spawn(move || supervisor::supervise(&l, &e, &*v))));
                    let fault = config.fault.clone();
                    Some(Arc::new(AppHost { app: app.handle().clone(), engine, launch, view, agent: agent.clone(), fault, supervisor }))
                }
                Err(error) => {
                    view.status(&Page::NotStarted(error).fragment(Path::new("")));
                    None
                }
            };
            app.manage(Updates { agent, config: config.clone(), host });
            // At start once the window exists, then every 12 hours (§1); independent of the engine.
            if config.enabled {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) {
                        if let Some(app) = update::install::installed_app(&exe, &|k| std::env::var(k).ok()) {
                            update::install::remove_leftovers(&app, &|line| log_line(&handle, line));
                        }
                    }
                    loop {
                        run_check(&handle, false).await;
                        tokio::time::sleep(config.interval).await;
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
        .build(context)
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // Every quit, whichever way it came (PLAN-B6 r1.1): close the engine's stdin and let the app go. Cmd-Q, Dock
            // Quit, AppleScript and logout arrive as Exit, which cannot be held; the engine drains on its own.
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => quit(app),
            // A Dock click or a Finder open while the window is hidden (r1.12).
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_window(app),
            _ => {}
        });
}

#[cfg(test)]
mod tests;
