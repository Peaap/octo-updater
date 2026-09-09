use crate::{
    addons::{AddonInstallState, CustomAddonDefinition},
    client::TweaksConfig,
    mods::ModInstallState,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterProfile {
    pub game_directory: Option<String>,
    /// Persisted install state per mod id (see `mods::ModInstallState`). This
    /// is the single source of truth for uninstall — a mod's registry entry
    /// changing later never causes deletion of files not recorded here.
    #[serde(default)]
    pub mods: HashMap<String, ModInstallState>,
    /// Exact file ownership and pinned commits for curated addon installs.
    #[serde(default)]
    pub addons: HashMap<String, AddonInstallState>,
    /// User-supplied repository definitions, keyed by an opaque custom id.
    /// Install state stays in `addons` so deletion has one ownership authority.
    #[serde(default)]
    pub custom_addons: HashMap<String, CustomAddonDefinition>,
    /// The game directory and torrent identity that were last fully synced.
    /// Used to detect a stale aria2 resume context (folder changed, or the
    /// server re-rolled the torrent) so resume metadata is cleared instead of
    /// producing an "info hash mismatch" or skipping a deleted file forever.
    #[serde(default)]
    pub last_synced_game_directory: Option<String>,
    #[serde(default)]
    pub last_synced_torrent_sha256: Option<String>,
    #[serde(default)]
    pub tweaks: Option<TweaksConfig>,
    #[serde(default)]
    pub clear_wdb_on_launch: bool,
    #[serde(default)]
    pub minimize_on_launch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdatePhase {
    Ready,
    NeedsConfiguration,
    Preparing,
    Recovering,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdaterStatus {
    pub phase: UpdatePhase,
    pub message: String,
    pub game_directory: Option<String>,
    pub interrupted_transactions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TorrentFile {
    pub path: String,
    pub length: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientManifest {
    pub source_url: String,
    pub sha256: String,
    pub total_bytes: u64,
    pub files: Vec<TorrentFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSelection {
    pub missing_files: usize,
    pub mismatched_files: usize,
    pub selected_files: usize,
    pub selected_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePlan {
    pub id: String,
    pub game_directory: String,
    pub manifest: ClientManifest,
    pub selection: FileSelection,
    pub manifest_path: String,
    pub plan_path: String,
    pub journal_path: String,
}

/// Typed transaction states for a journaled update. Ordering here is only
/// documentation; legal transitions are enforced explicitly in `service.rs`
/// via `TransactionState::can_transition_to`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransactionState {
    Prepared,
    Downloading,
    Verifying,
    Applying,
    Complete,
    Cancelled,
    Failed,
    Abandoned,
}

impl TransactionState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Complete | Self::Cancelled | Self::Failed | Self::Abandoned
        )
    }

    /// Whitelist of legal forward transitions. Anything not listed here
    /// (including all transitions away from a terminal state) is rejected.
    pub fn can_transition_to(self, next: TransactionState) -> bool {
        use TransactionState::*;
        if self.is_terminal() {
            return false;
        }
        matches!(
            (self, next),
            (Prepared, Downloading)
                | (Prepared, Abandoned)
                | (Prepared, Cancelled)
                | (Downloading, Verifying)
                | (Downloading, Cancelled)
                | (Downloading, Failed)
                | (Verifying, Applying)
                | (Verifying, Cancelled)
                | (Verifying, Failed)
                | (Applying, Complete)
                | (Applying, Failed)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJournal {
    pub id: String,
    pub game_directory: String,
    pub state: TransactionState,
    #[serde(default)]
    pub manifest_sha256: String,
    #[serde(default)]
    pub manifest_path: String,
    #[serde(default)]
    pub plan_path: String,
    #[serde(default)]
    pub selected_files: Vec<usize>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at_unix: u64,
    #[serde(default)]
    pub updated_at_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgressEvent {
    pub id: String,
    pub state: TransactionState,
    pub message: String,
    pub progress: f64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_per_second: u64,
}
