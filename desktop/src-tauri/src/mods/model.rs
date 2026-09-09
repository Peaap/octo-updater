use serde::{Deserialize, Serialize};

/// A file fetched directly from a pinned URL and placed at a client-root
/// relative destination (e.g. `transmogfix.dll`). This is how the legacy
/// updater's DLL-based mods work: a mod is one or more DLL/EXE files placed
/// next to `WoW.exe`, optionally registered in `dlls.txt` so the game's DLL
/// loader (VanillaFixes) loads it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DllFile {
    pub url: String,
    pub dest_relative_path: String,
}

/// How a mod is distributed and installed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ModSource {
    /// One or more files (DLL/EXE) fetched directly and placed in the client
    /// root, mirroring the legacy Python updater's DLL-based mods
    /// (VanillaFixes, ClassicAPI, Nampower, TransmogFix, etc). `register_dll`
    /// is the file name added to `dlls.txt` so VanillaFixes' loader picks it
    /// up; `None` for the loader itself or a DLL that auto-loads (e.g. a
    /// `d3d9.dll` DirectX proxy).
    DllPatch {
        register_dll: Option<String>,
        files: Vec<DllFile>,
    },
    /// A standalone MPQ patch archive, verified against a `<url>.sha256`
    /// sidecar and installed into the client's `Data` folder. Used for
    /// content-only mods that don't need to inject a DLL.
    MpqPatch { file_name: String, url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub essential: bool,
    pub source: ModSource,
}

/// Persisted per-mod install state, keyed by mod id in `UpdaterProfile`.
/// `installed_files` are paths relative to the game directory (so both
/// client-root DLL paths and `Data\...` MPQ paths are represented the same
/// way) and are the source of truth for uninstall — never delete a file that
/// isn't recorded here, even if the registry entry changes later.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModInstallState {
    pub installed_files: Vec<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub essential: bool,
    pub installed: bool,
    pub enabled: bool,
    /// Only meaningful for `MpqPatch` mods (compared against a `.sha256`
    /// sidecar). `DllPatch` mods pinned to a direct URL have no reliable
    /// remote version signal, matching the legacy updater's
    /// `mod_supports_update_check` returning `false` for `direct_file` mods.
    pub update_available: bool,
}
