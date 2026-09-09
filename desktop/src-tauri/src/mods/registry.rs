use super::model::{DllFile, ModDefinition, ModSource};

/// Hosts mod content may be downloaded from. `dl.octowow.st` covers MPQ
/// content patches (matches the client-manifest/torrent host allowlist);
/// `codeberg.org`/`raw.githubusercontent.com` cover the two DLL mods below,
/// whose real pinned release URLs are used as-is from the legacy Python
/// registry (`MODS_REGISTRY`) rather than invented.
pub const MOD_DOWNLOAD_HOSTS: [&str; 3] =
    ["dl.octowow.st", "codeberg.org", "raw.githubusercontent.com"];

/// The built-in mod registry, ported from the legacy Python updater's
/// `MODS_REGISTRY`. Only mods with a `direct_file` source (a single pinned
/// URL, no zip/tar extraction) are included so far — those are the two
/// direct DLL patches (`TransmogFix`, `No1600x1200`) plus the one real MPQ
/// content patch (Octo Raid Visuals). Release-asset mods that require
/// GitHub/Codeberg API resolution and zip/tar extraction (VanillaFixes,
/// ClassicAPI, DXVK, Nampower, SuperWoW, UnitXP_SP3, VanillaHelpers,
/// VanillaMultiMonitorFix, AuctionQueryThrottle) are tracked as a follow-up
/// once that resolver exists (Milestone 3.1) rather than stubbed here with
/// invented data.
pub fn mod_registry() -> Vec<ModDefinition> {
    vec![
        ModDefinition {
            id: "transmogfix".into(),
            name: "TransmogFix".into(),
            description: "Prevents frame drops on character death caused by the server-side transmogrification durability recalculation.".into(),
            essential: true,
            source: ModSource::DllPatch {
                register_dll: Some("transmogfix.dll".into()),
                files: vec![DllFile {
                    url: "https://codeberg.org/MarcelineVQ/WeirdUtils/releases/download/v0.7.0/transmogfix.dll".into(),
                    dest_relative_path: "transmogfix.dll".into(),
                }],
            },
        },
        ModDefinition {
            id: "no1600x1200".into(),
            name: "No1600x1200".into(),
            description: "Fixes incorrect game resolution when your monitor's native resolution isn't detected or 1600x1200 is listed as the maximum.".into(),
            essential: false,
            source: ModSource::DllPatch {
                register_dll: Some("no1600x1200.dll".into()),
                files: vec![DllFile {
                    url: "https://raw.githubusercontent.com/RetroCro/TurtleWoW-Mods/refs/heads/main/Archive/DLL%20BACKUP/no1600x1200.dll".into(),
                    dest_relative_path: "no1600x1200.dll".into(),
                }],
            },
        },
        ModDefinition {
            id: "octo-raid-visuals".into(),
            name: "Octo Raid Visuals".into(),
            description: "Adds ground markers and sounds for boss abilities in raids.".into(),
            essential: false,
            source: ModSource::MpqPatch {
                file_name: "patch-O.mpq".into(),
                url: "https://dl.octowow.st/client/latest/Data/patch-O.mpq".into(),
            },
        },
    ]
}

pub fn find_mod(id: &str) -> Option<ModDefinition> {
    mod_registry().into_iter().find(|mod_def| mod_def.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_entries_have_unique_ids_and_allowlisted_https_hosts() {
        let registry = mod_registry();
        assert!(!registry.is_empty());
        let mut ids: Vec<&str> = registry.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), registry.len(), "duplicate mod ids in registry");

        for mod_def in &registry {
            let urls: Vec<&str> = match &mod_def.source {
                ModSource::DllPatch { files, .. } => files.iter().map(|f| f.url.as_str()).collect(),
                ModSource::MpqPatch { url, .. } => vec![url.as_str()],
            };
            for url in urls {
                assert!(
                    url.starts_with("https://"),
                    "{} must use HTTPS ({url})",
                    mod_def.id
                );
                let host = url
                    .split("://")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .unwrap_or("");
                assert!(
                    MOD_DOWNLOAD_HOSTS.contains(&host),
                    "{} host {host} is not allowlisted",
                    mod_def.id
                );
            }
        }
    }

    #[test]
    fn find_mod_returns_none_for_unknown_id() {
        assert!(find_mod("does-not-exist").is_none());
    }

    #[test]
    fn find_mod_returns_the_matching_definition() {
        let found = find_mod("transmogfix").unwrap();
        assert_eq!(found.name, "TransmogFix");
    }
}
