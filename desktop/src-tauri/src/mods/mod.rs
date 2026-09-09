//! Mod support. A mod is either:
//! - a `DllPatch`: one or more DLL/EXE files placed in the client root and
//!   (optionally) registered in `dlls.txt` for VanillaFixes' loader — this
//!   is how the legacy Python updater's mods actually work
//!   (VanillaFixes, ClassicAPI, Nampower, TransmogFix, etc), or
//! - an `MpqPatch`: a standalone MPQ patch archive verified against a
//!   `<url>.sha256` sidecar and installed into the client's `Data` folder
//!   (backed by the vendored StormLib in the `mpq` module).
//!
//! `dlls.txt` is never exposed to the user as free-text: every entry is
//! added/removed only through `dlls_txt::add_dll`/`remove_dll`, tied to a
//! specific mod's install/uninstall, so the file always reflects exactly
//! what this updater installed.
mod dlls_txt;
mod install;
mod model;
mod registry;

pub use install::{install_mod, published_sha256_for, uninstall_mod};
pub use model::{ModDefinition, ModInstallState, ModSource, ModStatus};
pub use registry::{find_mod, mod_registry, MOD_DOWNLOAD_HOSTS};

use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, path::Path};

/// Combines the registry with persisted per-mod install state into one
/// status per known mod, for the Mods tab. Never fails: an unreachable
/// sidecar or a missing on-disk file simply leaves `update_available` at
/// `false` rather than failing the whole status list, so one offline or
/// missing mod doesn't hide the installed state of every other mod.
pub fn mod_statuses(
    game_directory: &str,
    states: &HashMap<String, ModInstallState>,
) -> Vec<ModStatus> {
    mod_registry()
        .into_iter()
        .map(|mod_def| {
            let state = states.get(&mod_def.id);
            let installed = state.is_some();
            let enabled = state.is_some_and(|state| state.enabled);
            let update_available = installed
                && published_sha256_for(&mod_def, &MOD_DOWNLOAD_HOSTS).is_some_and(|published| {
                    !matches_installed_hash(&mod_def, state, game_directory, &published)
                });
            ModStatus {
                id: mod_def.id.clone(),
                name: mod_def.name.clone(),
                description: mod_def.description.clone(),
                essential: mod_def.essential,
                installed,
                enabled,
                update_available,
            }
        })
        .collect()
}

/// For an installed `MpqPatch` mod, whether its on-disk file's hash already
/// matches `published`. Always reports "matches" (never flags an update)
/// for anything else, since `DllPatch` mods have no comparable remote
/// signal — see `published_sha256_for`.
fn matches_installed_hash(
    mod_def: &ModDefinition,
    state: Option<&ModInstallState>,
    game_directory: &str,
    published: &str,
) -> bool {
    let ModSource::MpqPatch { .. } = &mod_def.source else {
        return true;
    };
    let Some(relative_path) = state.and_then(|state| state.installed_files.first()) else {
        return true;
    };
    let Ok(bytes) = fs::read(Path::new(game_directory).join(relative_path)) else {
        return true;
    };
    format!("{:x}", Sha256::digest(bytes)) == published
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_statuses_reports_not_installed_for_an_empty_state_map() {
        let states = HashMap::new();
        let statuses = mod_statuses("C:\\nonexistent", &states);
        assert!(!statuses.is_empty());
        assert!(statuses
            .iter()
            .all(|status| !status.installed && !status.enabled));
    }

    #[test]
    fn mod_statuses_reports_installed_and_enabled_from_state() {
        let mut states = HashMap::new();
        states.insert(
            "transmogfix".to_string(),
            ModInstallState {
                installed_files: vec!["transmogfix.dll".into()],
                enabled: true,
            },
        );
        let statuses = mod_statuses("C:\\nonexistent", &states);
        let transmogfix = statuses
            .iter()
            .find(|status| status.id == "transmogfix")
            .unwrap();
        assert!(transmogfix.installed);
        assert!(transmogfix.enabled);
    }
}
