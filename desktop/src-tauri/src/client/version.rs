use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

// Fixed offsets in a 1.12.1-family WoW.exe where the build number and version
// string live. Ported from the legacy Python updater (`get_client_version`).
const BUILD_OFFSET: u64 = 0x00437bfc;
const BUILD_LEN: usize = 4;
const VERSION_OFFSET: u64 = 0x00437c04;
const VERSION_LEN: usize = 6;

/// Reads the client version string ("1.12.1 (5875)") straight from fixed
/// offsets in `WoW.exe`, without loading the whole ~5 MB binary. Returns an
/// empty string (never an error) when the file is missing, too small, an
/// unexpected build, or mid-write with junk at those offsets — matching the
/// legacy Python updater's "show nothing rather than garbage" behavior.
pub fn read_client_version(client_dir: &Path) -> String {
    let exe_path = client_dir.join("WoW.exe");
    let Ok(text) = try_read(&exe_path) else {
        return String::new();
    };
    text
}

fn try_read(exe_path: &Path) -> std::io::Result<String> {
    let mut file = File::open(exe_path)?;
    let build = read_ascii_field(&mut file, BUILD_OFFSET, BUILD_LEN)?;
    let version = read_ascii_field(&mut file, VERSION_OFFSET, VERSION_LEN)?;

    let looks_like_version = !version.is_empty()
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        && version.contains('.');
    let looks_like_build = !build.is_empty() && build.chars().all(|c| c.is_ascii_digit());
    if !looks_like_version || !looks_like_build {
        return Ok(String::new());
    }
    Ok(format!("{version} ({build})"))
}

fn read_ascii_field(file: &mut File, offset: u64, len: usize) -> std::io::Result<String> {
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0_u8; len];
    file.read_exact(&mut buffer)?;
    let text = String::from_utf8_lossy(&buffer);
    Ok(text.trim_end_matches('\0').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(name: &str, build: &str, version: &str) -> tempfile_path::TempFile {
        let path = std::env::temp_dir().join(format!(
            "octo-version-test-{name}-{}.exe",
            std::process::id()
        ));
        let mut file = File::create(&path).unwrap();
        let mut buffer = vec![0_u8; 0x00437c04 + 8];
        let build_bytes = build.as_bytes();
        buffer[BUILD_OFFSET as usize..BUILD_OFFSET as usize + build_bytes.len()]
            .copy_from_slice(build_bytes);
        let version_bytes = version.as_bytes();
        buffer[VERSION_OFFSET as usize..VERSION_OFFSET as usize + version_bytes.len()]
            .copy_from_slice(version_bytes);
        file.write_all(&buffer).unwrap();
        tempfile_path::TempFile(path)
    }

    mod tempfile_path {
        pub struct TempFile(pub std::path::PathBuf);
        impl Drop for TempFile {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
    }

    #[test]
    fn reads_a_well_formed_version() {
        let fixture = write_fixture("well-formed", "5875", "1.12.1");
        let sub = std::env::temp_dir().join(format!(
            "octo-version-test-well-formed-dir-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::rename(&fixture.0, sub.join("WoW.exe")).unwrap();
        assert_eq!(read_client_version(&sub), "1.12.1 (5875)");
        std::fs::remove_dir_all(&sub).ok();
    }

    #[test]
    fn returns_empty_for_missing_file() {
        let dir =
            std::env::temp_dir().join(format!("octo-version-test-missing-{}", std::process::id()));
        assert_eq!(read_client_version(&dir), "");
    }

    #[test]
    fn returns_empty_for_garbage_bytes() {
        let fixture = write_fixture("garbage", "xx@1", "junk!!");
        let sub = std::env::temp_dir().join(format!(
            "octo-version-test-garbage-dir-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::rename(&fixture.0, sub.join("WoW.exe")).unwrap();
        assert_eq!(read_client_version(&sub), "");
        std::fs::remove_dir_all(&sub).ok();
    }
}
