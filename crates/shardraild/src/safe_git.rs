use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum SafeGitError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("unsafe or out-of-bounds gitdir link refused: {0:?}")]
    UnsafeGitDir(PathBuf),
    #[error("invalid repository structure: {0}")]
    InvalidRepo(String),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeGitInspection {
    pub is_git: bool,
    pub head_commit: Option<String>,
    pub head_ref: Option<String>,
    pub diff_hash: String,
    pub tracked_files_count: usize,
    pub untracked_files_count: usize,
    pub warnings: Vec<String>,
}

#[allow(dead_code)]
pub struct SafeGitWorker;

#[allow(dead_code)]
impl SafeGitWorker {
    /// Inspect a workspace root strictly within its boundaries without executing any external program.
    /// Never invokes `git` CLI or any configured hook/filter/driver.
    pub fn inspect(workspace_root: &Path) -> Result<SafeGitInspection, SafeGitError> {
        let canonical_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());

        let mut warnings = Vec::new();
        let dot_git = canonical_root.join(".git");

        if !dot_git.exists() {
            // Non-git workspace: compute pure tree hash
            let (diff_hash, file_count) = Self::compute_tree_hash(&canonical_root, &mut warnings)?;
            return Ok(SafeGitInspection {
                is_git: false,
                head_commit: None,
                head_ref: None,
                diff_hash,
                tracked_files_count: 0,
                untracked_files_count: file_count,
                warnings,
            });
        }

        // Bounded discovery: resolve .git strictly relative to workspace root
        let git_dir = if dot_git.is_file() {
            let content = fs::read_to_string(&dot_git)?;
            let trimmed = content.trim();
            if let Some(target) = trimmed.strip_prefix("gitdir:") {
                let target_path = Path::new(target.trim());
                let resolved = if target_path.is_absolute() {
                    target_path.to_path_buf()
                } else {
                    canonical_root.join(target_path)
                };
                let canonical_target = resolved.canonicalize().unwrap_or(resolved);
                // Flag out-of-workspace gitdir links
                if !canonical_target.starts_with(&canonical_root) {
                    warnings.push(format!(
                        "outbound gitdir link points outside workspace: {:?}",
                        canonical_target
                    ));
                    return Ok(SafeGitInspection {
                        is_git: false,
                        head_commit: None,
                        head_ref: None,
                        diff_hash: "outbound_gitdir_refused".to_string(),
                        tracked_files_count: 0,
                        untracked_files_count: 0,
                        warnings,
                    });
                }
                canonical_target
            } else {
                return Err(SafeGitError::InvalidRepo("malformed .git file".into()));
            }
        } else {
            dot_git
        };

        // Inspect .git/config for dangerous/executable settings without executing anything
        let config_path = git_dir.join("config");
        if config_path.is_file()
            && let Ok(config_text) = fs::read_to_string(&config_path)
        {
            Self::inspect_config_for_threats(&config_text, &mut warnings);
        }

        // Inspect HEAD safely
        let (head_commit, head_ref) = Self::resolve_head(&git_dir, &mut warnings)?;

        // Inspect workspace tree and diff hash
        let (diff_hash, tracked_count) = Self::compute_git_workspace_hash(
            &canonical_root,
            head_commit.as_deref(),
            &mut warnings,
        )?;

        Ok(SafeGitInspection {
            is_git: true,
            head_commit,
            head_ref,
            diff_hash,
            tracked_files_count: tracked_count,
            untracked_files_count: 0,
            warnings,
        })
    }

    fn inspect_config_for_threats(config: &str, warnings: &mut Vec<String>) {
        let mut current_section = String::new();
        for line in config.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();
            if lower.starts_with('[') && lower.ends_with(']') {
                current_section = lower[1..lower.len() - 1].trim().to_string();
                if current_section.starts_with("include") {
                    warnings.push(format!("neutralized execution threat: {}", trimmed));
                }
                continue;
            }

            let full_key = if !current_section.is_empty() {
                format!("{}.{}", current_section, lower)
            } else {
                lower.clone()
            };

            if full_key.contains("fsmonitor") {
                warnings.push(format!("neutralized execution threat: {}", trimmed));
            } else if full_key.contains("diff")
                && (full_key.contains("external") || full_key.contains("textconv"))
            {
                warnings.push(format!(
                    "neutralized execution threat (diff.external): {}",
                    trimmed
                ));
            } else if full_key.contains("hookspath") || full_key.contains("sshcommand") {
                warnings.push(format!("neutralized execution threat: {}", trimmed));
            } else if full_key.contains("filter")
                && (full_key.contains("clean")
                    || full_key.contains("smudge")
                    || full_key.contains("process"))
            {
                warnings.push(format!(
                    "neutralized execution threat (filter.*): {}",
                    trimmed
                ));
            }
        }
    }

    fn resolve_head(
        git_dir: &Path,
        _warnings: &mut Vec<String>,
    ) -> Result<(Option<String>, Option<String>), SafeGitError> {
        let head_path = git_dir.join("HEAD");
        if !head_path.is_file() {
            return Ok((None, None));
        }

        let content = fs::read_to_string(&head_path)?;
        let trimmed = content.trim();

        if let Some(reference) = trimmed.strip_prefix("ref:") {
            let ref_name = reference.trim();
            let ref_path = git_dir.join(ref_name);
            if ref_path.is_file() {
                let commit = fs::read_to_string(&ref_path)?.trim().to_string();
                return Ok((Some(commit), Some(ref_name.to_string())));
            }

            // Check packed-refs if loose ref does not exist
            let packed_path = git_dir.join("packed-refs");
            if packed_path.is_file()
                && let Ok(packed) = fs::read_to_string(&packed_path)
            {
                for line in packed.lines() {
                    let line_trimmed = line.trim();
                    if line_trimmed.ends_with(ref_name)
                        && let Some(sha) = line_trimmed.split_whitespace().next()
                    {
                        return Ok((Some(sha.to_string()), Some(ref_name.to_string())));
                    }
                }
            }

            Ok((None, Some(ref_name.to_string())))
        } else if trimmed.len() == 40 || trimmed.len() == 64 {
            Ok((Some(trimmed.to_string()), None))
        } else {
            Ok((None, None))
        }
    }

    fn compute_tree_hash(
        root: &Path,
        warnings: &mut Vec<String>,
    ) -> Result<(String, usize), SafeGitError> {
        let mut files = BTreeMap::new();
        Self::scan_dir_safe(root, root, &mut files, warnings)?;

        let mut hasher = Sha256::new();
        for (rel_path, (size, mtime)) in &files {
            hasher.update(rel_path.as_bytes());
            hasher.update(size.to_be_bytes());
            hasher.update(mtime.to_be_bytes());
        }
        let digest = format!("{:x}", hasher.finalize());
        Ok((digest, files.len()))
    }

    fn compute_git_workspace_hash(
        root: &Path,
        head_commit: Option<&str>,
        warnings: &mut Vec<String>,
    ) -> Result<(String, usize), SafeGitError> {
        let mut files = BTreeMap::new();
        Self::scan_dir_safe(root, root, &mut files, warnings)?;

        let mut hasher = Sha256::new();
        if let Some(head) = head_commit {
            hasher.update(b"HEAD:");
            hasher.update(head.as_bytes());
        }
        for (rel_path, (size, mtime)) in &files {
            hasher.update(rel_path.as_bytes());
            hasher.update(size.to_be_bytes());
            hasher.update(mtime.to_be_bytes());
        }
        let digest = format!("{:x}", hasher.finalize());
        Ok((digest, files.len()))
    }

    fn scan_dir_safe(
        current: &Path,
        root: &Path,
        files: &mut BTreeMap<String, (u64, u64)>,
        warnings: &mut Vec<String>,
    ) -> Result<(), SafeGitError> {
        let entries = match fs::read_dir(current) {
            Ok(iter) => iter,
            Err(_) => return Ok(()),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            if name_str == ".git" {
                continue;
            }

            let meta = match fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            if meta.file_type().is_symlink() {
                // Check symlink target boundary: do NOT follow links outside workspace
                if let Ok(target) = fs::read_link(&path) {
                    let resolved = if target.is_absolute() {
                        target.clone()
                    } else {
                        current.join(&target)
                    };
                    let canonical_resolved = resolved.canonicalize().unwrap_or(resolved);
                    if !canonical_resolved.starts_with(root) {
                        warnings.push(format!(
                            "symlink points outside workspace: {:?} -> {:?}",
                            path, target
                        ));
                        continue;
                    }
                }
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.is_file() && meta.nlink() > 1 {
                    warnings.push(format!("hardlink count > 1 on file: {:?}", path));
                }
            }

            if meta.is_dir() {
                Self::scan_dir_safe(&path, root, files, warnings)?;
            } else if meta.is_file() || meta.file_type().is_symlink() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                files.insert(rel, (meta.len(), mtime));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_inspect_non_git_workspace() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("main.rs");
        fs::write(&file, b"fn main() {}").unwrap();

        let inspection = SafeGitWorker::inspect(dir.path()).unwrap();
        assert!(!inspection.is_git);
        assert_eq!(inspection.head_commit, None);
        assert_eq!(inspection.untracked_files_count, 1);
        assert!(!inspection.diff_hash.is_empty());
    }

    #[test]
    fn test_inspect_git_workspace_with_head() {
        let dir = tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(git_dir.join("refs/heads")).unwrap();
        fs::write(git_dir.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
        fs::write(
            git_dir.join("refs/heads/main"),
            b"0123456789abcdef0123456789abcdef01234567\n",
        )
        .unwrap();

        let src_file = dir.path().join("file.txt");
        fs::write(&src_file, b"hello").unwrap();

        let inspection = SafeGitWorker::inspect(dir.path()).unwrap();
        assert!(inspection.is_git);
        assert_eq!(
            inspection.head_commit.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(inspection.head_ref.as_deref(), Some("refs/heads/main"));
        assert_eq!(inspection.tracked_files_count, 1);
        assert!(!inspection.diff_hash.is_empty());
    }

    #[test]
    fn test_neutralizes_malicious_configs() {
        let dir = tempdir().unwrap();
        let git_dir = dir.path().join(".git");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("HEAD"), b"ref: refs/heads/main\n").unwrap();

        let malicious_config = r#"
[core]
    fsmonitor = /bin/sh -c "echo pwned > /tmp/pwned"
    hooksPath = /tmp/evil-hooks
    sshCommand = /tmp/evil-ssh
[diff]
    external = /bin/sh -c "echo evil-diff"
[filter "lfs"]
    clean = git-lfs clean -- %f
    smudge = git-lfs smudge -- %f
[include]
    path = /etc/passwd
"#;
        fs::write(git_dir.join("config"), malicious_config).unwrap();

        let inspection = SafeGitWorker::inspect(dir.path()).unwrap();
        assert!(inspection.is_git);
        assert!(inspection.warnings.iter().any(|w| w.contains("fsmonitor")));
        assert!(
            inspection
                .warnings
                .iter()
                .any(|w| w.contains("diff.external"))
        );
        assert!(inspection.warnings.iter().any(|w| w.contains("hooksPath")));
        assert!(inspection.warnings.iter().any(|w| w.contains("sshCommand")));
        assert!(inspection.warnings.iter().any(|w| w.contains("filter.")));
        assert!(inspection.warnings.iter().any(|w| w.contains("[include")));
    }

    #[test]
    fn test_outbound_gitdir_refused() {
        let dir = tempdir().unwrap();
        let git_file = dir.path().join(".git");
        fs::write(&git_file, b"gitdir: /tmp/outside_repo_gitdir\n").unwrap();

        let inspection = SafeGitWorker::inspect(dir.path()).unwrap();
        assert!(!inspection.is_git);
        assert_eq!(inspection.diff_hash, "outbound_gitdir_refused");
        assert!(
            inspection
                .warnings
                .iter()
                .any(|w| w.contains("outbound gitdir link"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_escape_flagged_and_not_followed() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let target = outside_dir.path().join("secret.txt");
        fs::write(&target, b"secret").unwrap();

        let link = dir.path().join("link_to_outside");
        symlink(&target, &link).unwrap();

        let inspection = SafeGitWorker::inspect(dir.path()).unwrap();
        assert!(
            inspection
                .warnings
                .iter()
                .any(|w| w.contains("symlink points outside workspace"))
        );
    }
}
