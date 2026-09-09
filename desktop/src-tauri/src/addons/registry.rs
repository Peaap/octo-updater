use super::model::AddonDefinition;

/// Only these exact HTTPS hosts may serve metadata or archive bytes. GitHub's
/// archive endpoint redirects to codeload.github.com, hence both are present.
pub const ADDON_DOWNLOAD_HOSTS: [&str; 6] = [
    "api.github.com",
    "github.com",
    "codeload.github.com",
    "gitlab.com",
    "codeberg.org",
    "gitea.com",
];

/// Curated GitHub subset of the legacy recommended addon registry. pfUI is
/// intentionally omitted: the legacy launcher also modifies its Lua profile,
/// and that separate mutation has not been reviewed/ported yet.
pub fn addon_registry() -> Vec<AddonDefinition> {
    [
        (
            "atlasloot",
            "AtlasLoot",
            "AtlasLoot",
            "Loot tables and item browser.",
            "Otari98",
            "AtlasLoot",
        ),
        (
            "aux-addon",
            "aux-addon",
            "aux-addon",
            "Auction house helper.",
            "OldManAlpha",
            "aux-addon",
        ),
        (
            "better-character-stats",
            "BetterCharacterStats",
            "BetterCharacterStats",
            "Expanded character statistics.",
            "pepopo978",
            "BetterCharacterStats",
        ),
        (
            "doite-auras",
            "DoiteAuras",
            "DoiteAuras",
            "Aura tracking helper.",
            "deceius",
            "DoiteAuras",
        ),
        (
            "flight-tracker",
            "FlightTracker",
            "FlightTracker",
            "Flight path tracking.",
            "Lexxoi",
            "FlightTracker",
        ),
        (
            "instance-journal",
            "InstanceJournal",
            "InstanceJournal",
            "Instance encounter reference.",
            "Arthur-Helias",
            "InstanceJournal",
        ),
        (
            "itemrack",
            "ItemRack",
            "ItemRack",
            "Equipment set manager.",
            "Otari98",
            "ItemRack",
        ),
        (
            "levelrange-octo",
            "LevelRange-Octo",
            "LevelRange-Octo",
            "Octo level range display.",
            "Dusk-92",
            "LevelRange-Octo",
        ),
        (
            "magnify",
            "Magnify",
            "Magnify",
            "UI magnification utility.",
            "paokkerkir",
            "Magnify",
        ),
        (
            "modern-map-markers",
            "ModernMapMarkers",
            "ModernMapMarkers",
            "Modern map markers.",
            "tilare",
            "ModernMapMarkers",
        ),
        (
            "nampower-settings",
            "NampowerSettings",
            "NampowerSettings",
            "Nampower configuration.",
            "Dusk-92",
            "NampowerSettings",
        ),
        (
            "pallypower-tw",
            "PallyPowerTW",
            "PallyPowerTW",
            "Paladin blessing management.",
            "ShikawaLePaladin",
            "PallyPowerTW",
        ),
        (
            "pfquest",
            "pfQuest",
            "pfQuest",
            "Quest helper.",
            "The-Kludge-Bureau",
            "pfQuest",
        ),
        (
            "pfquest-turtle",
            "pfQuest-turtle",
            "pfQuest-turtle",
            "Turtle-compatible pfQuest data.",
            "KameleonUK",
            "pfQuest-turtle",
        ),
        (
            "shagudps",
            "ShaguDPS",
            "ShaguDPS",
            "Damage meter.",
            "shagu",
            "ShaguDPS",
        ),
        (
            "succ-bag",
            "SUCC-bag",
            "SUCC-bag",
            "Bag management helper.",
            "Otari98",
            "SUCC-bag",
        ),
        (
            "superapi",
            "SuperAPI",
            "SuperAPI",
            "Shared API dependency.",
            "balakethelock",
            "SuperAPI",
        ),
        (
            "super-cleveroid-macros",
            "SuperCleveRoidMacros",
            "SuperCleveRoidMacros",
            "Macro helper.",
            "brues-code",
            "SuperCleveRoidMacros",
        ),
        (
            "tmog",
            "Tmog",
            "Tmog",
            "Transmogrification helper.",
            "Otari98",
            "Tmog",
        ),
        (
            "trinket-menu",
            "TrinketMenu",
            "TrinketMenu",
            "Trinket manager.",
            "jrc13245",
            "TrinketMenu",
        ),
        (
            "turtle-calendar",
            "TurtleCalendar",
            "TurtleCalendar",
            "Calendar integration.",
            "Wayoff333",
            "TurtleCalendar",
        ),
        (
            "turtle-mail",
            "TurtleMail",
            "TurtleMail",
            "Mail enhancements.",
            "sica42",
            "TurtleMail",
        ),
        (
            "unitxp-sp3",
            "UnitXP_SP3_Addon",
            "UnitXP_SP3",
            "Unit experience display.",
            "rebasedkon",
            "UnitXP_SP3_Addon",
        ),
        (
            "whats-training-turtle",
            "WhatsTraining_Turtle",
            "WhatsTraining_Turtle",
            "Trainer spell information.",
            "rebasedkon",
            "WhatsTraining_Turtle",
        ),
    ]
    .into_iter()
    .map(
        |(id, folder, name, description, github_owner, github_repo)| AddonDefinition {
            id: id.into(),
            folder: folder.into(),
            name: name.into(),
            description: description.into(),
            github_owner: github_owner.into(),
            github_repo: github_repo.into(),
        },
    )
    .collect()
}

pub fn find_addon(id: &str) -> Option<AddonDefinition> {
    addon_registry().into_iter().find(|addon| addon.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_unique_non_pfui_ids() {
        let registry = addon_registry();
        let mut ids: Vec<_> = registry.iter().map(|addon| addon.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), registry.len());
        assert!(registry.iter().all(|addon| addon.folder != "pfUI"));
    }
}
