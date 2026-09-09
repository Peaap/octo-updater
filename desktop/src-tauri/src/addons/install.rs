use super::{AddonDefinition, AddonInstallState, ADDON_DOWNLOAD_HOSTS};
use reqwest::{blocking::Client, redirect::Policy, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Cursor, Read},
    path::{Component, Path, PathBuf},
    time::Duration,
};
use zip::ZipArchive;

const MAX_ARCHIVE_BYTES: usize = 128 * 1024 * 1024;
const MAX_EXTRACTED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 20_000;

#[derive(Deserialize)]
struct GitHubCommit {
    sha: String,
}

pub fn latest_commit_sha(addon: &AddonDefinition) -> Result<String, String> {
    commit_sha_for(addon, None)
}

pub fn install_addon(
    addon: &AddonDefinition,
    game_directory: &str,
) -> Result<AddonInstallState, String> {
    let commit_sha = latest_commit_sha(addon)?;
    let archive_url = format!(
        "https://github.com/{}/{}/archive/{commit_sha}.zip",
        addon.github_owner, addon.github_repo
    );
    install_addon_at_commit(addon, game_directory, commit_sha, archive_url)
}

fn install_addon_at_commit(
    addon: &AddonDefinition,
    game_directory: &str,
    commit_sha: String,
    archive_url: String,
) -> Result<AddonInstallState, String> {
    let archive = download_bounded(&archive_url)?;
    let addons_root = Path::new(game_directory).join("Interface").join("AddOns");
    fs::create_dir_all(&addons_root)
        .map_err(|error| format!("Could not create addon directory: {error}"))?;
    let destination = addons_root.join(&addon.folder);
    let staging = addons_root.join(format!(".{}.install-{}", addon.folder, std::process::id()));
    remove_directory_if_exists(&staging)?;
    fs::create_dir(&staging)
        .map_err(|error| format!("Could not create addon staging directory: {error}"))?;

    let installed_files = match extract_archive(&archive, &staging, &addon.folder) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            let _ = remove_directory_if_exists(&staging);
            return Err("The addon archive did not contain any installable files.".into());
        }
        Err(error) => {
            let _ = remove_directory_if_exists(&staging);
            return Err(error);
        }
    };

    let backup = addons_root.join(format!(".{}.backup-{}", addon.folder, std::process::id()));
    remove_directory_if_exists(&backup)?;
    let had_destination = destination.exists();
    if had_destination {
        fs::rename(&destination, &backup)
            .map_err(|error| format!("Could not stage existing {}: {error}", addon.folder))?;
    }
    if let Err(error) = fs::rename(&staging, &destination) {
        if had_destination {
            let _ = fs::rename(&backup, &destination);
        }
        return Err(format!("Could not finalize {}: {error}", addon.name));
    }
    let _ = remove_directory_if_exists(&backup);
    Ok(AddonInstallState {
        commit_sha,
        installed_files,
    })
}

pub fn install_custom_addon(
    custom: &super::CustomAddonDefinition,
    game_directory: &str,
) -> Result<AddonInstallState, String> {
    let destination = Path::new(game_directory)
        .join("Interface")
        .join("AddOns")
        .join(&custom.install_folder);
    if destination.exists() {
        return Err(format!(
            "Refusing to replace existing addon folder {}. Choose an unused install folder or remove the existing addon first.",
            custom.install_folder
        ));
    }
    let addon = AddonDefinition {
        id: custom.id.clone(),
        folder: custom.install_folder.clone(),
        name: custom.name.clone(),
        description: "User-supplied GitHub repository.".into(),
        github_owner: custom.github_owner.clone(),
        github_repo: custom.github_repo.clone(),
    };
    let commit_sha = commit_sha_for_custom(custom)?;
    let archive_url = archive_url_for_custom(custom, &commit_sha)?;
    install_addon_at_commit(&addon, game_directory, commit_sha, archive_url)
}

pub fn update_custom_addon(
    custom: &super::CustomAddonDefinition,
    old_state: &AddonInstallState,
    game_directory: &str,
) -> Result<AddonInstallState, String> {
    let commit_sha = commit_sha_for_custom(custom)?;
    let archive = download_bounded(&archive_url_for_custom(custom, &commit_sha)?)?;
    let addons_root = Path::new(game_directory).join("Interface").join("AddOns");
    let destination = addons_root.join(&custom.install_folder);
    if !destination.is_dir() {
        return Err(
            "The custom addon's managed folder is missing; remove and reinstall it instead.".into(),
        );
    }
    let staging = addons_root.join(format!(
        ".{}.update-{}",
        custom.install_folder,
        std::process::id()
    ));
    remove_directory_if_exists(&staging)?;
    fs::create_dir(&staging)
        .map_err(|error| format!("Could not create update staging directory: {error}"))?;
    let installed_files = match extract_archive(&archive, &staging, &custom.install_folder) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            let _ = remove_directory_if_exists(&staging);
            return Err("The addon archive did not contain any installable files.".into());
        }
        Err(error) => {
            let _ = remove_directory_if_exists(&staging);
            return Err(error);
        }
    };
    if let Err(error) =
        preserve_unowned_files(&destination, &staging, old_state, &custom.install_folder)
    {
        let _ = remove_directory_if_exists(&staging);
        return Err(error);
    }
    let backup = addons_root.join(format!(
        ".{}.update-backup-{}",
        custom.install_folder,
        std::process::id()
    ));
    remove_directory_if_exists(&backup)?;
    fs::rename(&destination, &backup).map_err(|error| {
        format!(
            "Could not back up {} before updating: {error}",
            custom.install_folder
        )
    })?;
    if let Err(error) = fs::rename(&staging, &destination) {
        let _ = fs::rename(&backup, &destination);
        return Err(format!("Could not activate addon update: {error}"));
    }
    let _ = remove_directory_if_exists(&backup);
    Ok(AddonInstallState {
        commit_sha,
        installed_files,
    })
}

fn preserve_unowned_files(
    destination: &Path,
    staging: &Path,
    old_state: &AddonInstallState,
    folder: &str,
) -> Result<(), String> {
    let prefix = format!("interface\\addons\\{}\\", folder.to_ascii_lowercase());
    let owned: std::collections::HashSet<String> = old_state
        .installed_files
        .iter()
        .map(|path| path.replace('/', "\\").to_ascii_lowercase())
        .collect();
    copy_unowned_files(destination, destination, staging, &owned, &prefix)
}
fn copy_unowned_files(
    root: &Path,
    directory: &Path,
    staging: &Path,
    owned: &std::collections::HashSet<String>,
    prefix: &str,
) -> Result<(), String> {
    for entry in fs::read_dir(directory)
        .map_err(|error| format!("Could not inspect existing addon files: {error}"))?
    {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.is_dir() {
            copy_unowned_files(root, &path, staging, owned, prefix)?;
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "Could not normalize addon path.".to_string())?;
        let owned_path = format!(
            "{prefix}{}",
            relative
                .to_string_lossy()
                .replace('/', "\\")
                .to_ascii_lowercase()
        );
        if owned.contains(&owned_path) {
            continue;
        }
        let target = staging.join(relative);
        if target.exists() {
            return Err(format!(
                "Update conflict: {} was not installed by the launcher and would be overwritten.",
                relative.display()
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not preserve addon file: {error}"))?;
        }
        fs::copy(&path, &target)
            .map_err(|error| format!("Could not preserve addon file: {error}"))?;
    }
    Ok(())
}

pub fn custom_addon_definition(
    request: super::CustomAddonRequest,
) -> Result<super::CustomAddonDefinition, String> {
    let parsed = Url::parse(request.repository_url.trim())
        .map_err(|error| format!("Invalid GitHub repository URL: {error}"))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let provider = match host.as_str() {
        "github.com" => super::RepositoryProvider::GitHub,
        "gitlab.com" => super::RepositoryProvider::GitLab,
        "codeberg.org" | "gitea.com" => super::RepositoryProvider::Gitea,
        _ => return Err("Supported custom repository hosts are github.com, gitlab.com, codeberg.org, and gitea.com.".into()),
    };
    if parsed.scheme() != "https"
        || parsed.port().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("Use a plain HTTPS GitHub repository URL, for example https://github.com/owner/repository.".into());
    }
    let parts: Vec<_> = parsed
        .path_segments()
        .ok_or_else(|| "Invalid GitHub repository URL.".to_string())?
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() < 2 || (provider != super::RepositoryProvider::GitLab && parts.len() != 2) {
        return Err(
            "Repository URL must identify exactly https://github.com/owner/repository.".into(),
        );
    }
    let repo = parts.last().unwrap().trim_end_matches(".git").to_string();
    let owner = parts.first().unwrap().to_string();
    let repository_path: Vec<String> = parts
        .into_iter()
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                owner.clone()
            } else if index == 1 && provider != super::RepositoryProvider::GitLab {
                repo.clone()
            } else {
                part.trim_end_matches(".git").to_string()
            }
        })
        .collect();
    if !valid_repo_component(&owner) || !valid_repo_component(&repo) {
        return Err("Repository owner and name contain unsupported characters.".into());
    }
    let reference = request
        .reference
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if reference
        .as_ref()
        .is_some_and(|value| value.len() > 200 || value.chars().any(char::is_control))
    {
        return Err("The optional branch, tag, or commit reference is invalid.".into());
    }
    let install_folder = request
        .install_folder
        .unwrap_or_else(|| repo.clone())
        .trim()
        .to_string();
    if !valid_install_folder(&install_folder) {
        return Err("Install folder must be a single safe addon-folder name (letters, numbers, space, underscore, or hyphen).".into());
    }
    let repository_url = format!("https://{host}/{}", repository_path.join("/"));
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("{repository_url}\n{install_folder}").as_bytes())
    );
    Ok(super::CustomAddonDefinition {
        id: format!("custom-{}", &digest[..16]),
        repository_url,
        reference,
        install_folder,
        name: repo.clone(),
        provider,
        repository_path,
        github_owner: owner,
        github_repo: repo,
    })
}

fn commit_sha_for_custom(custom: &super::CustomAddonDefinition) -> Result<String, String> {
    let path = if custom.repository_path.is_empty() {
        vec![custom.github_owner.clone(), custom.github_repo.clone()]
    } else {
        custom.repository_path.clone()
    };
    let reference = custom.reference.as_deref();
    let url = match custom.provider {
        super::RepositoryProvider::GitHub => {
            let addon = AddonDefinition {
                id: custom.id.clone(),
                folder: custom.install_folder.clone(),
                name: custom.name.clone(),
                description: String::new(),
                github_owner: path[0].clone(),
                github_repo: path.last().cloned().unwrap_or_default(),
            };
            return commit_sha_for(&addon, reference);
        }
        super::RepositoryProvider::GitLab => {
            let project = path.join("%2F");
            match reference {
                Some(reference) => format!(
                    "https://gitlab.com/api/v4/projects/{project}/repository/commits/{reference}"
                ),
                None => format!(
                    "https://gitlab.com/api/v4/projects/{project}/repository/commits?per_page=1"
                ),
            }
        }
        super::RepositoryProvider::Gitea => {
            let host = Url::parse(&custom.repository_url)
                .map_err(|error| error.to_string())?
                .host_str()
                .unwrap_or_default()
                .to_string();
            let base = format!(
                "https://{host}/api/v1/repos/{}/{}",
                path[0],
                path.last().unwrap()
            );
            match reference {
                Some(reference) => format!("{base}/commits?sha={reference}&limit=1"),
                None => format!("{base}/commits?limit=1"),
            }
        }
    };
    #[derive(Deserialize)]
    struct Commit {
        #[serde(alias = "id")]
        sha: String,
    }
    if custom.provider == super::RepositoryProvider::GitLab && reference.is_some() {
        let commit: Commit = get_json(&url)?;
        return is_sha(&commit.sha)
            .then_some(commit.sha)
            .ok_or_else(|| "Provider returned an invalid commit SHA.".into());
    }
    let commits: Vec<Commit> = get_json(&url)?;
    commits
        .into_iter()
        .next()
        .map(|commit| commit.sha)
        .filter(|sha| is_sha(sha))
        .ok_or_else(|| "Provider returned no valid commit.".into())
}

fn archive_url_for_custom(
    custom: &super::CustomAddonDefinition,
    sha: &str,
) -> Result<String, String> {
    let path = if custom.repository_path.is_empty() {
        vec![custom.github_owner.clone(), custom.github_repo.clone()]
    } else {
        custom.repository_path.clone()
    };
    match custom.provider {
        super::RepositoryProvider::GitHub => Ok(format!(
            "https://github.com/{}/{}/archive/{sha}.zip",
            path[0],
            path.last().unwrap()
        )),
        super::RepositoryProvider::GitLab => Ok(format!(
            "{}/-/archive/{sha}/{}-{sha}.zip",
            custom.repository_url,
            path.last().unwrap()
        )),
        super::RepositoryProvider::Gitea => {
            Ok(format!("{}/archive/{sha}.zip", custom.repository_url))
        }
    }
}

fn commit_sha_for(addon: &AddonDefinition, reference: Option<&str>) -> Result<String, String> {
    let url = if let Some(reference) = reference {
        let mut url = Url::parse(&format!(
            "https://api.github.com/repos/{}/{}/commits",
            addon.github_owner, addon.github_repo
        ))
        .map_err(|error| error.to_string())?;
        url.path_segments_mut()
            .map_err(|_| "Could not construct GitHub commit URL.".to_string())?
            .push(reference);
        url.to_string()
    } else {
        format!(
            "https://api.github.com/repos/{}/{}/commits?per_page=1",
            addon.github_owner, addon.github_repo
        )
    };
    if reference.is_some() {
        let commit: GitHubCommit = get_json(&url)?;
        return is_sha(&commit.sha)
            .then_some(commit.sha)
            .ok_or_else(|| format!("GitHub returned no valid commit for {}.", addon.name));
    }
    let commits: Vec<GitHubCommit> = get_json(&url)?;
    commits
        .into_iter()
        .next()
        .map(|commit| commit.sha)
        .filter(|sha| is_sha(sha))
        .ok_or_else(|| format!("GitHub returned no valid commit for {}.", addon.name))
}

fn valid_repo_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}
fn valid_install_folder(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value != "."
        && value != ".."
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, ' ' | '-' | '_')
        })
}

pub fn uninstall_addon(game_directory: &str, state: &AddonInstallState) -> Result<(), String> {
    let game_root = Path::new(game_directory)
        .canonicalize()
        .map_err(|error| format!("Could not resolve game directory: {error}"))?;
    for relative_path in &state.installed_files {
        let relative = safe_relative_path(relative_path)?;
        let full_path = game_root.join(relative);
        if full_path.is_file() {
            fs::remove_file(&full_path)
                .map_err(|error| format!("Could not remove {relative_path}: {error}"))?;
        }
    }
    // Remove empty owned ancestor directories only; leave user files intact.
    let addons_root = game_root.join("Interface").join("AddOns");
    let mut directories: HashSet<PathBuf> = state
        .installed_files
        .iter()
        .filter_map(|path| {
            let path = safe_relative_path(path).ok()?;
            game_root.join(path).parent().map(Path::to_path_buf)
        })
        .collect();
    while let Some(directory) = directories.iter().next().cloned() {
        directories.remove(&directory);
        if directory.starts_with(&addons_root) {
            let _ = fs::remove_dir(&directory);
            if let Some(parent) = directory.parent() {
                directories.insert(parent.to_path_buf());
            }
        }
    }
    Ok(())
}

fn extract_archive(
    bytes: &[u8],
    staging: &Path,
    addon_folder: &str,
) -> Result<Vec<String>, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| format!("Downloaded archive is not a valid ZIP: {error}"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err("The addon archive contains too many entries.".into());
    }
    let mut installed_files = Vec::new();
    let mut extracted_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("Could not inspect archive entry: {error}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let mut parts = name.split('/');
        let _archive_root = parts
            .next()
            .filter(|part| !part.is_empty())
            .ok_or_else(|| "Archive entry is missing its root directory.".to_string())?;
        let relative: PathBuf = parts
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        if relative.as_os_str().is_empty()
            || relative.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(format!("Unsafe path in addon archive: {name}"));
        }
        extracted_bytes = extracted_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "Addon extraction size overflowed.".to_string())?;
        if extracted_bytes > MAX_EXTRACTED_BYTES {
            return Err("The addon archive exceeds the extracted-size safety limit.".into());
        }
        let target = staging.join(&relative);
        if !target.starts_with(staging) {
            return Err(format!("Unsafe path in addon archive: {name}"));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create extracted addon directory: {error}"))?;
        }
        let mut output = fs::File::create(&target)
            .map_err(|error| format!("Could not extract {name}: {error}"))?;
        std::io::copy(&mut entry, &mut output)
            .map_err(|error| format!("Could not extract {name}: {error}"))?;
        let game_relative = Path::new("Interface")
            .join("AddOns")
            .join(addon_folder)
            .join(relative);
        installed_files.push(game_relative.to_string_lossy().replace('/', "\\"));
    }
    Ok(installed_files)
}

fn safe_relative_path(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!("Refusing unsafe persisted addon path: {value}"));
    }
    Ok(path.to_path_buf())
}
fn remove_directory_if_exists(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|error| format!("Could not remove {}: {error}", path.display()))?;
    }
    Ok(())
}
fn is_sha(value: &str) -> bool {
    value.len() == 40 && value.chars().all(|character| character.is_ascii_hexdigit())
}
fn get_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<T, String> {
    let bytes = download_bounded(url)?;
    serde_json::from_slice(&bytes).map_err(|error| format!("GitHub returned invalid JSON: {error}"))
}
fn download_bounded(url: &str) -> Result<Vec<u8>, String> {
    let parsed = Url::parse(url).map_err(|error| format!("Invalid addon URL: {error}"))?;
    validate_url(&parsed)?;
    let client = Client::builder()
        .user_agent("Octo-Updater/0.1")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .redirect(Policy::custom(|attempt| {
            match validate_url(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(error) => attempt.error(error),
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
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES as u64)
    {
        return Err(format!("{url} exceeds the download safety limit."));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 32 * 1024];
    loop {
        let count = response
            .read(&mut chunk)
            .map_err(|error| format!("Could not read {url}: {error}"))?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_ARCHIVE_BYTES {
            return Err(format!("{url} exceeds the download safety limit."));
        }
    }
    Ok(bytes)
}
fn validate_url(url: &Url) -> Result<(), String> {
    if url.scheme() != "https" {
        return Err(format!("Refusing non-HTTPS addon URL: {url}"));
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if !ADDON_DOWNLOAD_HOSTS.contains(&host.as_str()) {
        return Err(format!("Refusing addon URL from unexpected host: {host}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persisted_paths_must_be_relative() {
        assert!(safe_relative_path("Interface\\AddOns\\Test\\x.lua").is_ok());
        assert!(safe_relative_path("..\\x.lua").is_err());
        assert!(safe_relative_path("C:\\x.lua").is_err());
    }
    #[test]
    fn custom_repository_requires_a_plain_github_repository_url() {
        let request = super::super::CustomAddonRequest {
            repository_url: "https://github.com/example/Test-Addon.git".into(),
            reference: Some("release/v1".into()),
            install_folder: Some("Test Addon".into()),
        };
        let addon = custom_addon_definition(request).unwrap();
        assert_eq!(
            addon.repository_url,
            "https://github.com/example/Test-Addon"
        );
        assert_eq!(addon.reference.as_deref(), Some("release/v1"));
        assert!(custom_addon_definition(super::super::CustomAddonRequest {
            repository_url: "https://github.com/example/repo/tree/main".into(),
            reference: None,
            install_folder: None,
        })
        .is_err());
    }
    #[test]
    fn custom_install_folder_must_be_a_safe_single_component() {
        assert!(!valid_install_folder(".."));
        assert!(!valid_install_folder("folder/name"));
        assert!(valid_install_folder("My Addon_2"));
    }
}
