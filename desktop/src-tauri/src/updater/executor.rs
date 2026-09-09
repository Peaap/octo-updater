use super::{
    aria2::{sync_client, Aria2Options, Aria2Progress},
    aria2_provision::ensure_aria2,
    model::{TorrentFile, TransactionState, UpdateProgressEvent},
    service::UpdaterService,
    UpdatePlan,
};
use crate::client::{
    heal_realmlist, patch_wow_exe, read_client_version, refresh_pristine_cache, write_config_wtf,
    TweaksConfig,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// Shared cancellation flag for one running update. Cloning is cheap; the
/// executor polls it between aria2 progress lines and the UI can request
/// cancellation from a separate Tauri command invocation.
#[derive(Clone, Default)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Runs one journaled update transaction to completion: Prepared -> Downloading
/// -> Verifying -> Applying -> Complete, or a terminal Failed/Cancelled state.
/// Every phase persists its journal transition before doing the corresponding
/// work, so a crash can be diagnosed from the journal alone. This function
/// never touches game files before a plan has been explicitly prepared and
/// approved by the caller (the Tauri command layer gates that approval).
pub fn execute_update(
    service: &UpdaterService,
    plan: &UpdatePlan,
    app_data_dir: &Path,
    cancel: CancelHandle,
    mut on_event: impl FnMut(UpdateProgressEvent),
) -> Result<(), String> {
    let emit = |on_event: &mut dyn FnMut(UpdateProgressEvent),
                state: TransactionState,
                message: &str,
                progress: f64| {
        on_event(UpdateProgressEvent {
            id: plan.id.clone(),
            state,
            message: message.into(),
            progress,
            bytes_done: 0,
            bytes_total: 0,
            bytes_per_second: 0,
        });
    };

    // Heal defensively before starting too: a prior interrupted sync can
    // leave a 0-byte realmlist.wtf in place indefinitely if the user never
    // relaunches through this path before trying again.
    if let Err(error) = heal_realmlist(&plan.game_directory) {
        eprintln!("Could not heal realmlist.wtf before update: {error}");
    }

    let journal = service
        .transition(&plan.id, TransactionState::Downloading, None)
        .map_err(|error| finish_failed(service, plan, error))?;
    emit(
        &mut on_event,
        TransactionState::Downloading,
        "Preparing aria2…",
        0.0,
    );

    // The direct-link fast path can stage beside a local client. When the
    // client filesystem rejects links, aria2 instead uses this updater-owned
    // local app-data path and applies only verified selected files afterward.
    let staging_directory =
        staging_directory_for_game(app_data_dir, &plan.game_directory, &plan.manifest.sha256);
    let executable_path = match ensure_aria2(app_data_dir) {
        Ok(path) => path,
        Err(error) => return Err(finish_failed(service, plan, error)),
    };

    // A stale resume context (different folder, or the server re-rolled the
    // torrent since the last completed sync) must not reuse aria2's saved
    // piece-completion state, or a deleted/replaced file can be skipped
    // forever, or aria2 can report an info-hash mismatch.
    if service.is_stale_resume_context(&plan.game_directory, &plan.manifest.sha256) {
        let _ = clear_resume_state(&staging_directory);
    }

    let options = Aria2Options {
        executable_path,
        staging_directory,
        game_directory: Path::new(&plan.game_directory).to_path_buf(),
        torrent_path: Path::new(&plan.manifest_path).to_path_buf(),
        selected_files: journal.selected_files.clone(),
        check_integrity: false,
    };

    let sync_result = sync_client(
        &options,
        |progress: Aria2Progress| {
            emit(
                &mut on_event,
                TransactionState::Downloading,
                "Downloading…",
                progress.progress,
            );
            on_event(UpdateProgressEvent {
                id: plan.id.clone(),
                state: TransactionState::Downloading,
                message: "Downloading…".into(),
                progress: progress.progress,
                bytes_done: progress.bytes_done,
                bytes_total: progress.bytes_total,
                bytes_per_second: progress.bytes_per_second,
            });
        },
        || cancel.is_cancelled(),
    );

    let layout = match sync_result {
        Ok(layout) => layout,
        Err(error) => {
            let state = if cancel.is_cancelled() {
                TransactionState::Cancelled
            } else {
                TransactionState::Failed
            };
            let _ = service.transition(&plan.id, state, Some(error.clone()));
            emit(&mut on_event, state, &error, 0.0);
            return Err(error);
        }
    };

    service
        .transition(&plan.id, TransactionState::Verifying, None)
        .map_err(|error| finish_failed(service, plan, error))?;
    emit(
        &mut on_event,
        TransactionState::Verifying,
        "Verifying downloaded files…",
        0.0,
    );

    if let Err(error) = verify_selected_files(
        &layout.client_root,
        &plan.manifest.files,
        &journal.selected_files,
    ) {
        return Err(finish_failed(service, plan, error));
    }

    service
        .transition(&plan.id, TransactionState::Applying, None)
        .map_err(|error| finish_failed(service, plan, error))?;
    emit(
        &mut on_event,
        TransactionState::Applying,
        "Finalizing update…",
        1.0,
    );

    if !layout.writes_directly_to_game {
        if let Err(error) = install_selected_files_atomically(
            &layout.client_root,
            Path::new(&plan.game_directory),
            &plan.manifest.files,
            &journal.selected_files,
            &plan.id,
        ) {
            return Err(finish_failed(service, plan, error));
        }
    }

    let tweaks = service.effective_tweaks();
    if let Err(error) = apply_client_state(app_data_dir, plan, &journal.selected_files, &tweaks) {
        return Err(finish_failed(service, plan, error));
    }

    service
        .transition(&plan.id, TransactionState::Complete, None)
        .map_err(|error| finish_failed(service, plan, error))?;
    emit(
        &mut on_event,
        TransactionState::Complete,
        "Update complete.",
        1.0,
    );
    Ok(())
}

/// Records the completed sync's client folder and torrent identity so a later
/// update can detect a stale aria2 resume context. Called by the command
/// layer after `execute_update` succeeds, using its own mutable lock on the
/// shared `UpdaterService` (the executor above only takes a shared reference,
/// since it may run concurrently with fast status/prepare commands).
pub fn record_synced(service: &mut UpdaterService, plan: &UpdatePlan) -> Result<(), String> {
    service.record_synced(&plan.game_directory, &plan.manifest.sha256)
}

/// Finalizes client-side state after a verified download: writes a fresh
/// `Config.wtf` when one doesn't already exist (never overwriting user
/// settings), refreshes the per-installation pristine WoW.exe cache, and
/// re-applies the user's tweaks/locale patch — but only when `WoW.exe` was
/// actually part of this sync's selection, mirroring the legacy Python
/// updater's "WoW.exe unchanged — skipping patch" behavior.
fn apply_client_state(
    app_data_dir: &Path,
    plan: &UpdatePlan,
    selected_files: &[usize],
    tweaks: &TweaksConfig,
) -> Result<(), String> {
    // Always heal after a verified sync, regardless of what was selected: an
    // interrupted torrent sync can leave a 0-byte realmlist.wtf placeholder
    // through a shared piece even when WoW.exe itself wasn't touched. Never
    // let an unrelated realmlist write failure block a verified, complete
    // client sync from being recorded as such.
    if let Err(error) = heal_realmlist(&plan.game_directory) {
        eprintln!("Could not heal realmlist.wtf after update: {error}");
    }

    let wow_was_downloaded = selected_files.iter().any(|&one_based_index| {
        one_based_index
            .checked_sub(1)
            .and_then(|index| plan.manifest.files.get(index))
            .is_some_and(|file| file.path.eq_ignore_ascii_case("WoW.exe"))
    });

    let config_path = Path::new(&plan.game_directory)
        .join("WTF")
        .join("Config.wtf");
    if !config_path.exists() {
        let (width, height, refresh_rate) = crate::client::current_display_info();
        write_config_wtf(&plan.game_directory, tweaks, (width, height), refresh_rate)?;
    }

    if !wow_was_downloaded {
        return Ok(());
    }

    refresh_pristine_cache(app_data_dir, &plan.game_directory, &plan.manifest.sha256)?;
    patch_wow_exe(
        app_data_dir,
        &plan.game_directory,
        &plan.manifest.sha256,
        tweaks,
    )?;
    let _ = read_client_version(Path::new(&plan.game_directory)); // surfaced via a future status event
    Ok(())
}

fn staging_directory_for_game(
    app_data_dir: &Path,
    game_directory: &str,
    manifest_sha256: &str,
) -> PathBuf {
    let identity = format!(
        "{:x}",
        Sha256::digest(format!("{game_directory}|{manifest_sha256}").as_bytes())
    );
    app_data_dir
        .join("aria2")
        .join(&identity[..16])
        .join("torrent-root")
}

fn finish_failed(service: &UpdaterService, plan: &UpdatePlan, error: String) -> String {
    // Best-effort: if even the failure transition cannot be persisted, the
    // original error still propagates to the caller and to the UI.
    let _ = service.transition(&plan.id, TransactionState::Failed, Some(error.clone()));
    error
}

fn verify_selected_files(
    root: &Path,
    files: &[TorrentFile],
    selected: &[usize],
) -> Result<(), String> {
    for &one_based_index in selected {
        let file = selected_file(files, one_based_index)?;
        let path = root.join(file.path.replace('/', "\\"));
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "Expected file is missing after download: {} ({error})",
                path.display()
            )
        })?;
        if !metadata.is_file() || metadata.len() != file.length {
            return Err(format!(
                "File size mismatch after download: {} (expected {} bytes, found {} bytes)",
                path.display(),
                file.length,
                metadata.len()
            ));
        }
    }
    Ok(())
}

fn install_selected_files_atomically(
    source_root: &Path,
    game_root: &Path,
    files: &[TorrentFile],
    selected: &[usize],
    update_id: &str,
) -> Result<(), String> {
    // Verify the complete staged selection before replacing any game file.
    verify_selected_files(source_root, files, selected)?;

    for (sequence, &one_based_index) in selected.iter().enumerate() {
        let file = selected_file(files, one_based_index)?;
        let relative_path = file.path.replace('/', "\\");
        let source = source_root.join(&relative_path);
        let destination = game_root.join(relative_path);
        let parent = destination.parent().ok_or_else(|| {
            format!(
                "Could not determine destination directory for {}.",
                destination.display()
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Could not create game directory {}: {error}",
                parent.display()
            )
        })?;

        let temporary = parent.join(format!(".octo-updater-{update_id}-{sequence}.part"));
        let copy_result = (|| -> Result<(), String> {
            let mut source_file = fs::File::open(&source).map_err(|error| {
                format!("Could not read staged file {}: {error}", source.display())
            })?;
            let mut temporary_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| {
                    format!("Could not stage game file {}: {error}", temporary.display())
                })?;
            let copied = std::io::copy(&mut source_file, &mut temporary_file).map_err(|error| {
                format!("Could not copy staged file {}: {error}", source.display())
            })?;
            if copied != file.length {
                return Err(format!(
                    "Copied byte count mismatch for {} (expected {}, copied {}).",
                    destination.display(),
                    file.length,
                    copied
                ));
            }
            temporary_file.sync_all().map_err(|error| {
                format!(
                    "Could not flush staged game file {}: {error}",
                    temporary.display()
                )
            })?;
            drop(temporary_file);
            fs::rename(&temporary, &destination).map_err(|error| {
                format!(
                    "Could not atomically replace game file {}: {error}",
                    destination.display()
                )
            })?;
            Ok(())
        })();
        if copy_result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        copy_result?;
    }
    Ok(())
}

fn selected_file<'a>(
    files: &'a [TorrentFile],
    one_based_index: usize,
) -> Result<&'a TorrentFile, String> {
    one_based_index
        .checked_sub(1)
        .and_then(|index| files.get(index))
        .ok_or_else(|| {
            format!("Update plan referenced an out-of-range file index {one_based_index}.")
        })
}

fn clear_resume_state(staging_directory: &Path) -> std::io::Result<()> {
    for entry in fs::read_dir(staging_directory)?.flatten() {
        let path = entry.path();
        let is_resume_metadata = path
            .extension()
            .is_some_and(|extension| extension == "aria2")
            || path
                .file_name()
                .is_some_and(|name| name == "client.torrent");
        if is_resume_metadata {
            let _ = fs::remove_file(&path);
        }
    }
    Ok(())
}
