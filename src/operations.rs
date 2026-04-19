use crate::model::{DotfileEntry, ManagedRecord, ManagedState, OperationResult, OriginScope};
use crate::scanner::stable_id;
use crate::state::{AppPaths, PersistedState};
use anyhow::{Result, anyhow};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub fn enable_entry(
    state: &mut PersistedState,
    paths: &AppPaths,
    entry: &DotfileEntry,
) -> Result<OperationResult> {
    let active_profile = state.active_profile().to_string();
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before enabling dotfiles"))?;
    let managed_source = managed_path_for(&repo_root, &active_profile, entry.origin, &entry.path)?;
    if let Some(parent) = managed_source.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut result = OperationResult {
        success: true,
        message: format!("Enabled {}", entry.display_name),
        filesystem_changes: Vec::new(),
        git_changes: vec!["Working tree updated".to_string()],
    };

    let existing_managed_symlink = existing_managed_symlink_target(&repo_root, &entry.path)
        .filter(|target| target != &managed_source);
    let first_enable =
        !managed_source.exists() && entry.path.exists() && existing_managed_symlink.is_none();

    if let Some(existing_target) = existing_managed_symlink.as_ref() {
        if !managed_source.exists() {
            copy_path_preserving_links(existing_target, &managed_source)?;
            result.filesystem_changes.push(format!(
                "Copied managed content {} -> {}",
                existing_target.display(),
                managed_source.display()
            ));
        }
        fs::remove_file(&entry.path)?;
        result.filesystem_changes.push(format!(
            "Removed previous managed symlink {}",
            entry.path.display()
        ));
    } else if first_enable {
        move_path(&entry.path, &managed_source)?;
        result
            .filesystem_changes
            .push(format!("Moved {} into repository", entry.path.display()));
    }

    let backup_path = if !first_enable && entry.path.exists() {
        let backup_path = backup_path_for(paths, entry.origin, &entry.path);
        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if backup_path.exists() {
            remove_path(&backup_path)?;
        }
        move_path(&entry.path, &backup_path)?;
        result.filesystem_changes.push(format!(
            "Backed up {} to {}",
            entry.path.display(),
            backup_path.display()
        ));
        Some(backup_path)
    } else {
        None
    };

    if let Some(parent) = entry.path.parent() {
        fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(&managed_source, &entry.path)?;
    result.filesystem_changes.push(format!(
        "Created symlink {} -> {}",
        entry.path.display(),
        managed_source.display()
    ));

    state.upsert_record(ManagedRecord {
        id: stable_id(entry.origin, &entry.path),
        profile: active_profile,
        active_path: entry.path.clone(),
        managed_source,
        backup_path,
        origin: entry.origin,
    });

    Ok(result)
}

pub fn disable_entry(state: &mut PersistedState, entry: &DotfileEntry) -> Result<OperationResult> {
    let active_profile = state.active_profile().to_string();
    let record = state
        .managed_entries
        .iter()
        .find(|record| record.profile == active_profile && record.id == entry.id)
        .cloned()
        .ok_or_else(|| anyhow!("No managed record for {}", entry.display_name))?;

    let mut result = OperationResult {
        success: true,
        message: format!("Disabled {}", entry.display_name),
        filesystem_changes: Vec::new(),
        git_changes: vec!["Working tree updated".to_string()],
    };

    if fs::symlink_metadata(&record.active_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        fs::remove_file(&record.active_path)?;
        result
            .filesystem_changes
            .push(format!("Removed symlink {}", record.active_path.display()));
    }

    if let Some(backup_path) = &record.backup_path {
        if backup_path.exists() {
            move_path(backup_path, &record.active_path)?;
            result.filesystem_changes.push(format!(
                "Restored backup {} -> {}",
                backup_path.display(),
                record.active_path.display()
            ));
        }
    } else if record.managed_source.exists() {
        copy_path_preserving_links(&record.managed_source, &record.active_path)?;
        result.filesystem_changes.push(format!(
            "Restored working copy from {}",
            record.managed_source.display()
        ));
    }

    state
        .managed_entries
        .retain(|existing| !(existing.profile == active_profile && existing.id == record.id));

    Ok(result)
}

pub fn resolve_conflict_entry(
    state: &mut PersistedState,
    paths: &AppPaths,
    entry: &DotfileEntry,
) -> Result<OperationResult> {
    let active_profile = state.active_profile().to_string();
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before repairing conflicts"))?;
    let record = state
        .managed_entries
        .iter()
        .find(|record| record.profile == active_profile && record.id == entry.id)
        .cloned()
        .ok_or_else(|| anyhow!("No managed record for {}", entry.display_name))?;

    let mut result = OperationResult {
        success: true,
        message: format!("Re-linked {}", entry.display_name),
        filesystem_changes: Vec::new(),
        git_changes: vec!["Working tree updated".to_string()],
    };

    ensure_conflict_managed_source(&repo_root, &record, entry, &mut result)?;

    let is_current_managed_symlink = fs::symlink_metadata(&record.active_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
        && fs::read_link(&record.active_path)
            .map(|target| target == record.managed_source)
            .unwrap_or(false);

    let mut backup_path = record.backup_path.clone();
    if fs::symlink_metadata(&record.active_path).is_ok() && !is_current_managed_symlink {
        let new_backup = backup_path
            .clone()
            .unwrap_or_else(|| backup_path_for(paths, record.origin, &record.active_path));
        if let Some(parent) = new_backup.parent() {
            fs::create_dir_all(parent)?;
        }
        if new_backup.exists() {
            remove_path(&new_backup)?;
        }
        move_path(&record.active_path, &new_backup)?;
        result.filesystem_changes.push(format!(
            "Backed up conflicting path {} to {}",
            record.active_path.display(),
            new_backup.display()
        ));
        backup_path = Some(new_backup);
    }

    if let Some(parent) = record.active_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(&record.active_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        fs::remove_file(&record.active_path)?;
    }
    std::os::unix::fs::symlink(&record.managed_source, &record.active_path)?;
    result.filesystem_changes.push(format!(
        "Created symlink {} -> {}",
        record.active_path.display(),
        record.managed_source.display()
    ));

    if let Some(existing) = state
        .managed_entries
        .iter_mut()
        .find(|existing| existing.profile == active_profile && existing.id == record.id)
    {
        existing.backup_path = backup_path;
    }

    Ok(result)
}

pub fn remove_profile(
    state: &mut PersistedState,
    _paths: &AppPaths,
    profile: &str,
) -> Result<OperationResult> {
    if state.config.profiles.len() <= 1 {
        return Err(anyhow!("At least one profile must remain"));
    }
    if !state
        .config
        .profiles
        .iter()
        .any(|existing| existing == profile)
    {
        return Err(anyhow!("Profile {profile} does not exist"));
    }

    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before removing profiles"))?;
    let records = state
        .managed_entries
        .iter()
        .filter(|record| record.profile == profile)
        .cloned()
        .collect::<Vec<_>>();

    let mut result = OperationResult {
        success: true,
        message: format!("Removed profile {profile}"),
        filesystem_changes: Vec::new(),
        git_changes: vec![format!("Deleted profiles/{profile} from repository")],
    };

    for record in &records {
        restore_profile_record(record, &mut result)?;
    }

    let profile_root = repo_root.join("profiles").join(profile);
    if profile_root.exists() {
        remove_path(&profile_root)?;
        result
            .filesystem_changes
            .push(format!("Removed {}", profile_root.display()));
    }

    state
        .managed_entries
        .retain(|record| record.profile != profile);
    state.config.profiles.retain(|existing| existing != profile);
    if state.config.active_profile == profile {
        state.config.active_profile = state
            .config
            .profiles
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("At least one profile must remain"))?;
    }
    state.config.ensure_active_profile();

    Ok(result)
}

pub fn validate_conflict(entry: &DotfileEntry) -> Result<()> {
    if entry.managed_state == ManagedState::Conflicted {
        return Err(anyhow!(
            "{} is in a conflicted state. Disable or resolve it first.",
            entry.display_name
        ));
    }
    Ok(())
}

pub fn managed_path_for(
    repo_root: &Path,
    profile: &str,
    origin: OriginScope,
    active_path: &Path,
) -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Unable to resolve home directory"))?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    managed_path_for_roots(repo_root, profile, origin, active_path, &home, &xdg)
}

fn managed_path_for_roots(
    repo_root: &Path,
    profile: &str,
    origin: OriginScope,
    active_path: &Path,
    home_root: &Path,
    xdg_root: &Path,
) -> Result<PathBuf> {
    let relative = match origin {
        OriginScope::Home => active_path.strip_prefix(home_root)?,
        OriginScope::XdgConfig => active_path.strip_prefix(xdg_root)?,
    };
    let root = match origin {
        OriginScope::Home => repo_root.join("profiles").join(profile).join("home"),
        OriginScope::XdgConfig => repo_root.join("profiles").join(profile).join("config"),
    };
    Ok(root.join(relative))
}

fn backup_path_for(paths: &AppPaths, origin: OriginScope, active_path: &Path) -> PathBuf {
    let label = match origin {
        OriginScope::Home => "home",
        OriginScope::XdgConfig => "config",
    };
    let safe_name = active_path
        .to_string_lossy()
        .replace('/', "__")
        .replace(':', "_");
    paths.backup_dir.join(format!("{label}_{safe_name}"))
}

fn ensure_conflict_managed_source(
    repo_root: &Path,
    record: &ManagedRecord,
    entry: &DotfileEntry,
    result: &mut OperationResult,
) -> Result<()> {
    if record.managed_source.exists() {
        return Ok(());
    }

    if let Some(parent) = record.managed_source.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Some(source) = entry
        .symlink_target
        .as_ref()
        .filter(|target| *target != &record.managed_source)
        .filter(|target| target.starts_with(repo_root.join("profiles")) && target.exists())
    {
        copy_path_preserving_links(source, &record.managed_source)?;
        result.filesystem_changes.push(format!(
            "Rebuilt missing managed source {} from {}",
            record.managed_source.display(),
            source.display()
        ));
        return Ok(());
    }

    if fs::symlink_metadata(&record.active_path)
        .map(|meta| !meta.file_type().is_symlink())
        .unwrap_or(false)
    {
        copy_path_preserving_links(&record.active_path, &record.managed_source)?;
        result.filesystem_changes.push(format!(
            "Rebuilt missing managed source {} from live path {}",
            record.managed_source.display(),
            record.active_path.display()
        ));
        return Ok(());
    }

    if let Some(backup_path) = record.backup_path.as_ref().filter(|path| path.exists()) {
        copy_path_preserving_links(backup_path, &record.managed_source)?;
        result.filesystem_changes.push(format!(
            "Rebuilt missing managed source {} from backup {}",
            record.managed_source.display(),
            backup_path.display()
        ));
        return Ok(());
    }

    Err(anyhow!(
        "Managed source missing at {} and no existing content was available to rebuild it",
        record.managed_source.display()
    ))
}

fn existing_managed_symlink_target(repo_root: &Path, active_path: &Path) -> Option<PathBuf> {
    let metadata = fs::symlink_metadata(active_path).ok()?;
    if !metadata.file_type().is_symlink() {
        return None;
    }
    let target = fs::read_link(active_path).ok()?;
    if target.starts_with(repo_root.join("profiles")) {
        Some(target)
    } else {
        None
    }
}

fn remove_path(path: &Path) -> Result<()> {
    if path.is_dir() && !fs::symlink_metadata(path)?.file_type().is_symlink() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn move_path(source: &Path, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }

    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error)
            if error.raw_os_error() == Some(18) || error.kind() == ErrorKind::CrossesDevices =>
        {
            copy_path_preserving_links(source, destination)?;
            remove_path(source)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

fn copy_path_preserving_links(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(source)?;
        std::os::unix::fs::symlink(target, destination)?;
        return Ok(());
    }

    if metadata.is_dir() {
        fs::create_dir_all(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let child_source = entry.path();
            let child_destination = destination.join(entry.file_name());
            copy_path_preserving_links(&child_source, &child_destination)?;
        }
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    Ok(())
}

fn restore_profile_record(record: &ManagedRecord, result: &mut OperationResult) -> Result<()> {
    let active_is_managed_symlink = fs::symlink_metadata(&record.active_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
        && fs::read_link(&record.active_path)
            .map(|target| target == record.managed_source)
            .unwrap_or(false);

    if active_is_managed_symlink {
        fs::remove_file(&record.active_path)?;
        result
            .filesystem_changes
            .push(format!("Removed symlink {}", record.active_path.display()));

        if let Some(backup_path) = &record.backup_path {
            if backup_path.exists() {
                move_path(backup_path, &record.active_path)?;
                result.filesystem_changes.push(format!(
                    "Restored backup {} -> {}",
                    backup_path.display(),
                    record.active_path.display()
                ));
                return Ok(());
            }
        }

        if record.managed_source.exists() {
            copy_path_preserving_links(&record.managed_source, &record.active_path)?;
            result.filesystem_changes.push(format!(
                "Restored working copy from {}",
                record.managed_source.display()
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AppConfig, DotfileEntry, EntryKind};
    use tempfile::tempdir;

    #[test]
    fn enables_and_disables_file() {
        let temp = tempdir().unwrap();
        let real_home = dirs::home_dir().unwrap();
        let home = real_home.join(".doter-test-home");
        std::fs::create_dir_all(&home).unwrap();

        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(&repo_root).unwrap();
        let paths = AppPaths {
            config_dir: temp.path().join("confdir"),
            data_dir: temp.path().join("datadir"),
            backup_dir: temp.path().join("backups"),
        };
        std::fs::create_dir_all(&paths.backup_dir).unwrap();
        let active = home.join(".bashrc");
        std::fs::write(&active, "export TEST=1").unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };
        let entry = DotfileEntry {
            id: stable_id(OriginScope::Home, &active),
            display_name: ".bashrc".to_string(),
            path: active.clone(),
            origin: OriginScope::Home,
            kind: EntryKind::File,
            managed_state: ManagedState::Unmanaged,
            managed_source: None,
            symlink_target: None,
            backup_path: None,
            warning: None,
        };

        enable_entry(&mut state, &paths, &entry).unwrap();
        assert!(state.managed_entries[0].backup_path.is_none());
        assert!(
            std::fs::symlink_metadata(&active)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        disable_entry(&mut state, &entry).unwrap();
        assert!(active.exists());
        assert!(
            std::fs::symlink_metadata(&active)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
                == false
        );
        assert!(state.managed_entries.is_empty());

        std::fs::remove_file(&active).ok();
        std::fs::remove_dir(&home).ok();
    }

    #[test]
    fn managed_paths_are_profile_scoped() {
        let repo_root = PathBuf::from("/tmp/repo");
        let home_root = PathBuf::from("/tmp/home");
        let xdg_root = home_root.join(".config");
        let active_path = home_root.join(".zshrc");
        let managed = managed_path_for_roots(
            &repo_root,
            "laptop",
            OriginScope::Home,
            &active_path,
            &home_root,
            &xdg_root,
        )
        .unwrap();
        assert_eq!(
            managed,
            PathBuf::from("/tmp/repo/profiles/laptop/home/.zshrc")
        );
    }

    #[test]
    fn enabling_in_new_profile_overwrites_old_profile_symlink() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let paths = AppPaths {
            config_dir: temp.path().join("confdir"),
            data_dir: temp.path().join("datadir"),
            backup_dir: temp.path().join("backups"),
        };
        std::fs::create_dir_all(&paths.backup_dir).unwrap();

        let real_home = dirs::home_dir().unwrap();
        let config_root = real_home.join(".config");
        std::fs::create_dir_all(&config_root).unwrap();
        let active_path = config_root.join("nvim-profile-switch-test");
        if std::fs::symlink_metadata(&active_path).is_ok() {
            remove_path(&active_path).unwrap();
        }

        let old_source = repo_root.join("profiles/default/config/nvim");
        std::fs::create_dir_all(old_source.parent().unwrap()).unwrap();
        std::fs::write(&old_source, "set number").unwrap();
        std::os::unix::fs::symlink(&old_source, &active_path).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["default".to_string(), "laptop".to_string()],
                active_profile: "laptop".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };

        let entry = DotfileEntry {
            id: stable_id(OriginScope::XdgConfig, &active_path),
            display_name: "nvim-profile-switch-test".to_string(),
            path: active_path.clone(),
            origin: OriginScope::XdgConfig,
            kind: EntryKind::Symlink,
            managed_state: ManagedState::Unmanaged,
            managed_source: None,
            symlink_target: Some(old_source.clone()),
            backup_path: None,
            warning: None,
        };

        let result = enable_entry(&mut state, &paths, &entry).unwrap();
        let new_source = repo_root.join("profiles/laptop/config/nvim-profile-switch-test");
        assert!(result.success);
        assert_eq!(std::fs::read_link(&active_path).unwrap(), new_source);
        assert!(new_source.exists());
        assert_eq!(std::fs::read_to_string(&new_source).unwrap(), "set number");
        assert!(state.managed_entries.iter().any(|record| {
            record.profile == "laptop"
                && record.active_path == active_path
                && record.managed_source == new_source
        }));

        std::fs::remove_file(&active_path).ok();
    }

    #[test]
    fn resolves_conflict_by_relinking_current_profile() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let managed_source = repo_root.join("profiles/default/config/nvim");
        let active_path = temp.path().join("active/nvim");
        std::fs::create_dir_all(managed_source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(active_path.parent().unwrap()).unwrap();
        std::fs::write(&managed_source, "set number").unwrap();
        std::fs::write(&active_path, "conflicting content").unwrap();

        let paths = AppPaths {
            config_dir: temp.path().join("confdir"),
            data_dir: temp.path().join("datadir"),
            backup_dir: temp.path().join("backups"),
        };
        std::fs::create_dir_all(&paths.backup_dir).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: "xdg:/tmp/nvim".to_string(),
                profile: "default".to_string(),
                active_path: active_path.clone(),
                managed_source: managed_source.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };
        let entry = DotfileEntry {
            id: "xdg:/tmp/nvim".to_string(),
            display_name: "nvim".to_string(),
            path: active_path.clone(),
            origin: OriginScope::XdgConfig,
            kind: EntryKind::Directory,
            managed_state: ManagedState::Conflicted,
            managed_source: Some(managed_source.clone()),
            symlink_target: None,
            backup_path: None,
            warning: Some("conflict".to_string()),
        };

        let result = resolve_conflict_entry(&mut state, &paths, &entry).unwrap();
        assert!(result.success);
        assert_eq!(std::fs::read_link(&active_path).unwrap(), managed_source);
        assert!(state.managed_entries[0].backup_path.is_some());
    }

    #[test]
    fn resolves_conflict_by_rebuilding_missing_profile_source() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let old_source = repo_root.join("profiles/default/config/nvim");
        let new_source = repo_root.join("profiles/desktop-arch/config/nvim");
        let active_path = temp.path().join("active/nvim");
        std::fs::create_dir_all(old_source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(active_path.parent().unwrap()).unwrap();
        std::fs::write(&old_source, "set number").unwrap();
        std::os::unix::fs::symlink(&old_source, &active_path).unwrap();

        let paths = AppPaths {
            config_dir: temp.path().join("confdir"),
            data_dir: temp.path().join("datadir"),
            backup_dir: temp.path().join("backups"),
        };
        std::fs::create_dir_all(&paths.backup_dir).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["default".to_string(), "desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: "xdg:/tmp/nvim".to_string(),
                profile: "desktop-arch".to_string(),
                active_path: active_path.clone(),
                managed_source: new_source.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };
        let entry = DotfileEntry {
            id: "xdg:/tmp/nvim".to_string(),
            display_name: "nvim".to_string(),
            path: active_path.clone(),
            origin: OriginScope::XdgConfig,
            kind: EntryKind::Symlink,
            managed_state: ManagedState::Conflicted,
            managed_source: Some(new_source.clone()),
            symlink_target: Some(old_source.clone()),
            backup_path: None,
            warning: Some("conflict".to_string()),
        };

        let result = resolve_conflict_entry(&mut state, &paths, &entry).unwrap();
        assert!(result.success);
        assert_eq!(std::fs::read_link(&active_path).unwrap(), new_source);
        assert_eq!(std::fs::read_to_string(&new_source).unwrap(), "set number");
    }

    #[test]
    fn removes_profile_and_restores_working_copy() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let managed_source = repo_root.join("profiles/laptop/config/nvim");
        let active_path = temp.path().join("active/nvim");
        std::fs::create_dir_all(managed_source.parent().unwrap()).unwrap();
        std::fs::create_dir_all(active_path.parent().unwrap()).unwrap();
        std::fs::write(&managed_source, "set number").unwrap();
        std::os::unix::fs::symlink(&managed_source, &active_path).unwrap();

        let paths = AppPaths {
            config_dir: temp.path().join("confdir"),
            data_dir: temp.path().join("datadir"),
            backup_dir: temp.path().join("backups"),
        };
        std::fs::create_dir_all(&paths.backup_dir).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["default".to_string(), "laptop".to_string()],
                active_profile: "laptop".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: "xdg:/tmp/nvim".to_string(),
                profile: "laptop".to_string(),
                active_path: active_path.clone(),
                managed_source: managed_source.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };

        let result = remove_profile(&mut state, &paths, "laptop").unwrap();
        assert!(result.success);
        assert!(active_path.exists());
        assert!(
            std::fs::symlink_metadata(&active_path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
                == false
        );
        assert!(!repo_root.join("profiles/laptop").exists());
        assert_eq!(state.config.active_profile, "default");
        assert!(state.managed_entries.is_empty());
    }
}
