use super::locale::PRISTINE_ASSERT_BYTE;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

const LOCALE_ASSERT_OFFSET: usize = 0x1b2115;

/// A cache key scoping a pristine WoW.exe to both the installation directory
/// and the torrent identity it was synced from. This replaces the legacy
/// Python updater's single global `base-WoW.exe`, which could otherwise be
/// reused across different game folders or stale torrent revisions and patch
/// an executable that was never actually verified against it.
pub fn pristine_cache_key(game_directory: &str, manifest_sha256: &str) -> String {
    let digest = Sha256::digest(format!("{game_directory}|{manifest_sha256}").as_bytes());
    format!("{:x}", digest)
}

fn cache_path(app_data_dir: &Path, cache_key: &str) -> PathBuf {
    app_data_dir
        .join("pristine")
        .join(format!("{cache_key}.exe"))
}

/// Caches the just-synced (unpatched) WoW.exe as the pristine base for this
/// exact (game directory, torrent) pair, but only when it is genuinely
/// unpatched (the locale-assert byte still reads as pristine). Returns
/// `Ok(false)` without writing anything when the on-disk exe is already
/// patched or missing, matching the Python updater's silent no-op behavior.
pub fn refresh_pristine_cache(
    app_data_dir: &Path,
    game_directory: &str,
    manifest_sha256: &str,
) -> Result<bool, String> {
    let exe_path = Path::new(game_directory).join("WoW.exe");
    let bytes = match fs::read(&exe_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("Could not read {}: {error}", exe_path.display())),
    };
    let is_pristine = bytes.get(LOCALE_ASSERT_OFFSET).copied() == Some(PRISTINE_ASSERT_BYTE);
    if !is_pristine {
        return Ok(false);
    }

    let key = pristine_cache_key(game_directory, manifest_sha256);
    let path = cache_path(app_data_dir, &key);
    fs::create_dir_all(path.parent().unwrap())
        .map_err(|error| format!("Could not create pristine cache directory: {error}"))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &bytes).map_err(|error| format!("Could not stage pristine cache: {error}"))?;
    fs::rename(&tmp, &path)
        .map_err(|error| format!("Could not finalize pristine cache: {error}"))?;
    Ok(true)
}

/// Returns the clean base bytes to patch from: the cached pristine exe for
/// this exact (game directory, torrent) pair if present and byte-verified as
/// pristine, otherwise the current on-disk WoW.exe. A cache entry that fails
/// the pristine-byte check is never trusted, closing the gap where the
/// original Python updater trusted any file present at its single global
/// cache path without re-validating it.
pub fn read_pristine_or_current(
    app_data_dir: &Path,
    game_directory: &str,
    manifest_sha256: &str,
) -> Result<Vec<u8>, String> {
    let key = pristine_cache_key(game_directory, manifest_sha256);
    let cached_path = cache_path(app_data_dir, &key);
    if let Ok(bytes) = fs::read(&cached_path) {
        if bytes.get(LOCALE_ASSERT_OFFSET).copied() == Some(PRISTINE_ASSERT_BYTE) {
            return Ok(bytes);
        }
    }
    let exe_path = Path::new(game_directory).join("WoW.exe");
    fs::read(&exe_path).map_err(|error| format!("Could not read {}: {error}", exe_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("octo-pristine-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn refresh_skips_a_patched_executable() {
        let app_data = temp_dir("refresh-patched");
        let game_dir = temp_dir("refresh-patched-game");
        let mut bytes = vec![0_u8; LOCALE_ASSERT_OFFSET + 8];
        bytes[LOCALE_ASSERT_OFFSET] = 0xb8; // patched marker
        fs::write(game_dir.join("WoW.exe"), &bytes).unwrap();

        let cached = refresh_pristine_cache(&app_data, game_dir.to_str().unwrap(), "abc").unwrap();
        assert!(!cached);

        fs::remove_dir_all(&app_data).ok();
        fs::remove_dir_all(&game_dir).ok();
    }

    #[test]
    fn refresh_and_read_round_trip_a_pristine_executable() {
        let app_data = temp_dir("refresh-pristine");
        let game_dir = temp_dir("refresh-pristine-game");
        let mut bytes = vec![0_u8; LOCALE_ASSERT_OFFSET + 8];
        bytes[LOCALE_ASSERT_OFFSET] = 0xa1; // pristine marker
        fs::write(game_dir.join("WoW.exe"), &bytes).unwrap();

        let cached = refresh_pristine_cache(&app_data, game_dir.to_str().unwrap(), "abc").unwrap();
        assert!(cached);

        let read_back =
            read_pristine_or_current(&app_data, game_dir.to_str().unwrap(), "abc").unwrap();
        assert_eq!(read_back, bytes);

        fs::remove_dir_all(&app_data).ok();
        fs::remove_dir_all(&game_dir).ok();
    }

    #[test]
    fn read_falls_back_to_current_exe_when_no_cache_exists() {
        let app_data = temp_dir("read-fallback");
        let game_dir = temp_dir("read-fallback-game");
        let bytes = vec![7_u8; 16];
        fs::write(game_dir.join("WoW.exe"), &bytes).unwrap();

        let read_back =
            read_pristine_or_current(&app_data, game_dir.to_str().unwrap(), "abc").unwrap();
        assert_eq!(read_back, bytes);

        fs::remove_dir_all(&app_data).ok();
        fs::remove_dir_all(&game_dir).ok();
    }
}
