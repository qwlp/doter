use crate::model::GitRepoState;
use anyhow::{Context, Result, anyhow};
use git2::{
    DiffFormat, DiffOptions, IndexAddOption, Repository, RepositoryInitOptions, Signature, Status,
    StatusOptions,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct SyncOutcome {
    pub fetched: bool,
    pub pulled: bool,
    pub pushed: bool,
}

pub fn detect_repo(path: &Path) -> Result<Option<PathBuf>> {
    match Repository::discover(path) {
        Ok(repo) => Ok(repo.workdir().map(Path::to_path_buf)),
        Err(_) => Ok(None),
    }
}

pub fn init_repo(path: &Path) -> Result<PathBuf> {
    fs::create_dir_all(path)?;
    let mut options = RepositoryInitOptions::new();
    options.initial_head("main");
    let repo = Repository::init_opts(path, &options)?;
    repo.workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("Repository has no workdir"))
}

pub fn clone_repo(url: &str, path: &Path) -> Result<PathBuf> {
    if url.trim().is_empty() {
        return Err(anyhow!("Repository URL is required"));
    }
    if path.exists() {
        let mut entries = fs::read_dir(path)?;
        if entries.next().is_some() {
            return Err(anyhow!(
                "Destination {} is not empty",
                path.display()
            ));
        }
    } else {
        fs::create_dir_all(path)?;
    }

    let repo = Repository::clone(url, path)?;
    repo.workdir()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("Repository has no workdir"))
}

pub fn repo_status(repo_root: &Path) -> Result<GitRepoState> {
    let repo = Repository::open(repo_root)?;
    let remotes = repo.remotes()?;
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .include_ignored(false)
        .recurse_untracked_dirs(true);
    let statuses = repo.statuses(Some(&mut options))?;

    let mut state = GitRepoState {
        repo_root: Some(repo_root.to_path_buf()),
        current_branch: repo
            .head()
            .ok()
            .and_then(|head| head.shorthand().map(|name| name.to_string())),
        remotes: remotes.iter().flatten().map(ToString::to_string).collect(),
        remote_details: remotes
            .iter()
            .flatten()
            .filter_map(|name| {
                repo.find_remote(name)
                    .ok()
                    .and_then(|remote| remote.url().map(|url| format!("{} -> {}", name, url)))
            })
            .collect(),
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        untracked_files: Vec::new(),
    };

    for entry in statuses.iter() {
        let Some(path) = entry.path() else {
            continue;
        };
        let path = path.to_string();
        let status = entry.status();
        if status.contains(Status::WT_NEW) {
            state.untracked_files.push(path.clone());
        }
        if status.intersects(
            Status::WT_MODIFIED | Status::WT_DELETED | Status::WT_RENAMED | Status::WT_TYPECHANGE,
        ) {
            state.unstaged_files.push(path.clone());
        }
        if status.intersects(
            Status::INDEX_NEW
                | Status::INDEX_MODIFIED
                | Status::INDEX_DELETED
                | Status::INDEX_RENAMED
                | Status::INDEX_TYPECHANGE,
        ) {
            state.staged_files.push(path);
        }
    }

    state.staged_files.sort();
    state.unstaged_files.sort();
    state.untracked_files.sort();
    Ok(state)
}

pub fn stage_paths(repo_root: &Path, paths: &[PathBuf]) -> Result<()> {
    let repo = Repository::open(repo_root)?;
    let workdir = repo.workdir().context("Repository has no workdir")?;
    let mut index = repo.index()?;
    let relatives = paths
        .iter()
        .map(|path| path.strip_prefix(workdir).unwrap_or(path).to_path_buf())
        .collect::<Vec<_>>();
    index.add_all(relatives.iter(), IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

pub fn commit_staged(repo_root: &Path, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        return Err(anyhow!("Commit message cannot be empty"));
    }

    let repo = Repository::open(repo_root)?;
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Doter", "doter@example.invalid"))
        .context("Unable to build git signature")?;

    let parent_commit = repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .and_then(|oid| repo.find_commit(oid).ok());

    let parents = parent_commit.iter().collect::<Vec<_>>();
    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        message,
        &tree,
        &parents,
    )?;
    Ok(())
}

pub fn set_remote(repo_root: &Path, name: &str, url: &str) -> Result<()> {
    if name.trim().is_empty() || url.trim().is_empty() {
        return Err(anyhow!("Remote name and URL are required"));
    }

    let repo = Repository::open(repo_root)?;
    match repo.find_remote(name) {
        Ok(_) => {
            repo.remote_set_url(name, url)?;
        }
        Err(_) => {
            repo.remote(name, url)?;
        }
    }
    Ok(())
}

pub fn update_remote(repo_root: &Path, previous_name: &str, name: &str, url: &str) -> Result<()> {
    if name.trim().is_empty() || url.trim().is_empty() {
        return Err(anyhow!("Remote name and URL are required"));
    }

    let repo = Repository::open(repo_root)?;
    let previous_exists = repo.find_remote(previous_name).is_ok();
    let target_exists = repo.find_remote(name).is_ok();

    if previous_name == name {
        return set_remote(repo_root, name, url);
    }

    if previous_exists {
        if target_exists {
            repo.remote_set_url(name, url)?;
            repo.remote_delete(previous_name)?;
        } else {
            let _problems = repo.remote_rename(previous_name, name)?;
            repo.remote_set_url(name, url)?;
        }
    } else {
        set_remote(repo_root, name, url)?;
    }

    Ok(())
}

pub fn remote_url(repo_root: &Path, name: &str) -> Result<Option<String>> {
    let repo = Repository::open(repo_root)?;
    let remote = match repo.find_remote(name) {
        Ok(remote) => remote,
        Err(_) => return Ok(None),
    };
    Ok(remote.url().map(ToString::to_string))
}

pub fn push_current_branch(repo_root: &Path, remote_name: &str, branch_name: &str) -> Result<()> {
    if remote_name.trim().is_empty() {
        return Err(anyhow!("Remote name is required"));
    }
    if branch_name.trim().is_empty() {
        return Err(anyhow!("Branch name is required"));
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("push")
        .arg("--set-upstream")
        .arg(remote_name)
        .arg(branch_name)
        .output()
        .context("Failed to start git push")?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let message = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "git push failed".to_string()
        };
        Err(anyhow!(message))
    }
}

fn git_command(repo_root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(args)
        .output()
        .with_context(|| format!("Failed to start git {}", args.join(" ")))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("git {} failed", args.join(" "))
    };
    Err(anyhow!(message))
}

pub fn sync_with_remote(repo_root: &Path, remote_name: &str) -> Result<SyncOutcome> {
    if remote_name.trim().is_empty() {
        return Err(anyhow!("Remote name is required"));
    }

    let repo = Repository::open(repo_root)?;
    if repo.find_remote(remote_name).is_err() {
        return Ok(SyncOutcome::default());
    }

    git_command(repo_root, &["fetch", remote_name, "--prune"])?;

    let repo = Repository::open(repo_root)?;
    let branch_name = repo
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(|name| name.to_string()))
        .unwrap_or_else(|| "main".to_string());
    let remote_branch = format!("{remote_name}/{branch_name}");
    let remote_ref = format!("refs/remotes/{remote_branch}");
    let local_has_commit = repo.head().ok().and_then(|head| head.target()).is_some();
    let remote_has_branch = repo.find_reference(&remote_ref).is_ok();

    let mut outcome = SyncOutcome {
        fetched: true,
        ..SyncOutcome::default()
    };

    if remote_has_branch {
        if local_has_commit {
            git_command(
                repo_root,
                &["pull", "--rebase", "--autostash", remote_name, &branch_name],
            )?;
        } else {
            git_command(
                repo_root,
                &["checkout", "-B", &branch_name, "--track", &remote_branch],
            )?;
        }
        outcome.pulled = true;
    }

    let repo = Repository::open(repo_root)?;
    if repo.head().ok().and_then(|head| head.target()).is_some() {
        git_command(
            repo_root,
            &["push", "--set-upstream", remote_name, &branch_name],
        )?;
        outcome.pushed = true;
    }

    Ok(outcome)
}

pub fn diff_for_path(repo_root: &Path, path: Option<&Path>) -> Result<String> {
    let repo = Repository::open(repo_root)?;
    let mut options = DiffOptions::new();
    if let Some(path) = path {
        let workdir = repo.workdir().context("Repository has no workdir")?;
        let relative = path.strip_prefix(workdir).unwrap_or(path);
        options.pathspec(relative);
    }

    let diff = repo.diff_index_to_workdir(None, Some(&mut options))?;
    let mut output = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        if let Ok(text) = std::str::from_utf8(line.content()) {
            output.push_str(text);
        }
        true
    })?;

    if output.is_empty() {
        output.push_str(
            "No unstaged diff for selection.
",
        );
    }
    Ok(output)
}

pub fn tracked_file_text(repo_root: &Path, path: &Path) -> Result<Option<String>> {
    let repo = Repository::open(repo_root)?;
    let Some(head_target) = repo.head().ok().and_then(|head| head.target()) else {
        return Ok(None);
    };
    let tree = repo.find_commit(head_target)?.tree()?;
    let workdir = repo.workdir().context("Repository has no workdir")?;
    let relative = path.strip_prefix(workdir).unwrap_or(path);

    let entry = match tree.get_path(relative) {
        Ok(entry) => entry,
        Err(_) => return Ok(None),
    };

    let object = entry.to_object(&repo)?;
    let blob = object.peel_to_blob()?;
    let text = std::str::from_utf8(blob.content())
        .context("Tracked file is not valid UTF-8")?
        .to_string();
    Ok(Some(text))
}

pub fn stage_all(repo_root: &Path) -> Result<()> {
    let repo = Repository::open(repo_root)?;
    let mut index = repo.index()?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None)?;
    index.write()?;
    Ok(())
}

pub fn unstage_paths(repo_root: &Path, paths: &[PathBuf]) -> Result<()> {
    let repo = Repository::open(repo_root)?;
    let workdir = repo.workdir().context("Repository has no workdir")?;
    let mut index = repo.index()?;
    let relatives = paths
        .iter()
        .map(|path| path.strip_prefix(workdir).unwrap_or(path).to_path_buf())
        .collect::<Vec<_>>();
    for relative in relatives {
        index.remove(&relative, 0)?;
    }
    index.write()?;
    Ok(())
}

pub fn remove_from_index_and_delete(repo_root: &Path, path: &Path) -> Result<()> {
    let repo = Repository::open(repo_root)?;
    let workdir = repo.workdir().context("Repository has no workdir")?;
    let relative = path.strip_prefix(workdir).unwrap_or(path);
    let mut index = repo.index()?;
    index.remove(relative, 0)?;
    index.write()?;
    if path.exists() {
        if path.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

pub fn fetch_and_merge_remote(repo_root: &Path, remote_name: &str) -> Result<()> {
    sync_with_remote(repo_root, remote_name).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn creates_repo_and_tracks_remote() {
        let temp = tempdir().unwrap();
        let repo_root = init_repo(temp.path()).unwrap();
        set_remote(&repo_root, "origin", "https://example.com/test.git").unwrap();
        let state = repo_status(&repo_root).unwrap();
        assert_eq!(state.remotes, vec!["origin".to_string()]);
        assert_eq!(
            remote_url(&repo_root, "origin").unwrap(),
            Some("https://example.com/test.git".to_string())
        );
    }

    #[test]
    fn accepts_ssh_remote_urls() {
        let temp = tempdir().unwrap();
        let repo_root = init_repo(temp.path()).unwrap();
        set_remote(&repo_root, "origin", "git@github.com:user/repo.git").unwrap();
        assert_eq!(
            remote_url(&repo_root, "origin").unwrap(),
            Some("git@github.com:user/repo.git".to_string())
        );
    }

    #[test]
    fn updates_existing_remote_name_and_url() {
        let temp = tempdir().unwrap();
        let repo_root = init_repo(temp.path()).unwrap();
        set_remote(&repo_root, "origin", "https://example.com/old.git").unwrap();

        update_remote(
            &repo_root,
            "origin",
            "upstream",
            "git@github.com:user/repo.git",
        )
        .unwrap();

        let state = repo_status(&repo_root).unwrap();
        assert_eq!(state.remotes, vec!["upstream".to_string()]);
        assert_eq!(
            remote_url(&repo_root, "upstream").unwrap(),
            Some("git@github.com:user/repo.git".to_string())
        );
        assert_eq!(remote_url(&repo_root, "origin").unwrap(), None);
    }

    #[test]
    fn stages_and_commits_changes() {
        let temp = tempdir().unwrap();
        let repo_root = init_repo(temp.path()).unwrap();
        let file = repo_root.join("file.txt");
        fs::write(&file, "hello").unwrap();
        stage_paths(&repo_root, &[file]).unwrap();
        commit_staged(&repo_root, "Initial commit").unwrap();

        let state = repo_status(&repo_root).unwrap();
        assert!(!state.is_dirty());
        assert_eq!(state.current_branch.as_deref(), Some("main"));
    }

    #[test]
    fn stages_directory_paths() {
        let temp = tempdir().unwrap();
        let repo_root = init_repo(temp.path()).unwrap();
        let directory = repo_root.join("profiles/default/config/nvim");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("init.lua"), "vim.opt.number = true").unwrap();

        stage_paths(&repo_root, &[directory]).unwrap();

        let state = repo_status(&repo_root).unwrap();
        assert!(
            state
                .staged_files
                .iter()
                .any(|path| path.ends_with("init.lua"))
        );
    }

    #[test]
    fn pushes_current_branch_to_remote() {
        let temp = tempdir().unwrap();
        let remote_root = temp.path().join("remote.git");
        Repository::init_bare(&remote_root).unwrap();

        let repo_root = temp.path().join("repo");
        let repo_root = init_repo(&repo_root).unwrap();
        set_remote(&repo_root, "origin", remote_root.to_str().unwrap()).unwrap();

        let file = repo_root.join("file.txt");
        fs::write(&file, "hello").unwrap();
        stage_paths(&repo_root, &[file]).unwrap();
        commit_staged(&repo_root, "Initial commit").unwrap();
        push_current_branch(&repo_root, "origin", "main").unwrap();

        let remote_repo = Repository::open_bare(&remote_root).unwrap();
        let head = remote_repo.find_reference("refs/heads/main").unwrap();
        assert!(head.target().is_some());
    }

    #[test]
    fn sync_with_remote_pulls_existing_remote_history() {
        let temp = tempdir().unwrap();
        let remote_root = temp.path().join("remote.git");
        Repository::init_bare(&remote_root).unwrap();

        let seed_root = temp.path().join("seed");
        let seed_root = init_repo(&seed_root).unwrap();
        set_remote(&seed_root, "origin", remote_root.to_str().unwrap()).unwrap();
        let seed_file = seed_root.join("seed.txt");
        fs::write(&seed_file, "hello").unwrap();
        stage_paths(&seed_root, &[seed_file]).unwrap();
        commit_staged(&seed_root, "Seed remote").unwrap();
        push_current_branch(&seed_root, "origin", "main").unwrap();

        let repo_root = temp.path().join("repo");
        let repo_root = init_repo(&repo_root).unwrap();
        set_remote(&repo_root, "origin", remote_root.to_str().unwrap()).unwrap();

        let outcome = sync_with_remote(&repo_root, "origin").unwrap();

        assert!(outcome.fetched);
        assert!(outcome.pulled);
        assert!(repo_root.join("seed.txt").exists());
    }

    #[test]
    fn sync_with_remote_pushes_local_commits() {
        let temp = tempdir().unwrap();
        let remote_root = temp.path().join("remote.git");
        Repository::init_bare(&remote_root).unwrap();

        let repo_root = temp.path().join("repo");
        let repo_root = init_repo(&repo_root).unwrap();
        set_remote(&repo_root, "origin", remote_root.to_str().unwrap()).unwrap();

        let file = repo_root.join("local.txt");
        fs::write(&file, "local").unwrap();
        stage_paths(&repo_root, &[file]).unwrap();
        commit_staged(&repo_root, "Local commit").unwrap();

        let outcome = sync_with_remote(&repo_root, "origin").unwrap();
        let remote_repo = Repository::open_bare(&remote_root).unwrap();

        assert!(outcome.fetched);
        assert!(outcome.pushed);
        assert!(remote_repo.find_reference("refs/heads/main").is_ok());
    }
}
