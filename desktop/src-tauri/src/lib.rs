//! Desktop shell around the `montgomery` training engine.
//!
//! The engine is a separate process, never a linked library: montgomery is AGPL-3.0 and linking it
//! would make this app a derivative work (see docs/DECISIONS.md D002). Talking to it over
//! stdin/stdout also means a driver crash kills only the child, which the UI can report and retry.
//!
//! Division of labour in this crate — the split exists so a second front end can be added without
//! touching the logic (DECISIONS D011):
//!
//! | module | responsibility | depends on Tauri |
//! | --- | --- | --- |
//! | [`dataset`] | read `dataset.yaml`, count images and labels | no |
//! | [`engine`] | locate the exe, spawn it, fan events out through a hub | no |
//! | [`settings`] | load/save user settings from one JSON file | no |
//! | [`runstate`] | fold hub events into one readable snapshot | no |
//! | [`runs`] | discover finished runs on disk (what `/state` cannot know after a restart) | no |
//! | [`runtime`] | the state both front ends share (settings, engine, snapshot, control) | no |
//! | [`control`] | local HTTP service that lets an AI client drive the app | no |
//! | [`tray`] | tray icon, show/hide, quit | yes |
//! | this file | the `#[tauri::command]` surface, the event forwarder, process wiring | yes |
//!
//! Two front ends, one state: the window calls the commands below, the HTTP service calls the same
//! `Runtime` methods. Neither owns the engine; both observe it.
//!
//! The progress contract lives in docs/API.md. Nothing here invents a field the engine does not
//! already emit — this layer forwards JSON lines verbatim and only adds lifecycle events (`exit`)
//! that a process cannot emit about itself.

use tauri::{AppHandle, Emitter, Manager, State};

mod control;
mod dataset;
mod diagnostics;
mod engine;
mod feedback;
mod onnx_export;
mod predict;
mod pretrained;
mod runstate;
mod runs;
mod runtime;
mod settings;
mod tray;
mod updater;
mod version;

use engine::TrainRequest;
use runtime::{Runtime, SettingsView};

/// The version the settings page displays. Kept as a command rather than a build-time constant in
/// the frontend so a browser preview can never disagree with the installed binary.
#[tauri::command]
fn app_version() -> String {
    version::VERSION.to_owned()
}

/// Async so the HTTP request never blocks a command thread. `updater::check` folds all network
/// failures into the returned struct; the command itself always succeeds.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> updater::UpdateInfo {
    updater::check(&app).await
}

/// On Windows this does not return on success: the updater launches the NSIS installer and exits.
#[tauri::command]
async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    updater::install(&app).await
}

#[tauri::command]
fn resolve_engine(runtime: State<'_, Runtime>) -> Result<String, String> {
    let configured = runtime.settings().engine_path;
    engine::resolve(configured.as_deref()).map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
fn scan_dataset(path: String) -> Result<dataset::DatasetSummary, String> {
    dataset::scan(&path)
}

/// Inference from the window. Blocking on purpose: the engine takes a second or two, the window
/// has nothing else to do meanwhile, and a thread here is cheaper than a state machine there.
/// Tauri runs synchronous commands off the main thread, so the UI stays responsive.
#[tauri::command]
fn predict_image(
    runtime: State<'_, Runtime>,
    request: predict::PredictRequest,
) -> Result<predict::PredictResult, String> {
    let engine = engine::resolve(runtime.settings().engine_path.as_deref())?;
    let model = match request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(path) => std::path::PathBuf::from(path),
        None => runtime
            .run_snapshot()
            .get("best")
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "还没有可用的模型：先跑一次训练，或手动指定一个 .bpk。".to_owned())?,
    };
    if !model.is_file() {
        return Err(format!("模型文件不存在：{}", model.display()));
    }
    let default_dir = runtime.output_root().join("predictions");
    predict::run(&engine, &request, &model, &default_dir)
        .map_err(|error| error.message().to_owned())
}

/// Export the current model as ONNX. Blocking for the same reason as `predict_image`: the engine
/// takes a few seconds and Tauri already runs synchronous commands off the main thread.
#[tauri::command]
fn export_onnx(
    runtime: State<'_, Runtime>,
    request: onnx_export::ExportRequest,
) -> Result<onnx_export::ExportResult, String> {
    let engine = engine::resolve(runtime.settings().engine_path.as_deref())?;
    let model = match request
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(path) => std::path::PathBuf::from(path),
        None => runtime
            .run_snapshot()
            .get("best")
            .and_then(serde_json::Value::as_str)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| "还没有可导出的模型：先跑一次训练。".to_owned())?,
    };
    if !model.is_file() {
        return Err(format!("模型文件不存在：{}", model.display()));
    }
    let templates = onnx_export::resolve_templates()?;
    onnx_export::run(&engine, &model, &templates, &runtime.output_root().join("onnx"))
}

#[tauri::command]
fn start_training(runtime: State<'_, Runtime>, request: TrainRequest) -> Result<(), String> {
    runtime.start_training(&request)
}

#[tauri::command]
fn stop_training(runtime: State<'_, Runtime>) -> Result<(), String> {
    runtime.stop_training()
}

#[tauri::command]
fn get_settings(runtime: State<'_, Runtime>) -> Result<SettingsView, String> {
    Ok(runtime.view())
}

/// Open the folder holding the local failure reports.
///
/// Created on demand: a build that has never failed has no `diagnostics/` directory, and opening
/// a path that does not exist shows the user a Windows error instead of an empty folder.
#[tauri::command]
fn open_diagnostics_folder(
    app: tauri::AppHandle,
    runtime: State<'_, Runtime>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let root = runtime.diagnostics_root();
    std::fs::create_dir_all(&root).map_err(|error| format!("无法创建诊断目录：{error}"))?;
    app.opener()
        .open_path(root.to_string_lossy(), None::<&str>)
        .map_err(|error| format!("无法打开目录：{error}"))
}

/// Every pretrained weight this build offers, plus whether it is already on disk.
///
/// Sync and cheap: it only stats a few files. The window calls it when the train page opens and
/// after each download so the picker can swap "下载" for "已就绪".
#[tauri::command]
fn pretrained_catalog(app: AppHandle) -> Vec<pretrained::WeightView> {
    pretrained::catalog(&app)
}

/// Resolve one weight to a local path, downloading it first if needed.
///
/// Progress arrives through [`pretrained::PROGRESS_EVENT`]; the returned path goes straight into a
/// `finetune` training request. The transfer runs on a plain thread because `reqwest`'s blocking
/// client builds its own runtime and must not be driven from inside the async runtime.
#[tauri::command]
async fn download_pretrained(app: AppHandle, arch: String) -> Result<String, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    std::thread::spawn(move || {
        let _ = sender.send(pretrained::ensure(&app, &arch));
    });
    receiver.await.map_err(|_| "下载线程异常退出".to_owned())?
}

#[tauri::command]
fn save_settings(
    runtime: State<'_, Runtime>,
    settings: settings::Settings,
) -> Result<SettingsView, String> {
    runtime.save_settings(settings)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // Must be registered first: a second launch has to short-circuit before any other
        // plugin initialises, otherwise two engines race over the same dataset.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::focus_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // Reads `plugins.updater` from tauri.conf.json (feed URL + minisign public key).
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            // POSITION | SIZE | MAXIMIZED, deliberately without VISIBLE: the app closes to
            // the tray, so the last-launched state can be "hidden". Restoring that would
            // start the app with no window and look like a failed launch.
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::MAXIMIZED,
                )
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            resolve_engine,
            scan_dataset,
            start_training,
            stop_training,
            predict_image,
            export_onnx,
            get_settings,
            save_settings,
            pretrained_catalog,
            download_pretrained,
            open_diagnostics_folder,
            app_version,
            check_update,
            install_update
        ])
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let runtime = Runtime::new(config_dir, data_dir);
            app.manage(runtime.clone());

            // The window's subscriber. The snapshot is folded in `Runtime::new` because the hub
            // fans out to every subscriber; this thread only forwards to the UI. Keeping the fold
            // out of here is what lets the control service report progress with no window open.
            let receiver = runtime.engine().subscribe();
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                for message in receiver {
                    match message {
                        engine::Message::Event(value) => {
                            let _ = handle.emit("engine-event", value);
                        }
                        engine::Message::Log(line) => {
                            let _ = handle.emit("engine-log", serde_json::json!({ "line": line }));
                        }
                    }
                }
            });

            // Binds loopback and starts answering. Failure is reported in the settings page, not
            // fatal: the training UI works without it.
            runtime.start_control();

            tray::install(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // X hides, it does not quit — see src/tray.rs for why.
            if window.label() != tray::MAIN_WINDOW {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            // The child is not in a job object, so nothing else stops it: without this the
            // engine keeps burning CPU after the app is gone.
            handle.state::<Runtime>().engine().kill_now();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// `resolve` is the single entry point of the whole UI: if it returns nothing, every button in
    /// the app is dead. In the repository layout it must find the built engine.
    #[test]
    fn resolves_the_engine_from_the_repository_layout() {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let expected = repo
            .join("artifacts")
            .join("target-dist")
            .join("release")
            .join("montgomery.exe");
        if !expected.is_file() {
            return; // Engine not built in this checkout; nothing to assert against.
        }
        let found = engine::resolve(None).expect("engine should resolve in the repo layout");
        assert!(
            found.ends_with("montgomery.exe"),
            "resolved a non-engine path: {found:?}"
        );
    }
}
