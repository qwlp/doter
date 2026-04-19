use crate::model::{DotfileEntry, ManagedRecord, ManagedState, OperationResult, OriginScope};
use crate::scanner::stable_id;
use crate::state::{AppPaths, PersistedState, SharedLinkEntry, portable_entry_key};
use anyhow::{Result, anyhow};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileCopyMode {
    KeepExisting,
    OverwriteExisting,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileCopyPreview {
    pub managed_entries: usize,
    pub conflict_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileApplyPreview {
    pub inactive_entries: usize,
    pub existing_paths_to_replace: usize,
    pub missing_paths_to_create: usize,
}

#[derive(Debug, Clone, Default)]
pub struct EntryProfileSyncPreview {
    pub destination_profiles: usize,
    pub conflict_profiles: Vec<String>,
    pub conflict_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct EntryProfileCopyPreview {
    pub source_path: PathBuf,
    pub destination_path: PathBuf,
    pub conflict_paths: Vec<PathBuf>,
    pub destination_active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SharedEntryPreview {
    pub shared_path: PathBuf,
    pub target_profiles: Vec<String>,
    pub conflict_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedMigrationCandidate {
    pub display_name: String,
    pub origin: OriginScope,
    pub key: String,
    pub profiles: Vec<String>,
    pub source_path: PathBuf,
    pub shared_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
pub struct SharedMigrationPreview {
    pub candidates: Vec<SharedMigrationCandidate>,
    pub divergent_entries: Vec<String>,
    pub already_shared_entries: Vec<String>,
}

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

pub fn preview_profile_copy(
    state: &PersistedState,
    source_profile: &str,
    destination_profile: &str,
) -> Result<ProfileCopyPreview> {
    if source_profile == destination_profile {
        return Err(anyhow!("Choose two different profiles"));
    }
    if !state
        .config
        .profiles
        .iter()
        .any(|profile| profile == source_profile)
    {
        return Err(anyhow!("Profile {source_profile} does not exist"));
    }
    if !state
        .config
        .profiles
        .iter()
        .any(|profile| profile == destination_profile)
    {
        return Err(anyhow!("Profile {destination_profile} does not exist"));
    }

    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before copying profiles"))?;
    let source_root = repo_root.join("profiles").join(source_profile);
    let destination_root = repo_root.join("profiles").join(destination_profile);

    let managed_entries = state
        .managed_entries
        .iter()
        .filter(|record| record.profile == source_profile)
        .count();
    let mut conflict_paths = BTreeSet::new();
    collect_profile_conflicts(&source_root, &destination_root, &mut conflict_paths)?;

    Ok(ProfileCopyPreview {
        managed_entries,
        conflict_paths: conflict_paths.into_iter().collect(),
    })
}

pub fn copy_profile(
    state: &mut PersistedState,
    source_profile: &str,
    destination_profile: &str,
    mode: ProfileCopyMode,
) -> Result<OperationResult> {
    let preview = preview_profile_copy(state, source_profile, destination_profile)?;
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before copying profiles"))?;
    let source_root = repo_root.join("profiles").join(source_profile);
    let destination_root = repo_root.join("profiles").join(destination_profile);

    if !source_root.exists() && preview.managed_entries == 0 {
        return Err(anyhow!("Profile {source_profile} has no managed dotfiles to copy"));
    }

    let mut result = OperationResult {
        success: true,
        message: format!("Copied profile {source_profile} to {destination_profile}"),
        filesystem_changes: Vec::new(),
        git_changes: vec![format!(
            "Updated profiles/{destination_profile} from profiles/{source_profile}"
        )],
    };

    let mut copied_paths = 0usize;
    let mut skipped_paths = 0usize;
    if source_root.exists() {
        let stats =
            copy_profile_tree(&source_root, &destination_root, &destination_root, mode, &mut result)?;
        copied_paths = stats.copied_paths;
        skipped_paths = stats.skipped_paths;
    }

    let existing_backups = state
        .managed_entries
        .iter()
        .filter(|record| record.profile == destination_profile)
        .map(|record| (record.id.clone(), record.backup_path.clone()))
        .collect::<HashMap<_, _>>();

    for record in state
        .managed_entries
        .clone()
        .into_iter()
        .filter(|record| record.profile == source_profile)
    {
        let target_source = managed_path_for(
            &repo_root,
            destination_profile,
            record.origin,
            &record.active_path,
        )?;
        state.upsert_record(ManagedRecord {
            id: stable_id(record.origin, &record.active_path),
            profile: destination_profile.to_string(),
            active_path: record.active_path.clone(),
            managed_source: target_source,
            backup_path: existing_backups
                .get(&record.id)
                .cloned()
                .unwrap_or(None),
            origin: record.origin,
        });
    }

    if !state
        .config
        .profiles
        .iter()
        .any(|profile| profile == destination_profile)
    {
        state.config.profiles.push(destination_profile.to_string());
        state.config.ensure_active_profile();
    }

    result.message = match mode {
        ProfileCopyMode::KeepExisting if skipped_paths > 0 => format!(
            "Copied profile {source_profile} to {destination_profile}; kept {skipped_paths} existing destination path(s)"
        ),
        ProfileCopyMode::OverwriteExisting if !preview.conflict_paths.is_empty() => format!(
            "Copied profile {source_profile} to {destination_profile}; overwrote {} existing destination path(s)",
            preview.conflict_paths.len()
        ),
        _ => format!(
            "Copied profile {source_profile} to {destination_profile}; {} path(s) updated",
            copied_paths
        ),
    };

    Ok(result)
}

pub fn preview_apply_entries(entries: &[DotfileEntry]) -> ProfileApplyPreview {
    let mut preview = ProfileApplyPreview::default();
    for entry in entries {
        if entry.managed_state != ManagedState::ManagedInactive || entry.managed_source.is_none() {
            continue;
        }
        preview.inactive_entries += 1;
        if entry.path.exists() {
            preview.existing_paths_to_replace += 1;
        } else {
            preview.missing_paths_to_create += 1;
        }
    }
    preview
}

pub fn apply_entries(
    state: &mut PersistedState,
    paths: &AppPaths,
    entries: &[DotfileEntry],
) -> Result<OperationResult> {
    let preview = preview_apply_entries(entries);
    if preview.inactive_entries == 0 {
        return Err(anyhow!("No inactive managed entries are available to apply"));
    }

    let active_profile = state.active_profile().to_string();
    let mut result = OperationResult {
        success: true,
        message: String::new(),
        filesystem_changes: Vec::new(),
        git_changes: Vec::new(),
    };

    for entry in entries {
        if entry.managed_state != ManagedState::ManagedInactive || entry.managed_source.is_none() {
            continue;
        }
        let step = enable_entry(state, paths, entry)?;
        result.filesystem_changes.extend(step.filesystem_changes);
        result.git_changes.extend(step.git_changes);
    }

    result.message = format!(
        "Applied profile {} on this device; activated {} entr{}, backed up {} existing path(s), created {} missing path(s)",
        active_profile,
        preview.inactive_entries,
        if preview.inactive_entries == 1 { "y" } else { "ies" },
        preview.existing_paths_to_replace,
        preview.missing_paths_to_create
    );

    Ok(result)
}

pub fn preview_entry_profile_sync(
    state: &PersistedState,
    entry: &DotfileEntry,
    destination_profiles: &[String],
) -> Result<EntryProfileSyncPreview> {
    validate_conflict(entry)?;
    let source = entry
        .managed_source
        .clone()
        .ok_or_else(|| anyhow!("This dotfile is not managed in the active profile yet"))?;
    if !source.exists() {
        return Err(anyhow!(
            "Managed source is missing at {}",
            source.display()
        ));
    }

    let active_profile = state.active_profile().to_string();
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before syncing dotfiles between profiles"))?;
    let mut conflict_paths = BTreeSet::new();
    let mut conflict_profiles = Vec::new();
    let mut unique_profiles = BTreeSet::new();

    for profile in destination_profiles {
        if profile == &active_profile {
            continue;
        }
        if !state.config.profiles.iter().any(|existing| existing == profile) {
            return Err(anyhow!("Profile {profile} does not exist"));
        }
        if !unique_profiles.insert(profile.clone()) {
            continue;
        }

        let destination = managed_path_for(&repo_root, profile, entry.origin, &entry.path)?;
        let mut destination_conflicts = BTreeSet::new();
        collect_profile_conflicts(&source, &destination, &mut destination_conflicts)?;
        if !destination_conflicts.is_empty() {
            conflict_profiles.push(profile.clone());
            conflict_paths.extend(destination_conflicts);
        }
    }

    Ok(EntryProfileSyncPreview {
        destination_profiles: unique_profiles.len(),
        conflict_profiles,
        conflict_paths: conflict_paths.into_iter().collect(),
    })
}

pub fn sync_entry_to_profiles(
    state: &mut PersistedState,
    entry: &DotfileEntry,
    destination_profiles: &[String],
    mode: ProfileCopyMode,
) -> Result<OperationResult> {
    let preview = preview_entry_profile_sync(state, entry, destination_profiles)?;
    if preview.destination_profiles == 0 {
        return Err(anyhow!("Choose at least one destination profile"));
    }

    let source = entry
        .managed_source
        .clone()
        .ok_or_else(|| anyhow!("This dotfile is not managed in the active profile yet"))?;
    let active_profile = state.active_profile().to_string();
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before syncing dotfiles between profiles"))?;
    let mut unique_profiles = BTreeSet::new();
    let mut copied_profiles = 0usize;
    let mut copied_paths = 0usize;
    let mut skipped_paths = 0usize;

    let mut result = OperationResult {
        success: true,
        message: String::new(),
        filesystem_changes: Vec::new(),
        git_changes: Vec::new(),
    };

    for profile in destination_profiles {
        if profile == &active_profile || !unique_profiles.insert(profile.clone()) {
            continue;
        }
        let destination = managed_path_for(&repo_root, profile, entry.origin, &entry.path)?;
        let stats = copy_profile_tree(&source, &destination, &destination, mode, &mut result)?;
        copied_profiles += 1;
        copied_paths += stats.copied_paths;
        skipped_paths += stats.skipped_paths;
        result.git_changes.push(format!(
            "Updated {} in profile {}",
            destination
                .strip_prefix(repo_root.join("profiles").join(profile))
                .unwrap_or(&destination)
                .display(),
            profile
        ));
    }

    result.message = match mode {
        ProfileCopyMode::KeepExisting if skipped_paths > 0 => format!(
            "Synced {} from '{}' into {} profile(s); kept {} existing destination path(s)",
            entry.display_name,
            active_profile,
            copied_profiles,
            skipped_paths
        ),
        ProfileCopyMode::OverwriteExisting if !preview.conflict_paths.is_empty() => format!(
            "Synced {} from '{}' into {} profile(s); overwrote {} existing destination path(s)",
            entry.display_name,
            active_profile,
            copied_profiles,
            preview.conflict_paths.len()
        ),
        _ => format!(
            "Synced {} from '{}' into {} profile(s); {} path(s) updated",
            entry.display_name,
            active_profile,
            copied_profiles,
            copied_paths
        ),
    };

    Ok(result)
}

pub fn preview_copy_entry_from_profile(
    state: &PersistedState,
    entry: &DotfileEntry,
    source_profile: &str,
    destination_profile: &str,
) -> Result<EntryProfileCopyPreview> {
    validate_conflict(entry)?;
    if source_profile == destination_profile {
        return Err(anyhow!("Choose a different source profile"));
    }
    if !state
        .config
        .profiles
        .iter()
        .any(|profile| profile == source_profile)
    {
        return Err(anyhow!("Profile {source_profile} does not exist"));
    }
    if !state
        .config
        .profiles
        .iter()
        .any(|profile| profile == destination_profile)
    {
        return Err(anyhow!("Profile {destination_profile} does not exist"));
    }

    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before copying dotfiles between profiles"))?;
    let source_path = managed_path_for(&repo_root, source_profile, entry.origin, &entry.path)?;
    if !source_path.exists() {
        return Err(anyhow!(
            "'{}' is not managed in profile '{}'",
            entry.display_name,
            source_profile
        ));
    }

    let destination_path =
        managed_path_for(&repo_root, destination_profile, entry.origin, &entry.path)?;
    let mut conflict_paths = BTreeSet::new();
    collect_profile_conflicts(&source_path, &destination_path, &mut conflict_paths)?;
    let destination_active = state.active_profile() == destination_profile
        && fs::symlink_metadata(&entry.path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
        && fs::read_link(&entry.path)
            .map(|target| target == destination_path)
            .unwrap_or(false);

    Ok(EntryProfileCopyPreview {
        source_path,
        destination_path,
        conflict_paths: conflict_paths.into_iter().collect(),
        destination_active,
    })
}

pub fn copy_entry_from_profile(
    state: &mut PersistedState,
    entry: &DotfileEntry,
    source_profile: &str,
    destination_profile: &str,
    mode: ProfileCopyMode,
) -> Result<OperationResult> {
    let preview =
        preview_copy_entry_from_profile(state, entry, source_profile, destination_profile)?;
    let mut result = OperationResult {
        success: true,
        message: String::new(),
        filesystem_changes: Vec::new(),
        git_changes: vec![format!(
            "Updated {} from profile {}",
            preview.destination_path.display(),
            source_profile
        )],
    };

    let stats = copy_profile_tree(
        &preview.source_path,
        &preview.destination_path,
        &preview.destination_path,
        mode,
        &mut result,
    )?;

    result.message = match mode {
        ProfileCopyMode::KeepExisting if stats.skipped_paths > 0 => format!(
            "Copied {} from '{}' into '{}'; kept the existing destination copy",
            entry.display_name,
            source_profile,
            destination_profile
        ),
        ProfileCopyMode::OverwriteExisting if !preview.conflict_paths.is_empty() => format!(
            "Copied {} from '{}' into '{}'; overwrote the current profile copy",
            entry.display_name,
            source_profile,
            destination_profile
        ),
        _ => format!(
            "Copied {} from '{}' into '{}'",
            entry.display_name,
            source_profile,
            destination_profile
        ),
    };
    if preview.destination_active && matches!(mode, ProfileCopyMode::OverwriteExisting) {
        result.message.push_str("; the active live symlink now uses the new repo copy");
    }

    Ok(result)
}

pub fn preview_share_entry(
    state: &PersistedState,
    entry: &DotfileEntry,
    target_profiles: &[String],
) -> Result<SharedEntryPreview> {
    validate_conflict(entry)?;
    let source = entry
        .managed_source
        .clone()
        .ok_or_else(|| anyhow!("This dotfile is not managed in the active profile yet"))?;
    if !source.exists() {
        return Err(anyhow!("Managed source is missing at {}", source.display()));
    }

    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before sharing dotfiles"))?;
    let shared_path = shared_managed_path(&repo_root, entry.origin, &entry.path)?;
    let mut profiles = BTreeSet::new();
    profiles.insert(state.active_profile().to_string());
    for profile in target_profiles {
        if !state.config.profiles.iter().any(|existing| existing == profile) {
            return Err(anyhow!("Profile {profile} does not exist"));
        }
        profiles.insert(profile.clone());
    }

    let mut conflict_paths = BTreeSet::new();
    if shared_path.exists() {
        collect_profile_conflicts(&source, &shared_path, &mut conflict_paths)?;
    }
    Ok(SharedEntryPreview {
        shared_path,
        target_profiles: profiles.into_iter().collect(),
        conflict_paths: conflict_paths.into_iter().collect(),
    })
}

pub fn share_entry_with_profiles(
    state: &mut PersistedState,
    entry: &DotfileEntry,
    target_profiles: &[String],
    mode: ProfileCopyMode,
) -> Result<OperationResult> {
    let preview = preview_share_entry(state, entry, target_profiles)?;
    let source = entry
        .managed_source
        .clone()
        .ok_or_else(|| anyhow!("This dotfile is not managed in the active profile yet"))?;
    let mut result = OperationResult {
        success: true,
        message: String::new(),
        filesystem_changes: Vec::new(),
        git_changes: vec![format!(
            "Updated shared link for {}",
            entry.display_name
        )],
    };

    if preview.shared_path.exists() {
        let _ = copy_profile_tree(&source, &preview.shared_path, &preview.shared_path, mode, &mut result)?;
    } else {
        if let Some(parent) = preview.shared_path.parent() {
            fs::create_dir_all(parent)?;
        }
        copy_path_preserving_links(&source, &preview.shared_path)?;
        result.filesystem_changes.push(format!(
            "Copied {} into shared layer",
            entry.display_name
        ));
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow!("Unable to resolve home directory"))?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let key = portable_entry_key(entry.origin, &entry.path, &home, &xdg)?;
    let mut links = state.load_shared_links()?;
    if let Some(existing) = links
        .entries
        .iter_mut()
        .find(|existing| existing.origin == entry.origin && existing.key == key)
    {
        existing.profiles = preview.target_profiles.clone();
    } else {
        links.entries.push(SharedLinkEntry {
            origin: entry.origin,
            key,
            profiles: preview.target_profiles.clone(),
        });
    }
    links.entries.sort_by(|left, right| {
        (left.origin as u8)
            .cmp(&(right.origin as u8))
            .then(left.key.cmp(&right.key))
    });
    state.save_shared_links(&links)?;

    let active_profile = state.active_profile().to_string();
    if preview
        .target_profiles
        .iter()
        .any(|profile| profile == &active_profile)
    {
        if let Some(existing) = state
            .managed_entries
            .iter_mut()
            .find(|existing| existing.profile == active_profile && existing.id == entry.id)
        {
            existing.managed_source = preview.shared_path.clone();
        }
        let active_is_current_managed_symlink = fs::symlink_metadata(&entry.path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false)
            && fs::read_link(&entry.path)
                .map(|target| target == source)
                .unwrap_or(false);
        if active_is_current_managed_symlink {
            fs::remove_file(&entry.path)?;
            std::os::unix::fs::symlink(&preview.shared_path, &entry.path)?;
            result.filesystem_changes.push(format!(
                "Re-linked {} to shared source {}",
                entry.path.display(),
                preview.shared_path.display()
            ));
        }
    }

    result.message = format!(
        "Shared {} across {} profile(s)",
        entry.display_name,
        preview.target_profiles.len()
    );
    Ok(result)
}

pub fn preview_shared_migration(state: &PersistedState) -> Result<SharedMigrationPreview> {
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before migrating shared dotfiles"))?;
    let links = state.load_shared_links()?;
    let already_shared = links
        .entries
        .into_iter()
        .map(|entry| (entry.origin as u8, entry.key))
        .collect::<BTreeSet<_>>();

    let mut grouped =
        BTreeMap::<(u8, String), (OriginScope, Vec<(String, PathBuf)>)>::new();
    for profile in &state.config.profiles {
        for (origin, scope_dir) in [
            (OriginScope::Home, "home"),
            (OriginScope::XdgConfig, "config"),
        ] {
            let root = repo_root.join("profiles").join(profile).join(scope_dir);
            if !root.exists() {
                continue;
            }
            for child in fs::read_dir(&root)? {
                let child = child?;
                let key = child.file_name().to_string_lossy().to_string();
                grouped
                    .entry((origin as u8, key.clone()))
                    .or_insert_with(|| (origin, Vec::new()))
                    .1
                    .push((profile.clone(), child.path()));
            }
        }
    }

    let mut preview = SharedMigrationPreview::default();
    for ((_, key), (origin, paths)) in grouped {
        if paths.len() < 2 {
            continue;
        }
        let label = format!(
            "{} {}",
            match origin {
                OriginScope::Home => "home",
                OriginScope::XdgConfig => "config",
                OriginScope::Custom => "custom",
            },
            key
        );
        if already_shared.contains(&(origin as u8, key.clone())) {
            preview.already_shared_entries.push(label);
            continue;
        }
        let baseline = content_signature(&paths[0].1)?;
        if paths
            .iter()
            .skip(1)
            .all(|(_, path)| content_signature(path).map(|sig| sig == baseline).unwrap_or(false))
        {
            preview.candidates.push(SharedMigrationCandidate {
                display_name: key.clone(),
                origin,
                key: key.clone(),
                profiles: paths.iter().map(|(profile, _)| profile.clone()).collect(),
                source_path: paths[0].1.clone(),
                shared_path: shared_path_for_key(&repo_root, origin, &key),
            });
        } else {
            preview.divergent_entries.push(label);
        }
    }

    preview.candidates.sort_by(|left, right| {
        (left.origin as u8)
            .cmp(&(right.origin as u8))
            .then(left.key.cmp(&right.key))
    });
    preview.divergent_entries.sort();
    preview.already_shared_entries.sort();
    Ok(preview)
}

pub fn migrate_entries_to_shared(
    state: &mut PersistedState,
    candidates: &[SharedMigrationCandidate],
) -> Result<OperationResult> {
    let repo_root = state
        .config
        .repo_root
        .clone()
        .ok_or_else(|| anyhow!("Configure a repository before migrating shared dotfiles"))?;
    let active_profile = state.active_profile().to_string();
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Unable to resolve home directory"))?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let mut links = state.load_shared_links()?;
    let mut result = OperationResult {
        success: true,
        message: String::new(),
        filesystem_changes: Vec::new(),
        git_changes: vec!["Updated shared links manifest".to_string()],
    };

    for candidate in candidates {
        if candidate.shared_path.exists() {
            let shared_sig = content_signature(&candidate.shared_path)?;
            let source_sig = content_signature(&candidate.source_path)?;
            if shared_sig != source_sig {
                return Err(anyhow!(
                    "Shared path already exists with different contents: {}",
                    candidate.shared_path.display()
                ));
            }
        } else {
            if let Some(parent) = candidate.shared_path.parent() {
                fs::create_dir_all(parent)?;
            }
            copy_path_preserving_links(&candidate.source_path, &candidate.shared_path)?;
            result.filesystem_changes.push(format!(
                "Copied {} into shared layer",
                candidate.display_name
            ));
        }

        if let Some(existing) = links
            .entries
            .iter_mut()
            .find(|existing| existing.origin == candidate.origin && existing.key == candidate.key)
        {
            existing.profiles = candidate.profiles.clone();
        } else {
            links.entries.push(SharedLinkEntry {
                origin: candidate.origin,
                key: candidate.key.clone(),
                profiles: candidate.profiles.clone(),
            });
        }

        if candidate.profiles.iter().any(|profile| profile == &active_profile) {
            let live_path = live_path_for_key(candidate.origin, &candidate.key, &home, &xdg);
            if let Some(record) = state
                .managed_entries
                .iter_mut()
                .find(|record| record.profile == active_profile && record.active_path == live_path)
            {
                record.managed_source = candidate.shared_path.clone();
            }
            let active_profile_path =
                managed_path_for(&repo_root, &active_profile, candidate.origin, &live_path)?;
            let active_is_current_managed_symlink = fs::symlink_metadata(&live_path)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false)
                && fs::read_link(&live_path)
                    .map(|target| target == active_profile_path)
                    .unwrap_or(false);
            if active_is_current_managed_symlink {
                fs::remove_file(&live_path)?;
                std::os::unix::fs::symlink(&candidate.shared_path, &live_path)?;
                result.filesystem_changes.push(format!(
                    "Re-linked {} to shared source {}",
                    live_path.display(),
                    candidate.shared_path.display()
                ));
            }
        }
    }

    links.entries.sort_by(|left, right| {
        (left.origin as u8)
            .cmp(&(right.origin as u8))
            .then(left.key.cmp(&right.key))
    });
    state.save_shared_links(&links)?;
    result.message = format!(
        "Migrated {} dotfile{} into the shared layer",
        candidates.len(),
        if candidates.len() == 1 { "" } else { "s" }
    );
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

pub fn shared_managed_path(
    repo_root: &Path,
    origin: OriginScope,
    active_path: &Path,
) -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("Unable to resolve home directory"))?;
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let root = match origin {
        OriginScope::Home => repo_root.join("shared").join("home"),
        OriginScope::XdgConfig => repo_root.join("shared").join("config"),
        OriginScope::Custom => repo_root.join("shared").join("custom"),
    };
    let relative = match origin {
        OriginScope::Home => active_path.strip_prefix(&home)?.to_path_buf(),
        OriginScope::XdgConfig => active_path.strip_prefix(&xdg)?.to_path_buf(),
        OriginScope::Custom => custom_relative_path(active_path, &home, &xdg),
    };
    Ok(root.join(relative))
}

fn managed_path_for_roots(
    repo_root: &Path,
    profile: &str,
    origin: OriginScope,
    active_path: &Path,
    home_root: &Path,
    xdg_root: &Path,
) -> Result<PathBuf> {
    let root = match origin {
        OriginScope::Home => repo_root.join("profiles").join(profile).join("home"),
        OriginScope::XdgConfig => repo_root.join("profiles").join(profile).join("config"),
        OriginScope::Custom => repo_root.join("profiles").join(profile).join("custom"),
    };
    let relative = match origin {
        OriginScope::Home => active_path.strip_prefix(home_root)?.to_path_buf(),
        OriginScope::XdgConfig => active_path.strip_prefix(xdg_root)?.to_path_buf(),
        OriginScope::Custom => custom_relative_path(active_path, home_root, xdg_root),
    };
    Ok(root.join(relative))
}

fn backup_path_for(paths: &AppPaths, origin: OriginScope, active_path: &Path) -> PathBuf {
    let label = match origin {
        OriginScope::Home => "home",
        OriginScope::XdgConfig => "config",
        OriginScope::Custom => "custom",
    };
    let safe_name = active_path
        .to_string_lossy()
        .replace('/', "__")
        .replace(':', "_");
    paths.backup_dir.join(format!("{label}_{safe_name}"))
}

fn custom_relative_path(active_path: &Path, home_root: &Path, xdg_root: &Path) -> PathBuf {
    if let Ok(relative) = active_path.strip_prefix(xdg_root) {
        return PathBuf::from("config").join(relative);
    }
    if let Ok(relative) = active_path.strip_prefix(home_root) {
        return PathBuf::from("home").join(relative);
    }
    if !active_path.is_absolute() {
        return PathBuf::from("relative").join(active_path);
    }

    let mut relative = PathBuf::from("absolute");
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

fn collect_profile_conflicts(
    source: &Path,
    destination: &Path,
    conflicts: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }

    let source_meta = fs::symlink_metadata(source)?;
    let destination_meta = fs::symlink_metadata(destination).ok();
    if destination_meta.is_some() {
        conflicts.insert(destination.to_path_buf());
    }

    if source_meta.is_dir() && !source_meta.file_type().is_symlink() {
        if destination_meta
            .as_ref()
            .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
            .unwrap_or(false)
        {
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                collect_profile_conflicts(
                    &entry.path(),
                    &destination.join(entry.file_name()),
                    conflicts,
                )?;
            }
        }
    }

    Ok(())
}

fn content_signature(path: &Path) -> Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)?;
        return Ok(format!("symlink:{}", target.to_string_lossy()));
    }
    if metadata.is_file() {
        let bytes = fs::read(path)?;
        return Ok(format!("file:{:016x}", hash_value(&bytes)));
    }
    if metadata.is_dir() {
        let mut rows = Vec::new();
        collect_directory_signature_rows(path, path, &mut rows)?;
        return Ok(format!("dir:{:016x}", hash_value(&rows.join("\n"))));
    }
    Ok(format!("unknown:{}", path.display()))
}

fn collect_directory_signature_rows(
    root: &Path,
    current: &Path,
    rows: &mut Vec<String>,
) -> Result<()> {
    let mut children = fs::read_dir(current)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let child_path = child.path();
        let relative = child_path
            .strip_prefix(root)
            .unwrap_or(&child_path)
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = fs::symlink_metadata(&child_path)?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&child_path)?;
            rows.push(format!("symlink:{relative}:{}", target.to_string_lossy()));
        } else if metadata.is_dir() {
            rows.push(format!("dir:{relative}"));
            collect_directory_signature_rows(root, &child_path, rows)?;
        } else if metadata.is_file() {
            rows.push(format!(
                "file:{relative}:{:016x}",
                hash_value(&fs::read(&child_path)?)
            ));
        }
    }
    Ok(())
}

fn hash_value<T: Hash>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn shared_path_for_key(repo_root: &Path, origin: OriginScope, key: &str) -> PathBuf {
    let base = match origin {
        OriginScope::Home => repo_root.join("shared").join("home"),
        OriginScope::XdgConfig => repo_root.join("shared").join("config"),
        OriginScope::Custom => repo_root.join("shared").join("custom"),
    };
    base.join(key)
}

fn live_path_for_key(origin: OriginScope, key: &str, home: &Path, xdg: &Path) -> PathBuf {
    match origin {
        OriginScope::Home => home.join(key),
        OriginScope::XdgConfig => xdg.join(key),
        OriginScope::Custom => home.join(key),
    }
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

#[derive(Default)]
struct CopyStats {
    copied_paths: usize,
    skipped_paths: usize,
}

fn copy_profile_tree(
    source: &Path,
    destination: &Path,
    destination_root: &Path,
    mode: ProfileCopyMode,
    result: &mut OperationResult,
) -> Result<CopyStats> {
    let source_meta = fs::symlink_metadata(source)?;

    if source_meta.is_dir() && !source_meta.file_type().is_symlink() {
        let destination_meta = fs::symlink_metadata(destination).ok();
        if let Some(meta) = destination_meta {
            if !meta.is_dir() || meta.file_type().is_symlink() {
                match mode {
                    ProfileCopyMode::KeepExisting => {
                        result.filesystem_changes.push(format!(
                            "Kept existing {}",
                            destination.display()
                        ));
                        return Ok(CopyStats {
                            copied_paths: 0,
                            skipped_paths: 1,
                        });
                    }
                    ProfileCopyMode::OverwriteExisting => {
                        remove_path(destination)?;
                        result.filesystem_changes.push(format!(
                            "Overwrote {}",
                            destination.display()
                        ));
                    }
                }
            }
        }

        fs::create_dir_all(destination)?;
        let mut stats = CopyStats::default();
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let child_stats = copy_profile_tree(
                &entry.path(),
                &destination.join(entry.file_name()),
                destination_root,
                mode,
                result,
            )?;
            stats.copied_paths += child_stats.copied_paths;
            stats.skipped_paths += child_stats.skipped_paths;
        }
        return Ok(stats);
    }

    if destination.exists() {
        match mode {
            ProfileCopyMode::KeepExisting => {
                result
                    .filesystem_changes
                    .push(format!("Kept existing {}", destination.display()));
                return Ok(CopyStats {
                    copied_paths: 0,
                    skipped_paths: 1,
                });
            }
            ProfileCopyMode::OverwriteExisting => {
                remove_path(destination)?;
                result
                    .filesystem_changes
                    .push(format!("Overwrote {}", destination.display()));
            }
        }
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    copy_path_preserving_links(source, destination)?;
    let label = destination
        .strip_prefix(destination_root)
        .unwrap_or(destination)
        .display()
        .to_string();
    result
        .filesystem_changes
        .push(format!("Copied {label} into destination profile"));
    Ok(CopyStats {
        copied_paths: 1,
        skipped_paths: 0,
    })
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
            shared_profiles: Vec::new(),
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
            shared_profiles: Vec::new(),
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
            shared_profiles: Vec::new(),
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
            shared_profiles: Vec::new(),
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

    #[test]
    fn previews_copy_conflicts_for_existing_destination_paths() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = repo_root.join("profiles/default/config/nvim");
        let destination_root = repo_root.join("profiles/laptop/config/nvim");
        std::fs::create_dir_all(source_root.join("lua")).unwrap();
        std::fs::create_dir_all(&destination_root).unwrap();
        std::fs::write(source_root.join("init.lua"), "source").unwrap();
        std::fs::write(source_root.join("lua/plugins.lua"), "plugins").unwrap();
        std::fs::write(destination_root.join("init.lua"), "dest").unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["default".to_string(), "laptop".to_string()],
                active_profile: "default".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![],
        };

        let preview = preview_profile_copy(&state, "default", "laptop").unwrap();
        assert_eq!(preview.conflict_paths.len(), 4);
        assert!(preview
            .conflict_paths
            .iter()
            .any(|path| path.ends_with("profiles/laptop/config")));
        assert!(preview
            .conflict_paths
            .iter()
            .any(|path| path.ends_with("profiles/laptop/config/nvim")));
        assert!(preview
            .conflict_paths
            .iter()
            .any(|path| path.ends_with("profiles/laptop/config/nvim/init.lua")));
    }

    #[test]
    fn copies_profile_and_keeps_existing_destination_files() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let source_root = repo_root.join("profiles/default/config/nvim");
        let destination_root = repo_root.join("profiles/laptop/config/nvim");
        std::fs::create_dir_all(source_root.join("lua")).unwrap();
        std::fs::create_dir_all(&destination_root).unwrap();
        std::fs::write(source_root.join("init.lua"), "source init").unwrap();
        std::fs::write(source_root.join("lua/plugins.lua"), "plugins").unwrap();
        std::fs::write(destination_root.join("init.lua"), "dest init").unwrap();

        let active_path = dirs::home_dir()
            .unwrap()
            .join(".config/nvim");
        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["default".to_string(), "laptop".to_string()],
                active_profile: "default".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: "xdg:/tmp/nvim".to_string(),
                profile: "default".to_string(),
                active_path: active_path.clone(),
                managed_source: source_root.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };

        let result = copy_profile(
            &mut state,
            "default",
            "laptop",
            ProfileCopyMode::KeepExisting,
        )
        .unwrap();

        assert!(result.success);
        assert_eq!(
            std::fs::read_to_string(destination_root.join("init.lua")).unwrap(),
            "dest init"
        );
        assert_eq!(
            std::fs::read_to_string(destination_root.join("lua/plugins.lua")).unwrap(),
            "plugins"
        );
        assert!(state.managed_entries.iter().any(|record| {
            record.profile == "laptop"
                && record.active_path == active_path
                && record.managed_source == destination_root
        }));
    }

    #[test]
    fn custom_paths_are_stored_under_profile_custom_root() {
        let repo_root = PathBuf::from("/tmp/repo");
        let home_root = PathBuf::from("/tmp/home");
        let xdg_root = home_root.join(".config");
        let active_path = PathBuf::from("/opt/tools/nvim/init.lua");
        let managed = managed_path_for_roots(
            &repo_root,
            "desktop-arch",
            OriginScope::Custom,
            &active_path,
            &home_root,
            &xdg_root,
        )
        .unwrap();
        assert_eq!(
            managed,
            PathBuf::from(
                "/tmp/repo/profiles/desktop-arch/custom/absolute/opt/tools/nvim/init.lua"
            )
        );
    }

    #[test]
    fn home_custom_paths_are_stored_without_username_segment() {
        let repo_root = PathBuf::from("/tmp/repo");
        let home_root = PathBuf::from("/tmp/home");
        let xdg_root = home_root.join(".config");
        let active_path = home_root.join(".local/share/foobar/config.json");
        let managed = managed_path_for_roots(
            &repo_root,
            "desktop-arch",
            OriginScope::Custom,
            &active_path,
            &home_root,
            &xdg_root,
        )
        .unwrap();
        assert_eq!(
            managed,
            PathBuf::from(
                "/tmp/repo/profiles/desktop-arch/custom/home/.local/share/foobar/config.json"
            )
        );
    }

    #[test]
    fn xdg_custom_paths_are_stored_under_custom_config_root() {
        let repo_root = PathBuf::from("/tmp/repo");
        let home_root = PathBuf::from("/tmp/home");
        let xdg_root = home_root.join(".config");
        let active_path = xdg_root.join("waybar/style.css");
        let managed = managed_path_for_roots(
            &repo_root,
            "desktop-arch",
            OriginScope::Custom,
            &active_path,
            &home_root,
            &xdg_root,
        )
        .unwrap();
        assert_eq!(
            managed,
            PathBuf::from("/tmp/repo/profiles/desktop-arch/custom/config/waybar/style.css")
        );
    }

    #[test]
    fn previews_inactive_profile_apply_counts() {
        let temp = tempdir().unwrap();
        let existing_path = temp.path().join("existing");
        std::fs::write(&existing_path, "local").unwrap();
        let missing_path = temp.path().join("missing");
        let entries = vec![
            DotfileEntry {
                id: "one".to_string(),
                display_name: "existing".to_string(),
                path: existing_path,
                origin: OriginScope::Home,
                kind: crate::model::EntryKind::File,
                managed_state: ManagedState::ManagedInactive,
                managed_source: Some(temp.path().join("repo/profiles/default/home/.one")),
                symlink_target: None,
                backup_path: None,
                warning: None,
                shared_profiles: Vec::new(),
            },
            DotfileEntry {
                id: "two".to_string(),
                display_name: "missing".to_string(),
                path: missing_path,
                origin: OriginScope::XdgConfig,
                kind: crate::model::EntryKind::Directory,
                managed_state: ManagedState::ManagedInactive,
                managed_source: Some(temp.path().join("repo/profiles/default/config/two")),
                symlink_target: None,
                backup_path: None,
                warning: None,
                shared_profiles: Vec::new(),
            },
        ];

        let preview = preview_apply_entries(&entries);
        assert_eq!(preview.inactive_entries, 2);
        assert_eq!(preview.existing_paths_to_replace, 1);
        assert_eq!(preview.missing_paths_to_create, 1);
    }

    #[test]
    fn applies_inactive_profile_entries() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let active_path = config_root.join("nvim");
        let managed_source = repo_root.join("profiles/desktop-arch/config/nvim");
        std::fs::create_dir_all(managed_source.parent().unwrap()).unwrap();
        std::fs::write(&managed_source, "repo config").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();
        std::fs::write(&active_path, "local config").unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                onboarding_complete: true,
                profiles: vec!["desktop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            backup_dir: temp.path().join("backups"),
        };
        std::fs::create_dir_all(&paths.config_dir).unwrap();
        std::fs::create_dir_all(&paths.data_dir).unwrap();
        std::fs::create_dir_all(&paths.backup_dir).unwrap();
        let entry = DotfileEntry {
            id: stable_id(OriginScope::XdgConfig, &active_path),
            display_name: "nvim".to_string(),
            path: active_path.clone(),
            origin: OriginScope::XdgConfig,
            kind: crate::model::EntryKind::File,
            managed_state: ManagedState::ManagedInactive,
            managed_source: Some(managed_source.clone()),
            symlink_target: None,
            backup_path: None,
            warning: None,
            shared_profiles: Vec::new(),
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let result = apply_entries(&mut state, &paths, &[entry]).unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        assert!(result.message.contains("Applied profile desktop-arch"));
        assert_eq!(state.managed_entries.len(), 1);
        assert!(std::fs::symlink_metadata(&active_path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&active_path).unwrap(), managed_source);
        let backup_path = state.managed_entries[0].backup_path.clone().unwrap();
        assert_eq!(std::fs::read_to_string(&backup_path).unwrap(), "local config");
    }

    #[test]
    fn previews_entry_profile_sync_conflicts() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let source = repo_root.join("profiles/desktop-arch/config/waybar");
        let destination = repo_root.join("profiles/laptop/config/waybar");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("config.jsonc"), "{}").unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("config.jsonc"), "{\"old\":true}").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string(), "laptop".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };
        let entry = DotfileEntry {
            id: "xdg:/tmp/waybar".to_string(),
            display_name: "waybar".to_string(),
            path: config_root.join("waybar"),
            origin: OriginScope::XdgConfig,
            kind: crate::model::EntryKind::Directory,
            managed_state: ManagedState::ManagedActive,
            managed_source: Some(source),
            symlink_target: None,
            backup_path: None,
            warning: None,
            shared_profiles: Vec::new(),
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let preview =
            preview_entry_profile_sync(&state, &entry, &["laptop".to_string()]).unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        assert_eq!(preview.destination_profiles, 1);
        assert_eq!(preview.conflict_profiles, vec!["laptop".to_string()]);
        assert!(preview
            .conflict_paths
            .iter()
            .any(|path| path.ends_with("profiles/laptop/config/waybar/config.jsonc")));
    }

    #[test]
    fn syncs_entry_to_multiple_profiles_and_keeps_existing_paths() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let source = repo_root.join("profiles/desktop-arch/config/waybar");
        let laptop_destination = repo_root.join("profiles/laptop/config/waybar");
        let work_destination = repo_root.join("profiles/work/config/waybar");
        std::fs::create_dir_all(source.join("scripts")).unwrap();
        std::fs::write(source.join("style.css"), "shared css").unwrap();
        std::fs::write(source.join("scripts/module.sh"), "echo shared").unwrap();
        std::fs::create_dir_all(&laptop_destination).unwrap();
        std::fs::write(laptop_destination.join("style.css"), "laptop css").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec![
                    "desktop-arch".to_string(),
                    "laptop".to_string(),
                    "work".to_string(),
                ],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };
        let entry = DotfileEntry {
            id: "xdg:/tmp/waybar".to_string(),
            display_name: "waybar".to_string(),
            path: config_root.join("waybar"),
            origin: OriginScope::XdgConfig,
            kind: crate::model::EntryKind::Directory,
            managed_state: ManagedState::ManagedActive,
            managed_source: Some(source),
            symlink_target: None,
            backup_path: None,
            warning: None,
            shared_profiles: Vec::new(),
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let result = sync_entry_to_profiles(
            &mut state,
            &entry,
            &["laptop".to_string(), "work".to_string()],
            ProfileCopyMode::KeepExisting,
        )
        .unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        assert!(result.message.contains("Synced waybar"));
        assert_eq!(
            std::fs::read_to_string(laptop_destination.join("style.css")).unwrap(),
            "laptop css"
        );
        assert_eq!(
            std::fs::read_to_string(work_destination.join("style.css")).unwrap(),
            "shared css"
        );
        assert_eq!(
            std::fs::read_to_string(work_destination.join("scripts/module.sh")).unwrap(),
            "echo shared"
        );
    }

    #[test]
    fn previews_copy_entry_from_profile_and_detects_active_destination() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let active_path = config_root.join("waybar");
        let source_path = repo_root.join("profiles/laptop/config/waybar");
        let destination_path = repo_root.join("profiles/desktop-arch/config/waybar");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(destination_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, "laptop").unwrap();
        std::fs::write(&destination_path, "desktop").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();
        std::os::unix::fs::symlink(&destination_path, &active_path).unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["desktop-arch".to_string(), "laptop".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };
        let entry = DotfileEntry {
            id: "xdg:/tmp/waybar".to_string(),
            display_name: "waybar".to_string(),
            path: active_path,
            origin: OriginScope::XdgConfig,
            kind: crate::model::EntryKind::File,
            managed_state: ManagedState::ManagedActive,
            managed_source: Some(destination_path.clone()),
            symlink_target: Some(destination_path.clone()),
            backup_path: None,
            warning: None,
            shared_profiles: Vec::new(),
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let preview =
            preview_copy_entry_from_profile(&state, &entry, "laptop", "desktop-arch").unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        assert_eq!(preview.source_path, source_path);
        assert_eq!(preview.destination_path, destination_path);
        assert!(preview.destination_active);
        assert!(!preview.conflict_paths.is_empty());
    }

    #[test]
    fn copies_entry_from_profile_and_overwrites_destination() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let source_path = repo_root.join("profiles/laptop/config/waybar");
        let destination_path = repo_root.join("profiles/desktop-arch/config/waybar");
        std::fs::create_dir_all(source_path.join("scripts")).unwrap();
        std::fs::write(source_path.join("style.css"), "shared css").unwrap();
        std::fs::write(source_path.join("scripts/module.sh"), "echo laptop").unwrap();
        std::fs::create_dir_all(destination_path.join("scripts")).unwrap();
        std::fs::write(destination_path.join("style.css"), "old css").unwrap();
        std::fs::write(destination_path.join("scripts/module.sh"), "echo old").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string(), "laptop".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };
        let entry = DotfileEntry {
            id: "xdg:/tmp/waybar".to_string(),
            display_name: "waybar".to_string(),
            path: config_root.join("waybar"),
            origin: OriginScope::XdgConfig,
            kind: crate::model::EntryKind::Directory,
            managed_state: ManagedState::ManagedInactive,
            managed_source: Some(destination_path.clone()),
            symlink_target: None,
            backup_path: None,
            warning: None,
            shared_profiles: Vec::new(),
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let result = copy_entry_from_profile(
            &mut state,
            &entry,
            "laptop",
            "desktop-arch",
            ProfileCopyMode::OverwriteExisting,
        )
        .unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        assert!(result.message.contains("Copied waybar from 'laptop'"));
        assert_eq!(
            std::fs::read_to_string(destination_path.join("style.css")).unwrap(),
            "shared css"
        );
        assert_eq!(
            std::fs::read_to_string(destination_path.join("scripts/module.sh")).unwrap(),
            "echo laptop"
        );
    }

    #[test]
    fn previews_identical_entries_for_shared_migration() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        std::fs::create_dir_all(repo_root.join("profiles/desktop-arch/config/nvim")).unwrap();
        std::fs::create_dir_all(repo_root.join("profiles/laptop-arch/config/nvim")).unwrap();
        std::fs::write(
            repo_root.join("profiles/desktop-arch/config/nvim/init.lua"),
            "set number",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("profiles/laptop-arch/config/nvim/init.lua"),
            "set number",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("profiles/desktop-arch/config/tmux"),
            "desktop config",
        )
        .unwrap();
        std::fs::write(
            repo_root.join("profiles/laptop-arch/config/tmux"),
            "laptop config",
        )
        .unwrap();

        let state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root),
                profiles: vec!["desktop-arch".to_string(), "laptop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: Vec::new(),
        };

        let preview = preview_shared_migration(&state).unwrap();
        assert_eq!(preview.candidates.len(), 1);
        assert_eq!(preview.candidates[0].origin, OriginScope::XdgConfig);
        assert_eq!(preview.candidates[0].key, "nvim");
        assert_eq!(
            preview.candidates[0].profiles,
            vec!["desktop-arch".to_string(), "laptop-arch".to_string()]
        );
        assert_eq!(preview.divergent_entries, vec!["config tmux".to_string()]);
    }

    #[test]
    fn migrates_entries_to_shared_and_relinks_active_paths() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let active_path = config_root.join("nvim");
        let desktop_path = repo_root.join("profiles/desktop-arch/config/nvim");
        let laptop_path = repo_root.join("profiles/laptop-arch/config/nvim");
        std::fs::create_dir_all(&desktop_path).unwrap();
        std::fs::create_dir_all(&laptop_path).unwrap();
        std::fs::write(desktop_path.join("init.lua"), "shared").unwrap();
        std::fs::write(laptop_path.join("init.lua"), "shared").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();
        std::os::unix::fs::symlink(&desktop_path, &active_path).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["desktop-arch".to_string(), "laptop-arch".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: stable_id(OriginScope::XdgConfig, &active_path),
                profile: "desktop-arch".to_string(),
                active_path: active_path.clone(),
                managed_source: desktop_path.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let preview = preview_shared_migration(&state).unwrap();
        let result = migrate_entries_to_shared(&mut state, &preview.candidates).unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        let shared_path = repo_root.join("shared/config/nvim");
        assert!(result.message.contains("Migrated 1 dotfile"));
        assert_eq!(std::fs::read_link(&active_path).unwrap(), shared_path);
        assert_eq!(state.managed_entries[0].managed_source, shared_path);
        let links = state.load_shared_links().unwrap();
        assert!(links.entries.iter().any(|entry| {
            entry.origin == OriginScope::XdgConfig
                && entry.key == "nvim"
                && entry.profiles == vec!["desktop-arch".to_string(), "laptop-arch".to_string()]
        }));
    }

    #[test]
    fn shares_entry_with_profiles_and_relinks_active_path() {
        let temp = tempdir().unwrap();
        let repo_root = temp.path().join("repo");
        let config_root = temp.path().join("config");
        let active_path = config_root.join("waybar");
        let source_path = repo_root.join("profiles/desktop-arch/config/waybar");
        std::fs::create_dir_all(source_path.parent().unwrap()).unwrap();
        std::fs::write(&source_path, "desktop").unwrap();
        std::fs::create_dir_all(&config_root).unwrap();
        std::os::unix::fs::symlink(&source_path, &active_path).unwrap();

        let mut state = PersistedState {
            config: AppConfig {
                repo_root: Some(repo_root.clone()),
                profiles: vec!["desktop-arch".to_string(), "laptop".to_string()],
                active_profile: "desktop-arch".to_string(),
                ..AppConfig::default()
            },
            managed_entries: vec![ManagedRecord {
                id: stable_id(OriginScope::XdgConfig, &active_path),
                profile: "desktop-arch".to_string(),
                active_path: active_path.clone(),
                managed_source: source_path.clone(),
                backup_path: None,
                origin: OriginScope::XdgConfig,
            }],
        };
        let entry = DotfileEntry {
            id: stable_id(OriginScope::XdgConfig, &active_path),
            display_name: "waybar".to_string(),
            path: active_path.clone(),
            origin: OriginScope::XdgConfig,
            kind: crate::model::EntryKind::File,
            managed_state: ManagedState::ManagedActive,
            managed_source: Some(source_path),
            symlink_target: None,
            backup_path: None,
            warning: None,
            shared_profiles: Vec::new(),
        };

        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &config_root);
        }
        let result = share_entry_with_profiles(
            &mut state,
            &entry,
            &["desktop-arch".to_string(), "laptop".to_string()],
            ProfileCopyMode::OverwriteExisting,
        )
        .unwrap();
        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }

        let shared_path = repo_root.join("shared/config/waybar");
        assert!(result.message.contains("Shared waybar across 2 profile(s)"));
        assert_eq!(std::fs::read_link(&active_path).unwrap(), shared_path);
        assert_eq!(state.managed_entries[0].managed_source, shared_path);
        let links = state.load_shared_links().unwrap();
        assert!(links.entries.iter().any(|entry| {
            entry.origin == OriginScope::XdgConfig
                && entry.key == "waybar"
                && entry.profiles == vec!["desktop-arch".to_string(), "laptop".to_string()]
        }));
    }
}
