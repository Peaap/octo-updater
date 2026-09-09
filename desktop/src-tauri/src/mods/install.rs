use super::dlls_txt;
use super::model::{DllFile, ModDefinition, ModInstallState, ModSource};
use reqwest::{blocking::Client, redirect::Policy, Url};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path, time::Duration};

const MAX_MOD_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const REQUEST_TIMEOUT_SECS: u64 = 60;

/// Installs a mod: downloads its file(s) into app-data staging first, then
/// moves everything into place and (for `DllPatch`) registers its DLL in
/// `dlls.txt`, only after every file downloaded successfully. If any
/// download fails, nothing is written to the game directory and any file
/// already staged for a multi-file mod is cleaned up — a failed install
/// never leaves a partial mod in place.
///
/// Returns the new `ModInstallState` (files relative to `game_directory`,
/// enabled) to be persisted by the caller.
pub fn install_mod(
    mod_def: &ModDefinition,
    game_directory: &str,
    allowed_hosts: &[&str],
    app_data_dir: &Path,
) -> Result<ModInstallState, String> {
    match &mod_def.source {
        ModSource::DllPatch {
            register_dll,
            files,
        } => install_dll_patch(
            files,
            register_dll.as_deref(),
            game_directory,
            allowed_hosts,
            app_data_dir,
        ),
        ModSource::MpqPatch { file_name, url } => {
            install_mpq_patch(file_name, url, game_directory, allowed_hosts)
        }
    }
}

/// Removes a mod's tracked files and, if it registered one, its `dlls.txt`
/// entry. Only ever deletes paths recorded in `state.installed_files` —
/// never anything the registry currently claims, so a stale/changed registry
/// entry can't cause deletion of a file this updater didn't actually
/// install for this mod.
pub fn uninstall_mod(
    mod_def: &ModDefinition,
    game_directory: &str,
    state: &ModInstallState,
) -> Result<(), String> {
    for relative_path in &state.installed_files {
        let full_path = Path::new(game_directory).join(relative_path);
        if full_path.is_file() {
            fs::remove_file(&full_path)
                .map_err(|error| format!("Could not remove {relative_path}: {error}"))?;
        }
    }
    if let ModSource::DllPatch {
        register_dll: Some(dll_name),
        ..
    } = &mod_def.source
    {
        dlls_txt::remove_dll(game_directory, dll_name)?;
    }
    Ok(())
}

fn install_dll_patch(
    files: &[DllFile],
    register_dll: Option<&str>,
    game_directory: &str,
    allowed_hosts: &[&str],
    app_data_dir: &Path,
) -> Result<ModInstallState, String> {
    let staging_dir = app_data_dir
        .join("mod-staging")
        .join(format!("dll-{}", std::process::id()));
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("Could not create mod staging directory: {error}"))?;

    // Download every file into staging first; a failure here touches nothing
    // in the game directory at all.
    let mut staged: Vec<(std::path::PathBuf, String)> = Vec::new();
    let download_result = (|| {
        for file in files {
            let bytes = download_bounded(&file.url, allowed_hosts, MAX_MOD_ARCHIVE_BYTES)?;
            let staged_path = staging_dir.join(sanitize_file_component(&file.dest_relative_path));
            fs::write(&staged_path, &bytes)
                .map_err(|error| format!("Could not stage {}: {error}", file.dest_relative_path))?;
            staged.push((staged_path, file.dest_relative_path.clone()));
        }
        Ok(())
    })();
    if let Err(error) = download_result {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(error);
    }

    // Every download succeeded: move staged files into the game directory.
    let mut installed_files = Vec::new();
    let move_result = (|| {
        for (staged_path, dest_relative_path) in &staged {
            let destination = Path::new(game_directory).join(dest_relative_path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
            }
            fs::rename(staged_path, &destination)
                .or_else(|_| fs::copy(staged_path, &destination).map(|_| ())) // cross-device fallback
                .map_err(|error| format!("Could not install {dest_relative_path}: {error}"))?;
            installed_files.push(dest_relative_path.clone());
        }
        if let Some(dll_name) = register_dll {
            dlls_txt::add_dll(game_directory, dll_name)?;
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging_dir);

    if let Err(error) = move_result {
        // Roll back any file that did get installed before the failure, so a
        // partial multi-file mod is never left in a half-installed state.
        for relative_path in &installed_files {
            let _ = fs::remove_file(Path::new(game_directory).join(relative_path));
        }
        return Err(error);
    }
    Ok(ModInstallState {
        installed_files,
        enabled: true,
    })
}

fn install_mpq_patch(
    file_name: &str,
    url: &str,
    game_directory: &str,
    allowed_hosts: &[&str],
) -> Result<ModInstallState, String> {
    let expected_sha256 = fetch_sidecar_sha256(url, allowed_hosts)?;
    let bytes = download_bounded(url, allowed_hosts, MAX_MOD_ARCHIVE_BYTES)?;
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if actual_sha256 != expected_sha256 {
        return Err(format!("Checksum mismatch for {file_name}: expected {expected_sha256}, got {actual_sha256}. Refusing to install."));
    }

    let data_dir = Path::new(game_directory).join("Data");
    fs::create_dir_all(&data_dir)
        .map_err(|error| format!("Could not create the Data directory: {error}"))?;
    let destination = data_dir.join(file_name);
    let staging = destination.with_extension("mpq.part");
    fs::write(&staging, &bytes).map_err(|error| format!("Could not stage {file_name}: {error}"))?;
    fs::rename(&staging, &destination)
        .map_err(|error| format!("Could not finalize {file_name}: {error}"))?;
    Ok(ModInstallState {
        installed_files: vec![format!("Data\\{file_name}")],
        enabled: true,
    })
}

/// The published SHA-256 for an `MpqPatch` mod, from its `<url>.sha256`
/// sidecar. Returns `None` (rather than an error) when unreachable, so a
/// status check can treat "we don't know" (offline) differently from a real
/// update; `DllPatch` mods have no reliable remote version signal (a pinned
/// direct-file URL doesn't change), matching the legacy updater's
/// `mod_supports_update_check` returning `false` for `direct_file` sources.
pub fn published_sha256_for(mod_def: &ModDefinition, allowed_hosts: &[&str]) -> Option<String> {
    match &mod_def.source {
        ModSource::MpqPatch { url, .. } => fetch_sidecar_sha256(url, allowed_hosts).ok(),
        ModSource::DllPatch { .. } => None,
    }
}

fn sanitize_file_component(relative_path: &str) -> String {
    relative_path.replace(['/', '\\'], "_")
}

fn fetch_sidecar_sha256(url: &str, allowed_hosts: &[&str]) -> Result<String, String> {
    let sidecar_url = format!("{url}.sha256");
    let bytes = download_bounded(&sidecar_url, allowed_hosts, 4096)?;
    let text = String::from_utf8_lossy(&bytes);
    let hash = text
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("The sha256 sidecar at {sidecar_url} was empty."))?;
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "The sha256 sidecar at {sidecar_url} did not contain a valid hash."
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

fn download_bounded(
    url: &str,
    allowed_hosts: &[&str],
    max_bytes: usize,
) -> Result<Vec<u8>, String> {
    let parsed = Url::parse(url).map_err(|error| format!("Invalid mod download URL: {error}"))?;
    validate_url(&parsed, allowed_hosts)?;
    let owned_hosts: Vec<String> = allowed_hosts.iter().map(|host| host.to_string()).collect();
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .redirect(Policy::custom(move |attempt| {
            let hosts: Vec<&str> = owned_hosts.iter().map(String::as_str).collect();
            match validate_url(attempt.url(), &hosts) {
                Ok(()) => attempt.follow(),
                Err(message) => attempt.error(message),
            }
        }))
        .build()
        .map_err(|error| format!("Could not build secure HTTP client: {error}"))?;

    let mut response = client
        .get(url)
        .send()
        .map_err(|error| format!("Could not download {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Server returned an error for {url}: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(format!("{url} exceeds the {max_bytes}-byte safety limit."));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        let count = response
            .read(&mut chunk)
            .map_err(|error| format!("Could not read response from {url}: {error}"))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > max_bytes {
            return Err(format!("{url} exceeds the {max_bytes}-byte safety limit."));
        }
    }
    Ok(bytes)
}

fn validate_url(url: &Url, allowed_hosts: &[&str]) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err(format!("Refusing non-HTTPS URL: {url}"));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !allowed_hosts.iter().any(|allowed| host == *allowed) {
        return Err(format!("Refusing download from unexpected host: {host}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_url_rejects_non_https() {
        let url = Url::parse("http://dl.octowow.st/mods/example.mpq").unwrap();
        let error = validate_url(&url, &["dl.octowow.st"]).expect_err("http must be rejected");
        assert!(error.contains("non-HTTPS"));
    }

    #[test]
    fn validate_url_rejects_unlisted_hosts() {
        let url = Url::parse("https://evil.example/mods/example.mpq").unwrap();
        let error =
            validate_url(&url, &["dl.octowow.st"]).expect_err("unlisted host must be rejected");
        assert!(error.contains("unexpected host"));
    }

    #[test]
    fn validate_url_accepts_an_allowed_https_host() {
        let url = Url::parse("https://dl.octowow.st/mods/example.mpq").unwrap();
        assert!(validate_url(&url, &["dl.octowow.st"]).is_ok());
    }

    #[test]
    fn sanitize_file_component_strips_separators() {
        assert_eq!(sanitize_file_component("a/b\\c.dll"), "a_b_c.dll");
    }

    #[test]
    fn published_sha256_for_returns_none_for_dll_patches() {
        let mod_def = ModDefinition {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            essential: false,
            source: ModSource::DllPatch {
                register_dll: None,
                files: vec![],
            },
        };
        assert_eq!(published_sha256_for(&mod_def, &["dl.octowow.st"]), None);
    }

    #[test]
    fn uninstall_mod_only_removes_tracked_files() {
        let dir =
            std::env::temp_dir().join(format!("octo-mods-uninstall-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tracked.dll"), b"tracked").unwrap();
        fs::write(dir.join("untracked.dll"), b"untracked").unwrap();
        let mod_def = ModDefinition {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            essential: false,
            source: ModSource::DllPatch {
                register_dll: None,
                files: vec![],
            },
        };
        let state = ModInstallState {
            installed_files: vec!["tracked.dll".into()],
            enabled: true,
        };
        uninstall_mod(&mod_def, dir.to_str().unwrap(), &state).unwrap();
        assert!(!dir.join("tracked.dll").exists());
        assert!(dir.join("untracked.dll").exists());
        fs::remove_dir_all(&dir).ok();
    }
}
