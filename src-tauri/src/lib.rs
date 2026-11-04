//! The desktop app: a window, a tray, and the engine (`deltabadger-engine serve`) as its child (PLAN-B6). `setup`
//! shows the window on the launcher's status page (ui/index.html) and returns at once; everything that can fail runs
//! on the supervisor's thread (supervisor.rs), which moves the window to the engine's page once it answers.
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
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

mod secrets;
pub mod supervisor;

use supervisor::{stop_engine, Engine, Launch, Page, View};

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

// Also the Settings button: an install that can update itself should not be handing its owner
// instructions. Returns whether an update was found — the caller only needs that much to say
// "you are up to date", because everything after finding one is the native prompt below, and a
// successful install never returns at all.
#[tauri::command]
async fn check_for_updates(app: tauri::AppHandle) -> Result<bool, String> {
    let Some(update) = app
        .updater()
        .map_err(|error| format!("Failed to initialize updater: {error}"))?
        .check()
        .await
        .map_err(|error| format!("Failed to check for updates: {error}"))?
    else {
        log::info!("Deltabadger is up to date");
        return Ok(false);
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
        return Ok(true);
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
    let builder = builder.invoke_handler(tauri::generate_handler![check_for_updates, test_report]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(tauri::generate_handler![check_for_updates]);
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
            let window_builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Deltabadger")
                .inner_size(1280.0, 800.0)
                .min_inner_size(320.0, 600.0)
                .center()
                .devtools(true)
                .initialization_script(initialization_script())
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

            let window = window_builder.build()?;
            let status = window.url().map(|mut url| {
                url.set_fragment(None);
                url
            })?;

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
                        stop_engine(&app.state::<Arc<Engine>>());
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

            let view = WindowView { app: app.handle().clone(), status, last: Mutex::new(None) };
            match launch(app) {
                Ok(launch) => {
                    app.manage(LogFile(launch.log_file()));
                    let engine = app.state::<Arc<Engine>>().inner().clone();
                    std::thread::spawn(move || supervisor::supervise(&launch, &engine, &view));
                }
                Err(error) => view.status(&Page::NotStarted(error).fragment(Path::new(""))),
            }

            // Packaged apps check after startup so development never contacts the
            // production update endpoint or requires a real updater public key.
            if !cfg!(debug_assertions) {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = check_for_updates(app_handle).await {
                        log::error!("{error}");
                    }
                    // The result is only interesting to the Settings button; at startup the
                    // prompt has already said everything there is to say.
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
        .run(|app, event| match event {
            // Every quit, whichever way it came (PLAN-B6 r1.1): close the engine's stdin and let the app go. Cmd-Q, Dock
            // Quit, AppleScript and logout arrive as Exit, which cannot be held; the engine drains on its own.
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
                stop_engine(&app.state::<Arc<Engine>>());
            }
            // A Dock click or a Finder open while the window is hidden (r1.12).
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_window(app),
            _ => {}
        });
}

#[cfg(test)]
mod tests;
