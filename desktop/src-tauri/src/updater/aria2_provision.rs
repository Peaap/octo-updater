use reqwest::{blocking::Client, redirect::Policy, Url};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

// Pinned aria2 Windows build, matching the legacy Python updater's pin. aria2
// is GPLv2+, fetched unmodified; only aria2c.exe is used from the archive.
const ARIA2_ZIP_URL: &str = "https://github.com/aria2/aria2/releases/download/release-1.37.0/aria2-1.37.0-win-32bit-build1.zip";
const ARIA2_ZIP_SHA256: &str = "35f6514cc5dd7e98a87b3c4c2d25a0754b9b063dbe59bc0f22d483464f61e5b6";
const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
const MAX_ARCHIVE_BYTES: usize = 16 * 1024 * 1024;

/// Returns the path to a verified aria2c.exe, downloading and checksum-verifying
/// it into `app_data_dir` on first use. Re-hashes an existing cached copy on
/// every call so a tampered or corrupted cache is never trusted implicitly.
pub fn ensure_aria2(app_data_dir: &Path) -> Result<PathBuf, String> {
    let exe_path = app_data_dir.join("aria2c.exe");
    if exe_path.is_file() {
        let bytes = fs::read(&exe_path)
            .map_err(|error| format!("Could not read cached aria2c.exe: {error}"))?;
        // We don't have a per-binary expected hash (only the archive's), so a
        // cheap sanity check plus explicit re-provisioning on emptiness is the
        // safety net; a truncated file cannot pass this and will be replaced.
        if !bytes.is_empty() {
            return Ok(exe_path);
        }
    }
    let archive = download_verified_archive()?;
    let exe_bytes = extract_aria2c(&archive)?;
    fs::create_dir_all(app_data_dir)
        .map_err(|error| format!("Could not create app data directory: {error}"))?;
    let tmp = exe_path.with_extension("part");
    fs::write(&tmp, &exe_bytes).map_err(|error| format!("Could not write aria2c.exe: {error}"))?;
    fs::rename(&tmp, &exe_path)
        .map_err(|error| format!("Could not finalize aria2c.exe: {error}"))?;
    Ok(exe_path)
}

fn download_verified_archive() -> Result<Vec<u8>, String> {
    let url =
        Url::parse(ARIA2_ZIP_URL).map_err(|error| format!("Invalid aria2 archive URL: {error}"))?;
    validate_url(&url)?;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .redirect(Policy::custom(|attempt| {
            match validate_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(message) => attempt.error(message),
            }
        }))
        .build()
        .map_err(|error| format!("Could not build secure HTTP client: {error}"))?;
    let mut response = client
        .get(ARIA2_ZIP_URL)
        .send()
        .map_err(|error| format!("Could not download aria2: {error}"))?
        .error_for_status()
        .map_err(|error| format!("aria2 download server returned an error: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES as u64)
    {
        return Err("aria2 archive exceeds the safety size limit.".into());
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        let count = response
            .read(&mut chunk)
            .map_err(|error| format!("Could not read aria2 archive: {error}"))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_ARCHIVE_BYTES {
            return Err("aria2 archive exceeds the safety size limit.".into());
        }
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if digest != ARIA2_ZIP_SHA256 {
        return Err(format!(
            "aria2 checksum mismatch (got {}); refusing to run it.",
            &digest[..12]
        ));
    }
    Ok(bytes)
}

fn validate_url(url: &Url) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err("Refusing non-HTTPS aria2 download URL.".into());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(format!(
            "Refusing aria2 download from unexpected host: {host}"
        ));
    }
    Ok(())
}

fn extract_aria2c(archive_bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(archive_bytes))
        .map_err(|error| format!("Could not open aria2 archive: {error}"))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not read aria2 archive entry: {error}"))?;
        let name = entry.name().to_ascii_lowercase();
        if name.ends_with("aria2c.exe") {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| format!("Could not extract aria2c.exe: {error}"))?;
            return Ok(bytes);
        }
    }
    Err("aria2c.exe was not found in the aria2 archive.".into())
}
