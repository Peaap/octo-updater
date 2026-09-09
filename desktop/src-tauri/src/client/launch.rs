use super::process_guard::is_process_running;
use super::realmlist::heal_realmlist;
use std::{fs, path::Path};

/// Deletes the client's WDB folder (server-data cache; safe to drop). Mirrors
/// the legacy Python updater's `remove_wdb` — best-effort, never raises.
pub fn clear_wdb(game_directory: &str) {
    let wdb = Path::new(game_directory).join("WDB");
    if wdb.is_dir() {
        let _ = fs::remove_dir_all(&wdb);
    }
}

/// Chooses which executable to launch: `VanillaFixes.exe` when present (it
/// injects DLLs and starts `WoW.exe` itself), otherwise `WoW.exe` directly.
/// Mirrors the legacy Python updater's `_launch_game` executable selection.
/// Milestone 3 (mod registry/state) will refine this to also check whether
/// VanillaFixes is *enabled*, not just present on disk.
pub fn select_launch_executable(game_directory: &str) -> Option<(String, &'static str)> {
    let vanilla_fixes = Path::new(game_directory).join("VanillaFixes.exe");
    if vanilla_fixes.is_file() {
        return Some((vanilla_fixes.display().to_string(), "VanillaFixes.exe"));
    }
    let wow = Path::new(game_directory).join("WoW.exe");
    if wow.is_file() {
        return Some((wow.display().to_string(), "WoW.exe"));
    }
    None
}

/// Launches the game if it is not already running and the launch executable
/// exists. Returns a human-readable label for the executable that was
/// launched. Guards against double-launch by checking the process list
/// before spawning, closing the gap where a double click could otherwise
/// spawn two game clients.
pub fn launch_game(game_directory: &str, clear_wdb_first: bool) -> Result<String, String> {
    let (executable_path, label) = select_launch_executable(game_directory)
        .ok_or_else(|| format!("No game executable was found in {game_directory}."))?;

    if is_process_running(Path::new(&executable_path))? {
        return Err(format!("{label} is already running."));
    }

    // A missing/interrupted heal before launch is a real connectivity trap: a
    // 0-byte realmlist.wtf left by an interrupted sync silently disconnects
    // the client with no obvious error. Heal defensively right before launch
    // as well as after a sync, and never block launch on a heal failure.
    if let Err(error) = heal_realmlist(game_directory) {
        eprintln!("Could not heal realmlist.wtf before launch: {error}");
    }

    if clear_wdb_first {
        clear_wdb(game_directory);
    }

    std::process::Command::new(&executable_path)
        .current_dir(game_directory)
        .spawn()
        .map_err(|error| format!("Could not launch {label}: {error}"))?;
    Ok(label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("octo-launch-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prefers_vanillafixes_when_present() {
        let dir = temp_dir("prefers-vf");
        fs::write(dir.join("WoW.exe"), b"stub").unwrap();
        fs::write(dir.join("VanillaFixes.exe"), b"stub").unwrap();
        let (_, label) = select_launch_executable(dir.to_str().unwrap()).unwrap();
        assert_eq!(label, "VanillaFixes.exe");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn falls_back_to_wow_exe() {
        let dir = temp_dir("falls-back");
        fs::write(dir.join("WoW.exe"), b"stub").unwrap();
        let (_, label) = select_launch_executable(dir.to_str().unwrap()).unwrap();
        assert_eq!(label, "WoW.exe");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn returns_none_when_no_executable_exists() {
        let dir = temp_dir("no-exe");
        assert!(select_launch_executable(dir.to_str().unwrap()).is_none());
        fs::remove_dir_all(&dir).ok();
    }
}
