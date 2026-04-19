use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OriginScope {
    Home,
    XdgConfig,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManagedState {
    Unmanaged,
    ManagedActive,
    ManagedInactive,
    Conflicted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DotfileEntry {
    pub id: String,
    pub display_name: String,
    pub path: PathBuf,
    pub origin: OriginScope,
    pub kind: EntryKind,
    pub managed_state: ManagedState,
    pub managed_source: Option<PathBuf>,
    pub symlink_target: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub warning: Option<String>,
    #[serde(default)]
    pub shared_profiles: Vec<String>,
}

impl DotfileEntry {
    pub fn status_label(&self) -> &'static str {
        match self.managed_state {
            ManagedState::Unmanaged => "Unmanaged",
            ManagedState::ManagedActive => "Active",
            ManagedState::ManagedInactive => "Inactive",
            ManagedState::Conflicted => "Conflicted",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScanReport {
    pub entries: Vec<DotfileEntry>,
    pub warnings: Vec<String>,
    pub conflicts: Vec<String>,
    pub skipped_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub repo_root: Option<PathBuf>,
    pub include_hidden_home: bool,
    pub include_xdg_config: bool,
    pub backup_enabled: bool,
    pub remote_name: String,
    #[serde(default)]
    pub onboarding_complete: bool,
    #[serde(default)]
    pub custom_paths: Vec<PathBuf>,
    #[serde(default = "default_profiles")]
    pub profiles: Vec<String>,
    #[serde(default = "default_active_profile")]
    pub active_profile: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            repo_root: None,
            include_hidden_home: true,
            include_xdg_config: true,
            backup_enabled: true,
            remote_name: "origin".to_string(),
            onboarding_complete: false,
            custom_paths: Vec::new(),
            profiles: default_profiles(),
            active_profile: default_active_profile(),
        }
    }
}

fn default_profiles() -> Vec<String> {
    vec![default_active_profile()]
}

fn default_active_profile() -> String {
    "default".to_string()
}

impl AppConfig {
    pub fn ensure_active_profile(&mut self) {
        if self.profiles.is_empty() {
            self.profiles = default_profiles();
        }
        if !self
            .profiles
            .iter()
            .any(|profile| profile == &self.active_profile)
        {
            self.active_profile = self.profiles[0].clone();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagedRecord {
    pub id: String,
    #[serde(default = "default_active_profile")]
    pub profile: String,
    pub active_path: PathBuf,
    pub managed_source: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub origin: OriginScope,
}

#[derive(Debug, Clone, Default)]
pub struct GitRepoState {
    pub repo_root: Option<PathBuf>,
    pub current_branch: Option<String>,
    pub remotes: Vec<String>,
    pub remote_details: Vec<String>,
    pub staged_files: Vec<String>,
    pub unstaged_files: Vec<String>,
    pub untracked_files: Vec<String>,
}

impl GitRepoState {
    pub fn is_dirty(&self) -> bool {
        !(self.staged_files.is_empty()
            && self.unstaged_files.is_empty()
            && self.untracked_files.is_empty())
    }
}

#[derive(Debug, Clone, Default)]
pub struct OperationResult {
    pub success: bool,
    pub message: String,
    pub filesystem_changes: Vec<String>,
    pub git_changes: Vec<String>,
}
