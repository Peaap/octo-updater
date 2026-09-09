//! Addon support. Curated and user-supplied repositories share bounded,
//! commit-pinned archive installation and exact-owned-file removal.
mod install;
mod model;
mod registry;

pub use install::{
    custom_addon_definition, install_addon, install_custom_addon, uninstall_addon,
    update_custom_addon,
};
pub use model::{
    AddonDefinition, AddonInstallState, AddonStatus, CustomAddonDefinition, CustomAddonRequest,
    RepositoryProvider,
};
pub use registry::{addon_registry, find_addon, ADDON_DOWNLOAD_HOSTS};

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

/// Combines known launcher state with a read-only scan of `Interface/AddOns`.
/// A discovered folder is deliberately not considered managed: only persisted
/// exact-file ownership authorizes this launcher to update or remove files.
pub fn addon_statuses(
    game_directory: &str,
    states: &HashMap<String, AddonInstallState>,
    custom_addons: &HashMap<String, CustomAddonDefinition>,
) -> Vec<AddonStatus> {
    let folders = detected_addon_folders(game_directory);
    let mut represented = HashSet::new();
    let mut statuses: Vec<_> = addon_registry()
        .into_iter()
        .map(|addon| {
            let detected = folders.contains(&addon.folder.to_ascii_lowercase());
            represented.insert(addon.folder.to_ascii_lowercase());
            let managed = states.contains_key(&addon.id);
            AddonStatus {
                installed: detected,
                detected,
                managed,
                id: addon.id,
                name: addon.name,
                description: addon.description,
                update_available: false,
                custom: false,
                repository_url: None,
                reference: None,
                provider: None,
            }
        })
        .collect();
    statuses.extend(custom_addons.values().map(|addon| {
        let detected = folders.contains(&addon.install_folder.to_ascii_lowercase());
        represented.insert(addon.install_folder.to_ascii_lowercase());
        AddonStatus {
            id: addon.id.clone(),
            name: addon.name.clone(),
            description: "User-supplied repository.".into(),
            installed: detected,
            detected,
            managed: states.contains_key(&addon.id),
            update_available: false,
            custom: true,
            repository_url: Some(addon.repository_url.clone()),
            reference: addon.reference.clone(),
            provider: Some(addon.provider),
        }
    }));
    statuses.extend(
        folders
            .into_iter()
            .filter(|folder| !represented.contains(&folder.to_ascii_lowercase()))
            .map(|folder| AddonStatus {
                id: format!("detected:{}", folder.to_ascii_lowercase()),
                name: folder.clone(),
                description: "Detected in Interface/AddOns. This launcher does not own its files."
                    .into(),
                installed: true,
                detected: true,
                managed: false,
                update_available: false,
                custom: false,
                repository_url: None,
                reference: None,
                provider: None,
            }),
    );
    statuses.sort_by(|left, right| {
        left.custom
            .cmp(&right.custom)
            .then_with(|| left.name.cmp(&right.name))
    });
    statuses
}

fn detected_addon_folders(game_directory: &str) -> HashSet<String> {
    let root = Path::new(game_directory).join("Interface").join("AddOns");
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry.file_type().ok().filter(|kind| kind.is_dir())?;
            let name = entry.file_name().to_str()?.to_string();
            (!name.starts_with('.')).then_some(name.to_ascii_lowercase())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn statuses_include_every_curated_addon() {
        assert_eq!(
            addon_statuses("C:\\missing", &HashMap::new(), &HashMap::new()).len(),
            addon_registry().len()
        );
    }
    #[test]
    fn scan_detects_unmanaged_addon_folder() {
        let root = std::env::temp_dir().join(format!("octo-addon-scan-{}", std::process::id()));
        fs::create_dir_all(root.join("Interface/AddOns/ManualAddon")).unwrap();
        let statuses = addon_statuses(root.to_str().unwrap(), &HashMap::new(), &HashMap::new());
        let addon = statuses
            .iter()
            .find(|status| status.name == "manualaddon")
            .unwrap();
        assert!(addon.detected && addon.installed && !addon.managed);
        let _ = fs::remove_dir_all(root);
    }
}
