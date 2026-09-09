mod addons;
mod client;
mod log_events;
mod mods;
mod mpq;
mod news;
mod updater;

use addons::AddonStatus;
use client::TweaksConfig;
use log_events::{log, LogBuffer, LogEntry, LogLevel};
use mods::{ModStatus, MOD_DOWNLOAD_HOSTS};
use news::NewsFeed;
use std::{collections::HashMap, path::PathBuf, sync::Mutex};
use tauri::{Emitter, Manager, State};
use updater::{
    execute_update, record_synced, CancelHandle, UpdatePlan, UpdaterService, UpdaterStatus,
};

struct AppState {
    service: Mutex<UpdaterService>,
    data_dir: PathBuf,
    active_cancellations: Mutex<HashMap<String, CancelHandle>>,
    logs: LogBuffer,
}

fn with_service<T>(
    state: &State<'_, AppState>,
    action: impl FnOnce(&mut UpdaterService) -> Result<T, String>,
) -> Result<T, String> {
    let mut service = state
        .service
        .lock()
        .map_err(|_| "Updater state lock was poisoned.".to_string())?;
    action(&mut service)
}

#[tauri::command]
fn get_updater_status(state: State<'_, AppState>) -> Result<UpdaterStatus, String> {
    with_service(&state, |service| Ok(service.status()))
}

#[tauri::command]
fn get_news() -> Result<NewsFeed, String> {
    news::fetch_news()
}

#[tauri::command]
fn set_game_directory(
    directory: String,
    state: State<'_, AppState>,
) -> Result<UpdaterStatus, String> {
    with_service(&state, |service| service.set_game_directory(directory))
}

#[tauri::command]
fn prepare_update(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<UpdatePlan, String> {
    let result = with_service(&state, |service| service.prepare_update());
    match &result {
        Ok(plan) => log(
            &app,
            &state.logs,
            LogLevel::Info,
            format!(
                "Prepared update {}: {} file(s) to sync.",
                plan.id, plan.selection.selected_files
            ),
        ),
        Err(error) => log(
            &app,
            &state.logs,
            LogLevel::Error,
            format!("Could not prepare update: {error}"),
        ),
    }
    result
}

#[tauri::command]
fn recover_interrupted_updates(state: State<'_, AppState>) -> Result<UpdaterStatus, String> {
    with_service(&state, |service| service.recover_interrupted_updates())
}

/// Starts executing a previously prepared, journaled update plan on a
/// background thread. Progress and terminal outcomes are delivered to the
/// frontend as "update-progress" events, keyed by plan id, rather than
/// blocking this command for the whole download.
#[tauri::command]
fn start_update(
    id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let plan = with_service(&state, |service| service.read_plan(&id))?;
    let data_dir = state.data_dir.clone();
    let cancel = CancelHandle::new();
    {
        let mut active = state
            .active_cancellations
            .lock()
            .map_err(|_| "Cancellation registry lock was poisoned.".to_string())?;
        active.insert(id.clone(), cancel.clone());
    }

    // The updater service is protected by a mutex the UI thread also uses for
    // fast operations (status, prepare). Cloning the profile/journal state
    // needed for the download keeps the long-running aria2 process from
    // holding that lock, while journal transitions still go through the
    // shared UpdaterService so they remain the single source of truth.
    log(
        &app,
        &state.logs,
        LogLevel::Info,
        format!("Starting update {id}…"),
    );

    let service_path = data_dir.clone();
    std::thread::spawn(move || {
        let service = match UpdaterService::open(service_path) {
            Ok(service) => service,
            Err(error) => {
                let app_state: State<'_, AppState> = app.state();
                log(
                    &app,
                    &app_state.logs,
                    LogLevel::Error,
                    format!("Update {id} could not start: {error}"),
                );
                let _ = app.emit(
                    "update-progress",
                    updater::UpdateProgressEvent {
                        id: id.clone(),
                        state: updater::TransactionState::Failed,
                        message: error,
                        progress: 0.0,
                        bytes_done: 0,
                        bytes_total: 0,
                        bytes_per_second: 0,
                    },
                );
                return;
            }
        };
        let app_for_events = app.clone();
        let result = execute_update(&service, &plan, &data_dir, cancel, |event| {
            let _ = app_for_events.emit("update-progress", event);
        });
        let app_state: State<'_, AppState> = app.state();
        match result {
            Ok(()) => {
                let outcome = app_state
                    .service
                    .lock()
                    .map(|mut shared_service| record_synced(&mut shared_service, &plan));
                if let Ok(Err(error)) = outcome {
                    log(
                        &app,
                        &app_state.logs,
                        LogLevel::Warning,
                        format!("Update {id} completed but could not record sync state: {error}"),
                    );
                } else {
                    log(
                        &app,
                        &app_state.logs,
                        LogLevel::Success,
                        format!("Update {id} completed."),
                    );
                }
            }
            Err(error) => log(
                &app,
                &app_state.logs,
                LogLevel::Error,
                format!("Update {id} did not complete: {error}"),
            ),
        }
        if let Ok(mut active) = app_state.active_cancellations.lock() {
            active.remove(&id);
        };
    });
    Ok(())
}

#[tauri::command]
fn cancel_update(
    id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let active = state
        .active_cancellations
        .lock()
        .map_err(|_| "Cancellation registry lock was poisoned.".to_string())?;
    match active.get(&id) {
        Some(handle) => {
            handle.cancel();
            log(
                &app,
                &state.logs,
                LogLevel::Warning,
                format!("Cancelling update {id}…"),
            );
            Ok(())
        }
        None => Err(format!("No running update was found for {id}.")),
    }
}

#[tauri::command]
fn get_recent_logs(state: State<'_, AppState>) -> Vec<LogEntry> {
    state.logs.snapshot()
}

#[tauri::command]
fn get_tweaks(state: State<'_, AppState>) -> Result<TweaksConfig, String> {
    let service = state
        .service
        .lock()
        .map_err(|_| "Updater state lock was poisoned.".to_string())?;
    Ok(service.effective_tweaks())
}

#[tauri::command]
fn save_tweaks(tweaks: TweaksConfig, state: State<'_, AppState>) -> Result<TweaksConfig, String> {
    with_service(&state, |service| {
        service.save_tweaks(tweaks)?;
        Ok(service.effective_tweaks())
    })
}

#[tauri::command]
fn get_client_version(state: State<'_, AppState>) -> Result<String, String> {
    let status = with_service(&state, |service| Ok(service.status()))?;
    let Some(game_directory) = status.game_directory else {
        return Ok(String::new());
    };
    Ok(client::read_client_version(std::path::Path::new(
        &game_directory,
    )))
}

/// Launches the game, refusing to do so while any update transaction touches
/// the same folder (the caller must ensure no update is in-flight; the
/// process-running guard here only prevents a duplicate game launch).
#[tauri::command]
fn launch_game(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let (game_directory, clear_wdb_first) = with_service(&state, |service| {
        let status = service.status();
        let game_directory = status
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())?;
        Ok((game_directory, service.clear_wdb_on_launch()))
    })?;
    let result = client::launch_game_process(&game_directory, clear_wdb_first);
    match &result {
        Ok(label) => log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Launched {label}."),
        ),
        Err(error) => log(
            &app,
            &state.logs,
            LogLevel::Error,
            format!("Could not launch the game: {error}"),
        ),
    }
    result
}

#[tauri::command]
fn set_clear_wdb_on_launch(value: bool, state: State<'_, AppState>) -> Result<(), String> {
    with_service(&state, |service| service.set_clear_wdb_on_launch(value))
}

#[tauri::command]
fn get_mod_statuses(state: State<'_, AppState>) -> Result<Vec<ModStatus>, String> {
    let service = state
        .service
        .lock()
        .map_err(|_| "Updater state lock was poisoned.".to_string())?;
    let game_directory = service.status().game_directory.unwrap_or_default();
    Ok(mods::mod_statuses(&game_directory, service.mod_states()))
}

/// Installs a mod: refuses while an update transaction may be touching the
/// same folder is left to the caller (the UI disables this while a sync is
/// running); the game-process guard here still blocks writing mod files
/// while WoW itself is running.
#[tauri::command]
fn install_mod(
    mod_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ModStatus, String> {
    let (mod_def, game_directory) = with_service(&state, |service| {
        let mod_def = mods::find_mod(&mod_id).ok_or_else(|| format!("Unknown mod: {mod_id}"))?;
        let game_directory = service
            .status()
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())?;
        Ok((mod_def, game_directory))
    })?;
    if client::is_process_running(
        std::path::Path::new(&game_directory)
            .join("WoW.exe")
            .as_path(),
    )? {
        return Err("Please close WoW before installing or updating a mod.".into());
    }

    let app_data_dir = state.data_dir.clone();
    let install_result = mods::install_mod(
        &mod_def,
        &game_directory,
        &MOD_DOWNLOAD_HOSTS,
        &app_data_dir,
    );
    let install_state = match install_result {
        Ok(install_state) => install_state,
        Err(error) => {
            log(
                &app,
                &state.logs,
                LogLevel::Error,
                format!("Could not install {}: {error}", mod_def.name),
            );
            return Err(error);
        }
    };
    let result = with_service(&state, |service| {
        service.set_mod_state(&mod_id, install_state)?;
        mods::mod_statuses(&game_directory, service.mod_states())
            .into_iter()
            .find(|status| status.id == mod_id)
            .ok_or_else(|| "Installed mod is missing from its own registry entry.".to_string())
    });
    if result.is_ok() {
        log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Installed {}.", mod_def.name),
        );
    }
    result
}

#[tauri::command]
fn uninstall_mod(
    mod_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<ModStatus, String> {
    let (mod_def, game_directory, install_state) = with_service(&state, |service| {
        let mod_def = mods::find_mod(&mod_id).ok_or_else(|| format!("Unknown mod: {mod_id}"))?;
        let game_directory = service
            .status()
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())?;
        let install_state = service
            .mod_states()
            .get(&mod_id)
            .cloned()
            .ok_or_else(|| format!("{} is not installed.", mod_def.name))?;
        Ok((mod_def, game_directory, install_state))
    })?;
    if client::is_process_running(
        std::path::Path::new(&game_directory)
            .join("WoW.exe")
            .as_path(),
    )? {
        return Err("Please close WoW before uninstalling a mod.".into());
    }

    if let Err(error) = mods::uninstall_mod(&mod_def, &game_directory, &install_state) {
        log(
            &app,
            &state.logs,
            LogLevel::Error,
            format!("Could not remove {}: {error}", mod_def.name),
        );
        return Err(error);
    }
    let result = with_service(&state, |service| {
        service.remove_mod_state(&mod_id)?;
        mods::mod_statuses(&game_directory, service.mod_states())
            .into_iter()
            .find(|status| status.id == mod_id)
            .ok_or_else(|| "Uninstalled mod is missing from its own registry entry.".to_string())
    });
    if result.is_ok() {
        log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Removed {}.", mod_def.name),
        );
    }
    result
}

#[tauri::command]
fn get_addon_statuses(state: State<'_, AppState>) -> Result<Vec<AddonStatus>, String> {
    let service = state
        .service
        .lock()
        .map_err(|_| "Updater state lock was poisoned.".to_string())?;
    let game_directory = service.status().game_directory.unwrap_or_default();
    Ok(addons::addon_statuses(
        &game_directory,
        service.addon_states(),
        service.custom_addons(),
    ))
}

/// Installs a curated addon while WoW is closed. Network/download work occurs
/// outside the profile lock; only the resulting ownership record is persisted
/// while holding it.
#[tauri::command]
fn install_addon(
    addon_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AddonStatus, String> {
    let (addon, game_directory) = with_service(&state, |service| {
        let addon =
            addons::find_addon(&addon_id).ok_or_else(|| format!("Unknown addon: {addon_id}"))?;
        let game_directory = service
            .status()
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())?;
        Ok((addon, game_directory))
    })?;
    if client::is_process_running(
        std::path::Path::new(&game_directory)
            .join("WoW.exe")
            .as_path(),
    )? {
        return Err("Please close WoW before installing or updating an addon.".into());
    }
    let install_state = match addons::install_addon(&addon, &game_directory) {
        Ok(value) => value,
        Err(error) => {
            log(
                &app,
                &state.logs,
                LogLevel::Error,
                format!("Could not install {}: {error}", addon.name),
            );
            return Err(error);
        }
    };
    let result = with_service(&state, |service| {
        service.set_addon_state(&addon_id, install_state)?;
        addons::addon_statuses(
            &game_directory,
            service.addon_states(),
            service.custom_addons(),
        )
        .into_iter()
        .find(|status| status.id == addon_id)
        .ok_or_else(|| "Installed addon is missing from its own registry entry.".to_string())
    });
    if result.is_ok() {
        log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Installed {}.", addon.name),
        );
    }
    result
}

#[tauri::command]
fn install_custom_addon(
    request: addons::CustomAddonRequest,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AddonStatus, String> {
    let custom = addons::custom_addon_definition(request)?;
    let game_directory = with_service(&state, |service| {
        service
            .status()
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())
    })?;
    if client::is_process_running(
        std::path::Path::new(&game_directory)
            .join("WoW.exe")
            .as_path(),
    )? {
        return Err("Please close WoW before installing an addon.".into());
    }
    let install_state = match addons::install_custom_addon(&custom, &game_directory) {
        Ok(value) => value,
        Err(error) => {
            log(
                &app,
                &state.logs,
                LogLevel::Error,
                format!("Could not install custom addon {}: {error}", custom.name),
            );
            return Err(error);
        }
    };
    let result = with_service(&state, |service| {
        service.set_custom_addon(&custom.id, custom.clone())?;
        service.set_addon_state(&custom.id, install_state)?;
        addons::addon_statuses(
            &game_directory,
            service.addon_states(),
            service.custom_addons(),
        )
        .into_iter()
        .find(|status| status.id == custom.id)
        .ok_or_else(|| "Installed custom addon is missing from its own state.".to_string())
    });
    if result.is_ok() {
        log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Installed custom addon {}.", custom.name),
        );
    }
    result
}

#[tauri::command]
fn update_custom_addon(
    addon_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AddonStatus, String> {
    let (custom, game_directory, old_state) = with_service(&state, |service| {
        let custom = service
            .custom_addons()
            .get(&addon_id)
            .cloned()
            .ok_or_else(|| format!("Unknown custom addon: {addon_id}"))?;
        let game_directory = service
            .status()
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())?;
        let old_state = service
            .addon_states()
            .get(&addon_id)
            .cloned()
            .ok_or_else(|| format!("{} is not installed.", custom.name))?;
        Ok((custom, game_directory, old_state))
    })?;
    if client::is_process_running(
        std::path::Path::new(&game_directory)
            .join("WoW.exe")
            .as_path(),
    )? {
        return Err("Please close WoW before updating an addon.".into());
    }
    let install_state = match addons::update_custom_addon(&custom, &old_state, &game_directory) {
        Ok(value) => value,
        Err(error) => {
            log(
                &app,
                &state.logs,
                LogLevel::Error,
                format!("Could not update {}: {error}", custom.name),
            );
            return Err(error);
        }
    };
    let result = with_service(&state, |service| {
        service.set_addon_state(&addon_id, install_state)?;
        addons::addon_statuses(
            &game_directory,
            service.addon_states(),
            service.custom_addons(),
        )
        .into_iter()
        .find(|status| status.id == addon_id)
        .ok_or_else(|| "Updated custom addon is missing from its state.".to_string())
    });
    if result.is_ok() {
        log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Updated custom addon {}.", custom.name),
        );
    }
    result
}

#[tauri::command]
fn uninstall_addon(
    addon_id: String,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<AddonStatus, String> {
    let (addon_name, _custom, game_directory, install_state) = with_service(&state, |service| {
        let custom = service.custom_addons().get(&addon_id).cloned();
        let addon_name = custom
            .as_ref()
            .map(|addon| addon.name.clone())
            .or_else(|| addons::find_addon(&addon_id).map(|addon| addon.name))
            .ok_or_else(|| format!("Unknown addon: {addon_id}"))?;
        let game_directory = service
            .status()
            .game_directory
            .ok_or_else(|| "Choose a game folder first.".to_string())?;
        let install_state = service
            .addon_states()
            .get(&addon_id)
            .cloned()
            .ok_or_else(|| format!("{addon_name} is not installed."))?;
        Ok((addon_name, custom.is_some(), game_directory, install_state))
    })?;
    if client::is_process_running(
        std::path::Path::new(&game_directory)
            .join("WoW.exe")
            .as_path(),
    )? {
        return Err("Please close WoW before uninstalling an addon.".into());
    }
    if let Err(error) = addons::uninstall_addon(&game_directory, &install_state) {
        log(
            &app,
            &state.logs,
            LogLevel::Error,
            format!("Could not remove {addon_name}: {error}"),
        );
        return Err(error);
    }
    let result = with_service(&state, |service| {
        service.remove_addon_state(&addon_id)?;
        addons::addon_statuses(
            &game_directory,
            service.addon_states(),
            service.custom_addons(),
        )
        .into_iter()
        .find(|status| status.id == addon_id)
        .ok_or_else(|| "Uninstalled addon is missing from its registry entry.".to_string())
    });
    if result.is_ok() {
        log(
            &app,
            &state.logs,
            LogLevel::Success,
            format!("Removed {addon_name}."),
        );
    }
    result
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| format!("Could not locate app data directory: {error}"))?;
            let service = UpdaterService::open(data_dir.clone())
                .map_err(|error| format!("Could not initialize updater state: {error}"))?;
            app.manage(AppState {
                service: Mutex::new(service),
                data_dir,
                active_cancellations: Mutex::new(HashMap::new()),
                logs: LogBuffer::new(),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_updater_status,
            get_news,
            set_game_directory,
            prepare_update,
            recover_interrupted_updates,
            start_update,
            cancel_update,
            get_tweaks,
            save_tweaks,
            get_client_version,
            launch_game,
            set_clear_wdb_on_launch,
            get_mod_statuses,
            install_mod,
            uninstall_mod,
            get_addon_statuses,
            install_addon,
            install_custom_addon,
            update_custom_addon,
            uninstall_addon,
            get_recent_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Octo Updater");
}
