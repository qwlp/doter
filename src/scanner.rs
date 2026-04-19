use crate::model::{DotfileEntry, EntryKind, ManagedState, OriginScope, ScanReport};
use crate::state::PersistedState;
use anyhow::Result;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn scan_dotfiles(state: &PersistedState) -> Result<ScanReport> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Unable to resolve home directory"))?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    scan_dotfiles_for_roots(state, &home, &xdg)
}

pub fn scan_dotfiles_for_roots(
    state: &PersistedState,
    home: &Path,
    xdg_config: &Path,
) -> Result<ScanReport> {
    let mut seen_paths = BTreeSet::new();
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut conflicts = Vec::new();
    let mut skipped_paths = Vec::new();

    if state.config.include_hidden_home && home.exists() {
        for item in fs::read_dir(home)? {
            let item = item?;
            let path = item.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                skipped_paths.push(path);
                continue;
            };
            if !name.starts_with('.') || name == "." || name == ".." || name == ".config" {
                continue;
            }
            if matches!(name, ".cache" | ".local" | ".cargo" | ".rustup") {
                continue;
            }
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            match classify_entry_for_roots(state, &path, OriginScope::Home, home, xdg_config) {
                Ok(entry) => {
                    if entry.managed_state == ManagedState::Conflicted {
                        conflicts.push(entry.path.display().to_string());
                    }
                    entries.push(entry);
                }
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
    }

    if state.config.include_xdg_config && xdg_config.exists() {
        for item in fs::read_dir(xdg_config)? {
            let item = item?;
            let path = item.path();
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            match classify_entry_for_roots(state, &path, OriginScope::XdgConfig, home, xdg_config) {
                Ok(entry) => {
                    if entry.managed_state == ManagedState::Conflicted {
                        conflicts.push(entry.path.display().to_string());
                    }
                    entries.push(entry);
                }
                Err(error) => warnings.push(format!("{}: {error}", path.display())),
            }
        }
    }

    for path in &state.config.custom_paths {
        if !path.exists() {
            warnings.push(format!("{}: configured custom path does not exist", path.display()));
            continue;
        }
        if !seen_paths.insert(path.clone()) {
            continue;
        }
        match classify_entry_for_roots(state, path, OriginScope::Custom, home, xdg_config) {
            Ok(entry) => {
                if entry.managed_state == ManagedState::Conflicted {
                    conflicts.push(entry.path.display().to_string());
                }
                entries.push(entry);
            }
            Err(error) => warnings.push(format!("{}: {error}", path.display())),
        }
    }

    let active_profile = state.active_profile().to_string();
    if let Some(repo_root) = state.config.repo_root.as_ref() {
        if state.config.include_hidden_home {
            collect_repo_profile_entries(
                &mut entries,
                &mut seen_paths,
                state,
                &repo_root.join("profiles").join(&active_profile).join("home"),
                OriginScope::Home,
                home,
                xdg_config,
            )?;
        }
        if state.config.include_xdg_config {
            collect_repo_profile_entries(
                &mut entries,
                &mut seen_paths,
                state,
                &repo_root.join("profiles").join(&active_profile).join("config"),
                OriginScope::XdgConfig,
                home,
                xdg_config,
            )?;
        }
    }

    entries.sort_by(|left, right| left.display_name.cmp(&right.display_name));

    Ok(ScanReport {
        entries,
        warnings,
        conflicts,
        skipped_paths,
    })
}

pub fn classify_entry(
    state: &PersistedState,
    path: &Path,
    origin: OriginScope,
) -> Result<DotfileEntry> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Unable to resolve home directory"))?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    classify_entry_for_roots(state, path, origin, &home, &xdg)
}

fn classify_entry_for_roots(
    state: &PersistedState,
    path: &Path,
    origin: OriginScope,
    home: &Path,
    xdg_config: &Path,
) -> Result<DotfileEntry> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if metadata.is_file() {
        EntryKind::File
    } else if metadata.is_dir() {
        EntryKind::Directory
    } else {
        EntryKind::Unknown
    };

    let id = stable_id(origin, path);
    let active_profile = state.active_profile();
    let record = state.find_record(active_profile, path);
    let expected_managed_source = state
        .config
        .repo_root
        .as_ref()
        .map(|repo_root| expected_managed_source(repo_root, active_profile, origin, path, home, xdg_config));
    let symlink_target = if file_type.is_symlink() {
        fs::read_link(path).ok()
    } else {
        None
    };
    let repo_root = state.config.repo_root.as_ref();

    let managed_state = if let Some(record) = record {
        if file_type.is_symlink() && symlink_target.as_ref() == Some(&record.managed_source) {
            ManagedState::ManagedActive
        } else {
            ManagedState::Conflicted
        }
    } else if let (Some(target), Some(repo_root)) = (&symlink_target, repo_root) {
        if target.starts_with(repo_root.join("profiles").join(active_profile)) {
            ManagedState::ManagedActive
        } else if expected_managed_source
            .as_ref()
            .map(|source| source.exists())
            .unwrap_or(false)
        {
            ManagedState::ManagedInactive
        } else {
            ManagedState::Unmanaged
        }
    } else if expected_managed_source
        .as_ref()
        .map(|source| source.exists())
        .unwrap_or(false)
    {
        ManagedState::ManagedInactive
    } else {
        ManagedState::Unmanaged
    };

    let warning = if managed_state == ManagedState::Conflicted {
        if file_type.is_symlink() {
            let target_label = symlink_target
                .as_ref()
                .map(|target| target.display().to_string())
                .unwrap_or_else(|| "(unknown target)".to_string());
            Some(format!(
                "Path has managed metadata but points at a different symlink target: {target_label}"
            ))
        } else {
            Some("Path has managed metadata but no matching app symlink".to_string())
        }
    } else {
        None
    };
    let managed_source = record
        .map(|record| record.managed_source.clone())
        .or_else(|| {
            if managed_state == ManagedState::ManagedActive {
                symlink_target.clone()
            } else if managed_state == ManagedState::ManagedInactive {
                expected_managed_source.clone()
            } else {
                None
            }
        });

    Ok(DotfileEntry {
        id,
        display_name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        path: path.to_path_buf(),
        origin,
        kind,
        managed_state,
        managed_source,
        symlink_target,
        backup_path: record.and_then(|record| record.backup_path.clone()),
        warning,
    })
}

fn expected_managed_source(
    repo_root: &Path,
    profile: &str,
    origin: OriginScope,
    active_path: &Path,
    home: &Path,
    xdg_config: &Path,
) -> PathBuf {
    match origin {
        OriginScope::Home => repo_root
            .join("profiles")
            .join(profile)
            .join("home")
            .join(active_path.strip_prefix(home).unwrap_or(active_path)),
        OriginScope::XdgConfig => repo_root
            .join("profiles")
            .join(profile)
            .join("config")
            .join(active_path.strip_prefix(xdg_config).unwrap_or(active_path)),
        OriginScope::Custom => repo_root
            .join("profiles")
            .join(profile)
            .join("custom")
            .join(custom_relative_path(active_path)),
    }
}

fn custom_relative_path(active_path: &Path) -> PathBuf {
    let mut relative = PathBuf::new();
    if active_path.is_absolute() {
        relative.push("absolute");
    } else {
        relative.push("relative");
    }

    for component in active_path.components() {
        use std::path::Component;
        match component {
            Component::Prefix(prefix) => relative.push(prefix.as_os_str()),
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => relative.push("parent"),
            Component::Normal(part) => relative.push(part),
        }
    }

    relative
}

fn collect_repo_profile_entries(
    entries: &mut Vec<DotfileEntry>,
    seen_paths: &mut BTreeSet<PathBuf>,
    state: &PersistedState,
    managed_root: &Path,
    origin: OriginScope,
    home: &Path,
    xdg_config: &Path,
) -> Result<()> {
    if !managed_root.exists() {
        return Ok(());
    }

    for item in fs::read_dir(managed_root)? {
        let item = item?;
        let managed_source = item.path();
        let live_path = match origin {
            OriginScope::Home => home.join(item.file_name()),
            OriginScope::XdgConfig => xdg_config.join(item.file_name()),
            OriginScope::Custom => continue,
        };
        if !seen_paths.insert(live_path.clone()) {
            continue;
        }
        entries.push(repo_only_entry(state, live_path, managed_source, origin));
    }
    Ok(())
}

fn repo_only_entry(
    state: &PersistedState,
    live_path: PathBuf,
    managed_source: PathBuf,
    origin: OriginScope,
) -> DotfileEntry {
    let kind = fs::symlink_metadata(&managed_source)
        .map(|metadata| {
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                EntryKind::Symlink
            } else if metadata.is_file() {
                EntryKind::File
            } else if metadata.is_dir() {
                EntryKind::Directory
            } else {
                EntryKind::Unknown
            }
        })
        .unwrap_or(EntryKind::Unknown);
    let backup_path = state
        .find_record(state.active_profile(), &live_path)
        .and_then(|record| record.backup_path.clone());

    DotfileEntry {
        id: stable_id(origin, &live_path),
        display_name: live_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        path: live_path,
        origin,
        kind,
        managed_state: ManagedState::ManagedInactive,
        managed_source: Some(managed_source),
        symlink_target: None,
        backup_path,
        warning: None,
    }
}

pub fn stable_id(origin: OriginScope, path: &Path) -> String {
    let prefix = match origin {
        OriginScope::Home => "home",
        OriginScope::XdgConfig => "xdg",
        OriginScope::Custom => "custom",
    };
    format!("{prefix}:{}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppConfig, ManagedRecord};
    use crate::state::PersistedState;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn classifies_home_dotfiles() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        fs::create_dir_all(&xdg).unwrap();
        fs::write(home.join(".bashrc"), "set -o vi").unwrap();

        let state = PersistedState {
            config: AppConfig::default(),
            managed_entries: Vec::new(),
        };

        let report = scan_dotfiles_for_roots(&state, &home, &xdg).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].display_name, ".bashrc");
        assert_eq!(report.entries[0].managed_state, ManagedState::Unmanaged);
    }

    #[test]
    fn detects_managed_symlink() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&xdg).unwrap();
        fs::create_dir_all(repo_root.join("profiles/default/home")).unwrap();
        let managed_source = repo_root.join("profiles/default/home/.zshrc");
        fs::write(&managed_source, "export TEST=1").unwrap();
        std::os::unix::fs::symlink(&managed_source, home.join(".zshrc")).unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: stable_id(OriginScope::Home, &home.join(".zshrc")),
                profile: "default".to_string(),
                active_path: home.join(".zshrc"),
                managed_source: managed_source.clone(),
                backup_path: None,
                origin: OriginScope::Home,
            }],
        };

        let entry = classify_entry(&state, &home.join(".zshrc"), OriginScope::Home).unwrap();
        assert_eq!(entry.managed_state, ManagedState::ManagedActive);
    }

    #[test]
    fn infers_managed_source_from_active_symlink_without_record() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&xdg).unwrap();
        fs::create_dir_all(repo_root.join("profiles/desktop-arch/config")).unwrap();
        let managed_source = repo_root.join("profiles/desktop-arch/config/nvim");
        std::fs::write(&managed_source, "set number").unwrap();
        std::os::unix::fs::symlink(&managed_source, xdg.join("nvim")).unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };

        let entry = classify_entry(&state, &xdg.join("nvim"), OriginScope::XdgConfig).unwrap();
        assert_eq!(entry.managed_state, ManagedState::ManagedActive);
        assert_eq!(entry.managed_source, Some(managed_source));
    }

    #[test]
    fn scans_configured_custom_paths() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        let custom_file = temp.path().join("work/random-tool/config.toml");
        fs::create_dir_all(custom_file.parent().unwrap()).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&xdg).unwrap();
        fs::write(&custom_file, "enabled = true").unwrap();

        let state = PersistedState {
            config: AppConfig {
                custom_paths: vec![custom_file.clone()],
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };

        let report = scan_dotfiles_for_roots(&state, &home, &xdg).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].origin, OriginScope::Custom);
        assert_eq!(report.entries[0].path, custom_file);
    }

    #[test]
    fn marks_local_entry_in_repo_but_not_active_as_inactive() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&xdg).unwrap();
        fs::create_dir_all(repo_root.join("profiles/desktop-arch/config")).unwrap();
        fs::write(xdg.join("nvim"), "local config").unwrap();
        fs::write(repo_root.join("profiles/desktop-arch/config/nvim"), "repo config").unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };

        let entry = classify_entry_for_roots(&state, &xdg.join("nvim"), OriginScope::XdgConfig, &home, &xdg)
            .unwrap();
        assert_eq!(entry.managed_state, ManagedState::ManagedInactive);
        assert!(entry.managed_source.is_some());
    }

    #[test]
    fn includes_repo_entries_missing_from_live_path_as_inactive() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&xdg).unwrap();
        fs::create_dir_all(repo_root.join("profiles/desktop-arch/config")).unwrap();
        fs::write(repo_root.join("profiles/desktop-arch/config/nvim"), "repo config").unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };

        let report = scan_dotfiles_for_roots(&state, &home, &xdg).unwrap();
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].display_name, "nvim");
        assert_eq!(report.entries[0].managed_state, ManagedState::ManagedInactive);
    }

    #[test]
    fn treats_broken_wrong_symlink_as_conflicted() {
        let temp = tempdir().unwrap();
        let home = temp.path().join("home");
        let xdg = home.join(".config");
        let repo_root = temp.path().join("repo");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&xdg).unwrap();
        fs::create_dir_all(repo_root.join("profiles/default/config")).unwrap();
        let managed_source = repo_root.join("profiles/default/config/nvim");
        let stale_target = temp.path().join("old-repo/profiles/default/config/nvim");
        std::os::unix::fs::symlink(&stale_target, xdg.join("nvim")).unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: stable_id(OriginScope::XdgConfig, &xdg.join("nvim")),
                profile: "default".to_string(),
                active_path: xdg.join("nvim"),
                managed_source: managed_source.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };

        let entry = classify_entry(&state, &xdg.join("nvim"), OriginScope::XdgConfig).unwrap();
        assert_eq!(entry.managed_state, ManagedState::Conflicted);
        assert_eq!(entry.symlink_target, Some(stale_target));
        assert!(
            entry.warning.unwrap().contains("different symlink target"),
            "expected warning to mention wrong symlink target"
        );
    }
}
