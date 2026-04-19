use crate::model::{AppConfig, ManagedRecord};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "doter";
const CONFIG_FILE: &str = "config.toml";
const STATE_FILE: &str = "state.toml";

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
}
