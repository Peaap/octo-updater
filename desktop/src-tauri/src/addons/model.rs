use serde::{Deserialize, Serialize};

/// A curated GitHub addon. Repositories are trusted registry entries rather
/// than user-provided URLs, and each install is pinned to an immutable commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonDefinition {
    pub id: String,
    pub folder: String,
    pub name: String,
    pub description: String,
    pub github_owner: String,
    pub github_repo: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RepositoryProvider {
    GitHub,
    GitLab,
    Gitea,
}
fn default_provider() -> RepositoryProvider {
    RepositoryProvider::GitHub
}

/// A user-supplied repository from one of the explicitly trusted public hosts.
/// Generic/self-hosted Gitea remains intentionally unsupported until it has an
/// explicit host-enrolment/trust model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAddonDefinition {
    pub id: String,
    pub repository_url: String,
    pub reference: Option<String>,
    pub install_folder: String,
    pub name: String,
    #[serde(default = "default_provider")]
    pub provider: RepositoryProvider,
    /// Repository path without the host. GitLab may contain nested groups.
    #[serde(default)]
    pub repository_path: Vec<String>,
    // Retained for backwards-compatible deserialization of existing GitHub
    // custom records; new records also populate them for display/migration.
    #[serde(default)]
    pub github_owner: String,
    #[serde(default)]
    pub github_repo: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAddonRequest {
    pub repository_url: String,
    pub reference: Option<String>,
    pub install_folder: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddonInstallState {
    pub commit_sha: String,
    pub installed_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonStatus {
    pub id: String,
    pub name: String,
    pub description: String,
    pub installed: bool,
    pub update_available: bool,
    pub custom: bool,
    pub repository_url: Option<String>,
    pub reference: Option<String>,
    pub provider: Option<RepositoryProvider>,
    /// True when the folder currently exists under Interface/AddOns.
    pub detected: bool,
    /// True only when this launcher owns persisted exact file paths for it.
    pub managed: bool,
}
