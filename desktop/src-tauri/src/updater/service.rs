use super::model::{TorrentFile, TransactionState};
use super::{
    fetch_client_manifest, FileSelection, UpdateJournal, UpdatePhase, UpdatePlan, UpdaterProfile,
    UpdaterStatus,
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub struct UpdaterService {
    data_dir: PathBuf,
    profile: UpdaterProfile,
}

impl UpdaterService {
    pub fn open(data_dir: PathBuf) -> Result<Self, String> {
        for directory in ["journals", "manifests", "plans"] {
            fs::create_dir_all(data_dir.join(directory)).map_err(io_error)?;
        }
        let profile_path = data_dir.join("profile.json");
        let profile = match fs::read_to_string(&profile_path) {
            Ok(value) => serde_json::from_str(&value)
                .map_err(|error| format!("Could not parse updater profile: {error}"))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => UpdaterProfile::default(),
            Err(error) => return Err(io_error(error)),
        };
        Ok(Self { data_dir, profile })
    }

    pub fn status(&self) -> UpdaterStatus {
        let interrupted_transactions = self.interrupted_count();
        let (phase, message) = match (&self.profile.game_directory, interrupted_transactions) {
            (None, _) => (
                UpdatePhase::NeedsConfiguration,
                "Choose your OctoWoW folder to begin.".into(),
            ),
            (Some(_), count) if count > 0 => (
                UpdatePhase::Recovering,
                format!("{count} interrupted update transaction(s) need recovery."),
            ),
            (Some(_), _) => (
                UpdatePhase::Ready,
                "Ready to retrieve and validate the client manifest.".into(),
            ),
        };
        UpdaterStatus {
            phase,
            message,
            game_directory: self.profile.game_directory.clone(),
            interrupted_transactions,
        }
    }

    pub fn set_game_directory(&mut self, directory: String) -> Result<UpdaterStatus, String> {
        let path = Path::new(&directory);
        if !path.is_dir() {
            return Err("The selected game folder does not exist or is not a directory.".into());
        }
        self.profile.game_directory =
            Some(path.canonicalize().map_err(io_error)?.display().to_string());
        self.save_profile()?;
        Ok(self.status())
    }

    pub fn prepare_update(&mut self) -> Result<UpdatePlan, String> {
        let game_directory = self
            .profile
            .game_directory
            .clone()
            .ok_or_else(|| "Choose a game folder before preparing an update.".to_string())?;
        preflight_game_directory(&game_directory)?;
        let fetched = fetch_client_manifest()?;
        let (selection, selected_files) = select_files(&game_directory, &fetched.manifest.files)?;
        let manifest_path = self
            .data_dir
            .join("manifests")
            .join(format!("{}.torrent", fetched.manifest.sha256));
        let cached_manifest_matches = fs::read(&manifest_path)
            .map(|cached| format!("{:x}", Sha256::digest(cached)) == fetched.manifest.sha256)
            .unwrap_or(false);
        if !cached_manifest_matches {
            write_bytes_atomic(&manifest_path, &fetched.raw)?;
        }

        let id = format!("update-{}", now_millis());
        let plan_path = self.plan_path(&id);
        let journal_path = self.journal_path(&id);
        let plan = UpdatePlan {
            id: id.clone(),
            game_directory: game_directory.clone(),
            manifest: fetched.manifest,
            selection,
            manifest_path: manifest_path.display().to_string(),
            plan_path: plan_path.display().to_string(),
            journal_path: journal_path.display().to_string(),
        };
        // Persist the immutable plan before the journal references it. The download and
        // apply phases must consume plan.manifest_path, never fetch the remote URL again.
        write_json_atomic(&plan_path, &plan)?;
        let now = (now_millis() / 1_000) as u64;
        let journal = UpdateJournal {
            id,
            game_directory,
            state: TransactionState::Prepared,
            manifest_sha256: plan.manifest.sha256.clone(),
            manifest_path: plan.manifest_path.clone(),
            plan_path: plan.plan_path.clone(),
            selected_files,
            last_error: None,
            created_at_unix: now,
            updated_at_unix: now,
        };
        write_json_atomic(&journal_path, &journal)?;
        Ok(plan)
    }

    pub fn read_plan(&self, id: &str) -> Result<UpdatePlan, String> {
        let path = self.plan_path(id);
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("Could not read update plan {}: {error}", path.display()))?;
        serde_json::from_str(&text)
            .map_err(|error| format!("Could not parse update plan {}: {error}", path.display()))
    }

    pub fn read_journal(&self, id: &str) -> Result<UpdateJournal, String> {
        let path = self.journal_path(id);
        let text = fs::read_to_string(&path).map_err(|error| {
            format!("Could not read update journal {}: {error}", path.display())
        })?;
        serde_json::from_str(&text)
            .map_err(|error| format!("Could not parse update journal {}: {error}", path.display()))
    }

    /// Validates and persists a journal transition. Rejects transitions the
    /// state machine does not allow (see `TransactionState::can_transition_to`),
    /// so a crash or race can never silently jump over a required phase.
    pub fn transition(
        &self,
        id: &str,
        next: TransactionState,
        last_error: Option<String>,
    ) -> Result<UpdateJournal, String> {
        let mut journal = self.read_journal(id)?;
        if !journal.state.can_transition_to(next) {
            return Err(format!(
                "Cannot move update {id} from {:?} to {:?}.",
                journal.state, next
            ));
        }
        journal.state = next;
        journal.last_error = last_error;
        journal.updated_at_unix = (now_millis() / 1_000) as u64;
        write_json_atomic(&self.journal_path(id), &journal)?;
        Ok(journal)
    }

    pub fn record_synced(
        &mut self,
        game_directory: &str,
        manifest_sha256: &str,
    ) -> Result<(), String> {
        self.profile.last_synced_game_directory = Some(game_directory.into());
        self.profile.last_synced_torrent_sha256 = Some(manifest_sha256.into());
        self.save_profile()
    }

    pub fn is_stale_resume_context(&self, game_directory: &str, manifest_sha256: &str) -> bool {
        self.profile.last_synced_game_directory.as_deref() != Some(game_directory)
            || self.profile.last_synced_torrent_sha256.as_deref() != Some(manifest_sha256)
    }

    /// The user's stored tweaks, or an aspect-ratio-appropriate default seeded
    /// from the current display when none have been saved yet.
    pub fn effective_tweaks(&self) -> crate::client::TweaksConfig {
        self.profile
            .tweaks
            .clone()
            .unwrap_or_else(|| {
                let (width, height, _) = crate::client::current_display_info();
                crate::client::TweaksConfig::default_for_aspect_ratio(width as f64 / height as f64)
            })
            .clamped()
    }

    pub fn save_tweaks(&mut self, tweaks: crate::client::TweaksConfig) -> Result<(), String> {
        self.profile.tweaks = Some(tweaks.clamped());
        self.save_profile()
    }

    pub fn clear_wdb_on_launch(&self) -> bool {
        self.profile.clear_wdb_on_launch
    }
    pub fn set_clear_wdb_on_launch(&mut self, value: bool) -> Result<(), String> {
        self.profile.clear_wdb_on_launch = value;
        self.save_profile()
    }

    pub fn mod_states(&self) -> &std::collections::HashMap<String, crate::mods::ModInstallState> {
        &self.profile.mods
    }

    pub fn set_mod_state(
        &mut self,
        mod_id: &str,
        state: crate::mods::ModInstallState,
    ) -> Result<(), String> {
        self.profile.mods.insert(mod_id.to_string(), state);
        self.save_profile()
    }

    pub fn remove_mod_state(&mut self, mod_id: &str) -> Result<(), String> {
        self.profile.mods.remove(mod_id);
        self.save_profile()
    }

    pub fn addon_states(
        &self,
    ) -> &std::collections::HashMap<String, crate::addons::AddonInstallState> {
        &self.profile.addons
    }

    pub fn set_addon_state(
        &mut self,
        addon_id: &str,
        state: crate::addons::AddonInstallState,
    ) -> Result<(), String> {
        self.profile.addons.insert(addon_id.to_string(), state);
        self.save_profile()
    }

    pub fn remove_addon_state(&mut self, addon_id: &str) -> Result<(), String> {
        self.profile.addons.remove(addon_id);
        self.save_profile()
    }

    pub fn custom_addons(
        &self,
    ) -> &std::collections::HashMap<String, crate::addons::CustomAddonDefinition> {
        &self.profile.custom_addons
    }

    pub fn set_custom_addon(
        &mut self,
        addon_id: &str,
        definition: crate::addons::CustomAddonDefinition,
    ) -> Result<(), String> {
        self.profile
            .custom_addons
            .insert(addon_id.to_string(), definition);
        self.save_profile()
    }

    pub fn recover_interrupted_updates(&mut self) -> Result<UpdaterStatus, String> {
        let journals = self.data_dir.join("journals");
        for entry in fs::read_dir(journals).map_err(io_error)? {
            let path = entry.map_err(io_error)?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let mut journal: UpdateJournal =
                serde_json::from_str(&fs::read_to_string(&path).map_err(io_error)?).map_err(
                    |error| format!("Could not parse update journal {}: {error}", path.display()),
                )?;
            // Only Prepared can be safely abandoned here: no game file is touched
            // before Downloading begins. Downloading/Verifying/Applying left
            // incomplete by a crash are reported as Failed instead of silently
            // discarded, since a partial file write may already be on disk.
            if journal.state == TransactionState::Prepared {
                journal.state = TransactionState::Abandoned;
                journal.updated_at_unix = (now_millis() / 1_000) as u64;
                write_json_atomic(&path, &journal)?;
            } else if !journal.state.is_terminal() {
                journal.state = TransactionState::Failed;
                journal.last_error = Some("Interrupted by an application restart.".into());
                journal.updated_at_unix = (now_millis() / 1_000) as u64;
                write_json_atomic(&path, &journal)?;
            }
        }
        Ok(self.status())
    }

    fn interrupted_count(&self) -> usize {
        fs::read_dir(self.data_dir.join("journals"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| fs::read_to_string(entry.path()).ok())
            .filter_map(|text| serde_json::from_str::<UpdateJournal>(&text).ok())
            .filter(|journal| !journal.state.is_terminal())
            .count()
    }

    fn journal_path(&self, id: &str) -> PathBuf {
        self.data_dir.join("journals").join(format!("{id}.json"))
    }
    fn plan_path(&self, id: &str) -> PathBuf {
        self.data_dir.join("plans").join(format!("{id}.json"))
    }
    fn save_profile(&self) -> Result<(), String> {
        write_json_atomic(&self.data_dir.join("profile.json"), &self.profile)
    }
}

fn write_json_atomic<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    write_bytes_atomic(path, &bytes)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut file = fs::File::create(&temporary).map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    fs::rename(temporary, path).map_err(io_error)
}

fn preflight_game_directory(game_directory: &str) -> Result<(), String> {
    if cfg!(windows) && game_directory.len() > 220 {
        return Err("The game-folder path is too long for a reliable Windows update. Choose a location below 220 characters.".into());
    }
    if cfg!(windows) && wow_process_running()? {
        return Err("Please close WoW before checking or preparing an update.".into());
    }
    Ok(())
}

fn wow_process_running() -> Result<bool, String> {
    crate::client::is_process_running(Path::new("WoW.exe"))
        .map_err(|error| format!("{error} Refusing to prepare an update safely."))
}

fn select_files(
    game_directory: &str,
    files: &[TorrentFile],
) -> Result<(FileSelection, Vec<usize>), String> {
    let mut selection = FileSelection {
        missing_files: 0,
        mismatched_files: 0,
        selected_files: 0,
        selected_bytes: 0,
    };
    let mut selected_indices = Vec::new();
    for (index, file) in files.iter().enumerate() {
        // The launcher owns root and locale-scoped realm configuration. The
        // torrent's different byte length must not re-select it after healing.
        if is_launcher_managed_realmlist(&file.path) {
            continue;
        }
        let path = Path::new(game_directory).join(file.path.replace('/', "\\"));
        let mut select = |missing: bool| -> Result<(), String> {
            if missing {
                selection.missing_files += 1;
            } else {
                selection.mismatched_files += 1;
            }
            selection.selected_files += 1;
            selection.selected_bytes = selection
                .selected_bytes
                .checked_add(file.length)
                .ok_or_else(|| "Selected download size overflows a 64-bit integer.".to_string())?;
            // aria2's --select-file indices are 1-based positions in the torrent's file list.
            selected_indices.push(index + 1);
            Ok(())
        };
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.len() == file.length => {}
            Ok(_) => select(false)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => select(true)?,
            Err(error) => return Err(format!("Could not inspect {}: {error}", path.display())),
        }
    }
    Ok((selection, selected_indices))
}

fn is_launcher_managed_realmlist(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    matches!(parts.as_slice(), [name] if name.eq_ignore_ascii_case("realmlist.wtf"))
        || matches!(parts.as_slice(), [data, locale, name]
            if data.eq_ignore_ascii_case("Data")
                && !locale.is_empty()
                && name.eq_ignore_ascii_case("realmlist.wtf"))
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
fn io_error(error: io::Error) -> String {
    format!("Filesystem operation failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("octo-updater-test-{name}-{}", now_millis()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn select_files_flags_missing_and_mismatched_files_only() {
        let dir = temp_dir("select-files");
        fs::write(dir.join("present.txt"), b"12345").unwrap();
        let files = vec![
            TorrentFile {
                path: "present.txt".into(),
                length: 5,
            },
            TorrentFile {
                path: "wrong-size.txt".into(),
                length: 99,
            },
            TorrentFile {
                path: "missing.txt".into(),
                length: 10,
            },
        ];
        fs::write(dir.join("wrong-size.txt"), b"short").unwrap();

        let (selection, indices) = select_files(dir.to_str().unwrap(), &files).unwrap();
        assert_eq!(selection.missing_files, 1);
        assert_eq!(selection.mismatched_files, 1);
        assert_eq!(selection.selected_files, 2);
        assert_eq!(selection.selected_bytes, 99 + 10);
        assert_eq!(indices, vec![2, 3]); // 1-based, "present.txt" (index 1) excluded

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn transitions_are_rejected_once_terminal() {
        use super::super::model::TransactionState::*;
        assert!(Prepared.can_transition_to(Downloading));
        assert!(Downloading.can_transition_to(Verifying));
        assert!(Verifying.can_transition_to(Applying));
        assert!(Applying.can_transition_to(Complete));
        assert!(!Complete.can_transition_to(Downloading));
        assert!(!Failed.can_transition_to(Downloading));
        assert!(!Cancelled.can_transition_to(Verifying));
        assert!(!Prepared.can_transition_to(Applying));
    }

    #[test]
    fn service_open_creates_directories_and_recovers_prepared_journal_as_abandoned() {
        let dir = temp_dir("service-open");
        let mut service = UpdaterService::open(dir.clone()).unwrap();
        service
            .set_game_directory(dir.to_str().unwrap().to_string())
            .unwrap();

        // Force a Prepared journal without needing network access, mirroring
        // what prepare_update would persist.
        let id = "update-test".to_string();
        let journal = UpdateJournal {
            id: id.clone(),
            game_directory: dir.to_str().unwrap().to_string(),
            state: TransactionState::Prepared,
            manifest_sha256: "deadbeef".into(),
            manifest_path: "irrelevant".into(),
            plan_path: "irrelevant".into(),
            selected_files: vec![],
            last_error: None,
            created_at_unix: 0,
            updated_at_unix: 0,
        };
        write_json_atomic(&dir.join("journals").join(format!("{id}.json")), &journal).unwrap();

        let status_before = service.status();
        assert_eq!(status_before.interrupted_transactions, 1);

        service.recover_interrupted_updates().unwrap();
        let recovered: UpdateJournal = serde_json::from_str(
            &fs::read_to_string(dir.join("journals").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(recovered.state, TransactionState::Abandoned);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn downloading_journal_is_recovered_as_failed_not_silently_discarded() {
        let dir = temp_dir("service-open-downloading");
        let service = UpdaterService::open(dir.clone()).unwrap();
        let id = "update-test-downloading".to_string();
        let journal = UpdateJournal {
            id: id.clone(),
            game_directory: dir.to_str().unwrap().to_string(),
            state: TransactionState::Downloading,
            manifest_sha256: "deadbeef".into(),
            manifest_path: "irrelevant".into(),
            plan_path: "irrelevant".into(),
            selected_files: vec![],
            last_error: None,
            created_at_unix: 0,
            updated_at_unix: 0,
        };
        write_json_atomic(&dir.join("journals").join(format!("{id}.json")), &journal).unwrap();

        let mut service = service;
        service.recover_interrupted_updates().unwrap();
        let recovered: UpdateJournal = serde_json::from_str(
            &fs::read_to_string(dir.join("journals").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(recovered.state, TransactionState::Failed);
        assert!(recovered.last_error.is_some());

        let _ = File::open(&dir); // touch dir to keep it alive until here for clarity
        fs::remove_dir_all(&dir).ok();
    }
}
