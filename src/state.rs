use crate::model::{AppConfig, ManagedRecord};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "doter";
const CONFIG_FILE: &str = "config.toml";
const STATE_FILE: &str = "state.toml";
const PROFILE_CUSTOM_PATHS_FILE: &str = "custom-paths.toml";
const SHARED_LINKS_FILE: &str = "links.toml";

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub backup_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let config_base = dirs::config_dir().context("Unable to resolve config directory")?;
        let data_base = dirs::data_dir().context("Unable to resolve data directory")?;
        let config_dir = config_base.join(APP_DIR);
        let data_dir = data_base.join(APP_DIR);
        let backup_dir = data_dir.join("backups");

        fs::create_dir_all(&config_dir)?;
        fs::create_dir_all(&backup_dir)?;

        Ok(Self {
            config_dir,
            data_dir,
            backup_dir,
        })
    }

    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILE)
    }

    pub fn state_path(&self) -> PathBuf {
        self.data_dir.join(STATE_FILE)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PersistedState {
    pub config: AppConfig,
    pub managed_entries: Vec<ManagedRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProfileCustomPathsFile {
    #[serde(default)]
    paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharedLinksFile {
    #[serde(default)]
    pub entries: Vec<SharedLinkEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedLinkEntry {
    pub origin: crate::model::OriginScope,
    pub key: String,
    #[serde(default)]
    pub profiles: Vec<String>,
}

impl PersistedState {
    pub fn load(paths: &AppPaths) -> Result<Self> {
        let mut state = Self::default();
        if paths.config_path().exists() {
            let content = fs::read_to_string(paths.config_path())?;
            state.config = toml::from_str(&content).context("Failed to parse config.toml")?;
        }
        if paths.state_path().exists() {
            let content = fs::read_to_string(paths.state_path())?;
            let loaded: Self = toml::from_str(&content).context("Failed to parse state.toml")?;
            state.managed_entries = loaded.managed_entries;
        }
        state.sync_profiles_from_repo()?;
        state.config.ensure_active_profile();
        Ok(state)
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        fs::write(paths.config_path(), toml::to_string_pretty(&self.config)?)?;
        fs::write(paths.state_path(), toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn prune_stale_managed_entries(&mut self) -> bool {
        let original_len = self.managed_entries.len();
        self.managed_entries.retain(|record| {
            if record.managed_source.exists() {
                return true;
            }

            let active_points_to_missing_source = fs::symlink_metadata(&record.active_path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
                && fs::read_link(&record.active_path)
                    .map(|target| target == record.managed_source)
                    .unwrap_or(false);

            active_points_to_missing_source
        });

        self.managed_entries.len() != original_len
    }

    pub fn active_profile(&self) -> &str {
        &self.config.active_profile
    }

    pub fn find_record(&self, profile: &str, path: &Path) -> Option<&ManagedRecord> {
        self.managed_entries
            .iter()
            .find(|record| record.profile == profile && record.active_path == path)
    }

    pub fn upsert_record(&mut self, record: ManagedRecord) {
        if let Some(existing) = self
            .managed_entries
            .iter_mut()
            .find(|existing| existing.profile == record.profile && existing.id == record.id)
        {
            *existing = record;
        } else {
            self.managed_entries.push(record);
        }
    }

    pub fn sync_profiles_from_repo(&mut self) -> Result<bool> {
        let Some(repo_root) = self.config.repo_root.as_ref() else {
            self.config.ensure_active_profile();
            return Ok(false);
        };

        let profiles_root = repo_root.join("profiles");
        if !profiles_root.exists() {
            self.config.ensure_active_profile();
            return Ok(false);
        }

        let original = self.config.profiles.clone();
        let mut discovered = BTreeSet::new();

        for entry in fs::read_dir(&profiles_root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            discovered.insert(name.to_string());
        }

        if !discovered.is_empty() {
            self.config.profiles = discovered.into_iter().collect();
        }
        self.config.ensure_active_profile();
        Ok(self.config.profiles != original)
    }

    pub fn resolve_active_custom_paths_for_roots(
        &self,
        home_root: &Path,
        xdg_root: &Path,
    ) -> Result<Vec<PathBuf>> {
        let mut resolved = BTreeSet::new();
        for path in &self.config.custom_paths {
            resolved.insert(path.clone());
        }
        for template in self.active_custom_path_templates()? {
            resolved.insert(resolve_custom_path_template(&template, home_root, xdg_root));
        }
        Ok(resolved.into_iter().collect())
    }

    pub fn add_active_custom_path_for_roots(
        &mut self,
        path: &Path,
        home_root: &Path,
        xdg_root: &Path,
    ) -> Result<()> {
        if !path.exists() {
            anyhow::bail!("{} does not exist", path.display());
        }
        if !is_portable_custom_path(path, home_root, xdg_root) {
            anyhow::bail!(
                "{} is machine-specific. Track only paths under $HOME or $XDG_CONFIG_HOME.",
                path.display()
            );
        }

        if self.config.repo_root.is_none() {
            if self.config.custom_paths.iter().any(|existing| existing == path) {
                anyhow::bail!("{} is already being tracked", path.display());
            }
            self.config.custom_paths.push(path.to_path_buf());
            self.config.custom_paths.sort();
            return Ok(());
        }

        let template = portable_custom_path_template(path, home_root, xdg_root);
        let mut templates = self.active_custom_path_templates()?;
        if templates.iter().any(|existing| existing == &template)
            || self
                .config
                .custom_paths
                .iter()
                .any(|existing| existing == path)
        {
            anyhow::bail!("{} is already being tracked", path.display());
        }
        templates.push(template);
        templates.sort();
        templates.dedup();
        self.save_active_custom_path_templates(&templates)
    }

    pub fn active_custom_path_templates(&self) -> Result<Vec<String>> {
        let Some(repo_root) = self.config.repo_root.as_ref() else {
            return Ok(Vec::new());
        };
        let path = profile_custom_paths_path(repo_root, self.active_profile());
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&path)?;
        let file: ProfileCustomPathsFile =
            toml::from_str(&content).context("Failed to parse custom-paths.toml")?;
        Ok(file.paths)
    }

    fn save_active_custom_path_templates(&self, templates: &[String]) -> Result<()> {
        let Some(repo_root) = self.config.repo_root.as_ref() else {
            return Ok(());
        };
        let path = profile_custom_paths_path(repo_root, self.active_profile());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = ProfileCustomPathsFile {
            paths: templates.to_vec(),
        };
        fs::write(path, toml::to_string_pretty(&file)?)?;
        Ok(())
    }

    pub fn load_shared_links(&self) -> Result<SharedLinksFile> {
        let Some(repo_root) = self.config.repo_root.as_ref() else {
            return Ok(SharedLinksFile::default());
        };
        let path = shared_links_path(repo_root);
        if !path.exists() {
            return Ok(SharedLinksFile::default());
        }
        let content = fs::read_to_string(&path)?;
        toml::from_str(&content).context("Failed to parse shared links manifest")
    }

    pub fn save_shared_links(&self, links: &SharedLinksFile) -> Result<()> {
        let Some(repo_root) = self.config.repo_root.as_ref() else {
            return Ok(());
        };
        let path = shared_links_path(repo_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, toml::to_string_pretty(links)?)?;
        Ok(())
    }
}

fn profile_custom_paths_path(repo_root: &Path, profile: &str) -> PathBuf {
    repo_root
        .join("profiles")
        .join(profile)
        .join(PROFILE_CUSTOM_PATHS_FILE)
}

pub fn shared_links_path(repo_root: &Path) -> PathBuf {
    repo_root.join("shared").join(SHARED_LINKS_FILE)
}

pub fn portable_custom_path_template(path: &Path, home_root: &Path, xdg_root: &Path) -> String {
    if let Ok(relative) = path.strip_prefix(xdg_root) {
        return join_template("$XDG_CONFIG_HOME", relative);
    }
    if let Ok(relative) = path.strip_prefix(home_root) {
        return join_template("$HOME", relative);
    }
    path.to_string_lossy().to_string()
}

pub fn resolve_custom_path_template(
    template: &str,
    home_root: &Path,
    xdg_root: &Path,
) -> PathBuf {
    if template == "$XDG_CONFIG_HOME" {
        return xdg_root.to_path_buf();
    }
    if let Some(relative) = template.strip_prefix("$XDG_CONFIG_HOME/") {
        return xdg_root.join(relative);
    }
    if template == "$HOME" {
        return home_root.to_path_buf();
    }
    if let Some(relative) = template.strip_prefix("$HOME/") {
        return home_root.join(relative);
    }
    PathBuf::from(template)
}

fn join_template(prefix: &str, relative: &Path) -> String {
    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}/{relative}")
    }
}

fn is_portable_custom_path(path: &Path, home_root: &Path, xdg_root: &Path) -> bool {
    path.starts_with(xdg_root) || path.starts_with(home_root)
}

pub fn portable_entry_key(
    origin: crate::model::OriginScope,
    path: &Path,
    home_root: &Path,
    xdg_root: &Path,
) -> Result<String> {
    let key = match origin {
        crate::model::OriginScope::Home => path.strip_prefix(home_root)?.to_path_buf(),
        crate::model::OriginScope::XdgConfig => path.strip_prefix(xdg_root)?.to_path_buf(),
        crate::model::OriginScope::Custom => {
            let template = portable_custom_path_template(path, home_root, xdg_root);
            PathBuf::from(template)
        }
    };
    Ok(key.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppConfig, ManagedRecord, OriginScope};
    use tempfile::tempdir;

    #[test]
    fn prunes_stale_record_when_repo_source_is_missing_and_path_is_not_linked() {
        let temp = tempdir().unwrap();
        let active_path = temp.path().join("pelec");
        std::fs::create_dir_all(&active_path).unwrap();

        let mut state = PersistedState {
            config: AppConfig::default(),
            managed_entries: vec![ManagedRecord {
                id: "xdg:/tmp/pelec".to_string(),
                profile: "default".to_string(),
                active_path: active_path.clone(),
                managed_source: temp.path().join("repo/profiles/default/config/pelec"),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };

        assert!(state.prune_stale_managed_entries());
        assert!(state.managed_entries.is_empty());
    }

    #[test]
    fn syncs_profiles_from_repo_root() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(repo_root.join("profiles/desktop-arch")).unwrap();
        std::fs::create_dir_all(repo_root.join("profiles/laptop")).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["default".to_string()],
                active_profile: "default".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![],
        };

        let changed = state.sync_profiles_from_repo().unwrap();

        assert!(changed);
        assert_eq!(
            state.config.profiles,
            vec!["desktop-arch".to_string(), "laptop".to_string()]
        );
    }

    #[test]
    fn preserves_active_profile_when_found_on_disk() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(repo_root.join("profiles/desktop-arch")).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["default".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![],
        };

        state.sync_profiles_from_repo().unwrap();

        assert_eq!(state.config.active_profile, "desktop-arch");
        assert!(state
            .config
            .profiles
            .iter()
            .any(|profile| profile == "desktop-arch"));
    }

    #[test]
    fn stores_custom_paths_in_profile_repo_file_with_portable_templates() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let home_root = temp.path().join("home");
        let xdg_root = home_root.join(".config");
        std::fs::create_dir_all(repo_root.join("profiles/desktop-arch")).unwrap();
        std::fs::create_dir_all(xdg_root.join("waybar")).unwrap();
        let custom_path = xdg_root.join("waybar/style.css");
        std::fs::write(&custom_path, "body{}").unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![],
        };

        state
            .add_active_custom_path_for_roots(&custom_path, &home_root, &xdg_root)
            .unwrap();

        let manifest = std::fs::read_to_string(
            repo_root.join("profiles/desktop-arch/custom-paths.toml"),
        )
        .unwrap();
        assert!(manifest.contains("$XDG_CONFIG_HOME/waybar/style.css"));
        assert!(state.config.custom_paths.is_empty());
    }

    #[test]
    fn resolves_custom_paths_from_profile_repo_file() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let home_root = temp.path().join("home");
        let xdg_root = home_root.join(".config");
        std::fs::create_dir_all(repo_root.join("profiles/desktop-arch")).unwrap();
        std::fs::write(
            repo_root.join("profiles/desktop-arch/custom-paths.toml"),
            "paths = [\"$HOME/.local/share/foo.json\", \"$XDG_CONFIG_HOME/waybar/style.css\"]\n",
        )
        .unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![],
        };

        let resolved = state
            .resolve_active_custom_paths_for_roots(&home_root, &xdg_root)
            .unwrap();
        assert!(resolved
            .iter()
            .any(|path| path == &home_root.join(".local/share/foo.json")));
        assert!(resolved
            .iter()
            .any(|path| path == &xdg_root.join("waybar/style.css")));
    }

    #[test]
    fn rejects_machine_specific_custom_paths() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let home_root = temp.path().join("home");
        let xdg_root = home_root.join(".config");
        let machine_specific = temp.path().join("opt/tool/config.toml");
        std::fs::create_dir_all(repo_root.join("profiles/desktop-arch")).unwrap();
        std::fs::create_dir_all(machine_specific.parent().unwrap()).unwrap();
        std::fs::write(&machine_specific, "enabled = true").unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![],
        };

        let error = state
            .add_active_custom_path_for_roots(&machine_specific, &home_root, &xdg_root)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Track only paths under $HOME or $XDG_CONFIG_HOME"));
    }
}
