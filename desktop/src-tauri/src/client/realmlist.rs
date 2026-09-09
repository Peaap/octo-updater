use std::{fs, io::Write, path::Path};

/// The server host written into every `realmlist.wtf`. Matches `Config.wtf`'s
/// `realmList`/`patchList` values (see `patcher::write_config_wtf`).
const REALM_HOST: &str = "octowow.st";

/// Rewrites `realmlist.wtf` (root and any `Data/<locale>/realmlist.wtf`) only
/// when missing, empty, or pointing at the wrong host — never when it
/// already holds the expected content, so a seeding/shared client isn't
/// rewritten needlessly. An interrupted torrent sync can leave a 0-byte
/// placeholder realmlist.wtf that silently disconnects a direct game launch;
/// this call is meant to run after every sync and before every launch.
/// Ported from the reference OctoLauncher's `healRealmlist`/`applyRealmlist`.
pub fn heal_realmlist(game_directory: &str) -> Result<(), String> {
    let expected = expected_contents();
    let root = Path::new(game_directory).join("realmlist.wtf");
    write_if_needed(&root, &expected)?;

    let data_dir = Path::new(game_directory).join("Data");
    let Ok(entries) = fs::read_dir(&data_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let scoped = entry.path().join("realmlist.wtf");
        if scoped.is_file() {
            // A per-locale realmlist is optional; failing to heal one locale
            // folder should not abort healing the others or the root file.
            let _ = write_if_needed(&scoped, &expected);
        }
    }
    Ok(())
}

fn expected_contents() -> String {
    format!("set realmlist \"{REALM_HOST}\"\n")
}

fn write_if_needed(path: &Path, expected: &str) -> Result<(), String> {
    if let Ok(current) = fs::read_to_string(path) {
        if current == expected {
            return Ok(()); // already correct; leave it alone
        }
    }
    let tmp = path.with_extension("wtf.tmp");
    let mut file = fs::File::create(&tmp)
        .map_err(|error| format!("Could not stage {}: {error}", path.display()))?;
    file.write_all(expected.as_bytes())
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("Could not flush {}: {error}", path.display()))?;
    drop(file);
    fs::rename(&tmp, path)
        .map_err(|error| format!("Could not finalize {}: {error}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("octo-realmlist-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_realmlist_when_missing() {
        let dir = temp_dir("missing");
        heal_realmlist(dir.to_str().unwrap()).unwrap();
        let contents = fs::read_to_string(dir.join("realmlist.wtf")).unwrap();
        assert_eq!(contents, expected_contents());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rewrites_a_zero_byte_placeholder() {
        let dir = temp_dir("zero-byte");
        fs::write(dir.join("realmlist.wtf"), b"").unwrap();
        heal_realmlist(dir.to_str().unwrap()).unwrap();
        let contents = fs::read_to_string(dir.join("realmlist.wtf")).unwrap();
        assert_eq!(contents, expected_contents());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rewrites_a_wrong_host() {
        let dir = temp_dir("wrong-host");
        fs::write(
            dir.join("realmlist.wtf"),
            b"set realmlist \"evil.example\"\n",
        )
        .unwrap();
        heal_realmlist(dir.to_str().unwrap()).unwrap();
        let contents = fs::read_to_string(dir.join("realmlist.wtf")).unwrap();
        assert_eq!(contents, expected_contents());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn leaves_a_correct_realmlist_untouched() {
        let dir = temp_dir("correct");
        let path = dir.join("realmlist.wtf");
        fs::write(&path, expected_contents()).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        heal_realmlist(dir.to_str().unwrap()).unwrap();
        let after = fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(before, after); // not rewritten
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn heals_a_locale_scoped_realmlist() {
        let dir = temp_dir("locale-scoped");
        let locale_dir = dir.join("Data").join("enUS");
        fs::create_dir_all(&locale_dir).unwrap();
        fs::write(locale_dir.join("realmlist.wtf"), b"").unwrap();
        heal_realmlist(dir.to_str().unwrap()).unwrap();
        let contents = fs::read_to_string(locale_dir.join("realmlist.wtf")).unwrap();
        assert_eq!(contents, expected_contents());
        fs::remove_dir_all(&dir).ok();
    }
}
