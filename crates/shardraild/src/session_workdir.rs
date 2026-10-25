//! Startup allowlist for session working directories.
//!
//! Paths are canonicalized before they are stored. A request must name one of
//! those canonical paths exactly: relative paths, `.` / `..`, and any symlink
//! component are rejected before the canonical comparison.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const SHARDRAIL_PREFIX: &str = "/Users/daliys/Daliys/Shardrail";
const SHARDRAIL_GIT: &str = "/Users/daliys/Daliys/Shardrail/.git";
const S1_TESTREPO: &str = "/Users/daliys/Daliys/Shardrail/spikes/s1-testrepo";

pub(crate) fn collect_session_workdirs(
    codex_workdir: Option<&Path>,
    session_workdirs: &[PathBuf],
) -> io::Result<Vec<PathBuf>> {
    let mut allowlist = Vec::new();
    if let Some(path) = codex_workdir {
        push_unique(&mut allowlist, validate_session_workdir(path)?);
    }
    for path in session_workdirs {
        push_unique(&mut allowlist, validate_session_workdir(path)?);
    }
    Ok(allowlist)
}

pub(crate) fn format_allowlist(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "(empty)".to_owned()
    } else {
        paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(crate) fn validate_session_workdir(path: &Path) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "session workdir {} cannot be resolved: {error}",
                path.display()
            ),
        )
    })?;
    if !canonical.is_dir() {
        return Err(workdir_error(&canonical, "is not a directory"));
    }
    let shardrail_worktree = git_common_is_shardrail(&canonical)?;
    if let Some(message) = rejection_message(&canonical, shardrail_worktree) {
        return Err(workdir_error(&canonical, message));
    }
    Ok(canonical)
}

/// Accept a client-supplied workdir only when it is exactly an allowlisted path.
pub(crate) fn accept_requested_workdir(
    raw: &str,
    allowlist: &[PathBuf],
) -> Result<PathBuf, &'static str> {
    if raw.is_empty() || raw.len() > 4096 || raw.chars().any(char::is_control) {
        return Err("workdir is invalid");
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err("workdir must be an absolute allowlisted path");
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("workdir must not contain . or ..");
    }
    let mut walked = PathBuf::new();
    for component in path.components() {
        walked.push(component);
        match fs::symlink_metadata(&walked) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("workdir must not contain a symlink");
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(_) => return Err("workdir is not allowlisted"),
        }
    }
    let canonical = fs::canonicalize(path).map_err(|_| "workdir is not allowlisted")?;
    if canonical.is_dir() && allowlist.iter().any(|allowed| allowed == &canonical) {
        Ok(canonical)
    } else {
        Err("workdir is not allowlisted")
    }
}

fn push_unique(allowlist: &mut Vec<PathBuf>, path: PathBuf) {
    if !allowlist.iter().any(|existing| existing == &path) {
        allowlist.push(path);
    }
}

fn workdir_error(path: &Path, detail: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("session workdir {} {detail}", path.display()),
    )
}

fn rejection_message(canonical: &Path, shardrail_worktree: bool) -> Option<&'static str> {
    if canonical == Path::new("/") {
        return Some("refusing the filesystem root");
    }
    if is_home(canonical) {
        return Some("refusing $HOME");
    }
    if shardrail_prefix_rejected(canonical) {
        return Some("refusing a path inside the Shardrail repository");
    }
    if shardrail_worktree {
        return Some("refusing a Shardrail worktree");
    }
    None
}

fn is_home(canonical: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let home = PathBuf::from(home);
    canonical == home || fs::canonicalize(&home).is_ok_and(|real| canonical == real)
}

fn shardrail_prefix_rejected(canonical: &Path) -> bool {
    if s1_testrepo_exception(canonical) {
        return false;
    }
    let prefix = Path::new(SHARDRAIL_PREFIX);
    canonical == prefix || canonical.starts_with(prefix)
}

fn s1_testrepo_exception(canonical: &Path) -> bool {
    let exception = Path::new(S1_TESTREPO);
    if canonical == exception || canonical.starts_with(exception) {
        return true;
    }
    fs::canonicalize(exception).is_ok_and(|real| canonical == real || canonical.starts_with(&real))
}

pub(crate) fn git_common_is_shardrail(dir: &Path) -> io::Result<bool> {
    let Some(common) = git_common_dir(dir)? else {
        return Ok(false);
    };
    Ok(common_is_shardrail_git(&common))
}

fn git_common_dir(dir: &Path) -> io::Result<Option<PathBuf>> {
    git_common_dir_with(Path::new("git"), dir)
}

fn git_common_dir_with(git: &Path, dir: &Path) -> io::Result<Option<PathBuf>> {
    let output = Command::new(git)
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "session workdir {} git check failed: {error}",
                    dir.display()
                ),
            )
        })?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let reported = PathBuf::from(text);
    let resolved = if reported.is_absolute() {
        reported
    } else {
        dir.join(reported)
    };
    Ok(Some(fs::canonicalize(&resolved).unwrap_or(resolved)))
}

fn common_is_shardrail_git(common: &Path) -> bool {
    let expected = Path::new(SHARDRAIL_GIT);
    common == expected || fs::canonicalize(expected).is_ok_and(|real| common == real)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn startup_validation_rejects_root_home_repo_and_accepts_a_temp_dir() {
        let root = validate_session_workdir(Path::new("/")).unwrap_err();
        assert!(root.to_string().contains("refusing the filesystem root"));

        let home = std::env::var("HOME").unwrap();
        let home_error = validate_session_workdir(Path::new(&home)).unwrap_err();
        assert!(home_error.to_string().contains("refusing $HOME"));

        let worktree = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let repo = validate_session_workdir(&worktree).unwrap_err();
        assert!(
            repo.to_string()
                .contains("refusing a path inside the Shardrail repository"),
            "{repo}"
        );
        assert!(git_common_is_shardrail(&worktree).unwrap());

        let repo_root = Path::new(SHARDRAIL_PREFIX);
        let repo_root_error = validate_session_workdir(repo_root).unwrap_err();
        assert!(
            repo_root_error
                .to_string()
                .contains("refusing a path inside the Shardrail repository")
        );

        let testrepo = Path::new(S1_TESTREPO);
        if testrepo.is_dir() {
            let accepted = validate_session_workdir(testrepo).unwrap();
            assert_eq!(accepted, fs::canonicalize(testrepo).unwrap());
            assert!(!git_common_is_shardrail(testrepo).unwrap());
        }

        let directory = tempfile::tempdir().unwrap();
        let accepted = validate_session_workdir(directory.path()).unwrap();
        assert_eq!(accepted, fs::canonicalize(directory.path()).unwrap());
        assert!(!git_common_is_shardrail(directory.path()).unwrap());

        let file = directory.path().join("file");
        fs::write(&file, b"x").unwrap();
        let not_dir = validate_session_workdir(&file).unwrap_err();
        assert!(not_dir.to_string().contains("is not a directory"));

        let missing = directory.path().join("missing");
        let unresolved = validate_session_workdir(&missing).unwrap_err();
        assert!(unresolved.to_string().contains("cannot be resolved"));
    }

    #[test]
    fn external_worktree_phrase_is_distinct_from_the_repo_prefix() {
        assert_eq!(
            rejection_message(Path::new("/tmp/outside-worktree"), true),
            Some("refusing a Shardrail worktree")
        );
        assert_eq!(
            rejection_message(
                Path::new("/Users/daliys/Daliys/Shardrail/spikes/wt-w8"),
                false
            ),
            Some("refusing a path inside the Shardrail repository")
        );
        assert_eq!(rejection_message(Path::new(S1_TESTREPO), false), None);
        assert!(common_is_shardrail_git(Path::new(SHARDRAIL_GIT)));
        assert!(!common_is_shardrail_git(Path::new("/tmp/other/.git")));
    }

    #[test]
    fn git_check_fails_closed_when_git_cannot_be_spawned() {
        let directory = tempfile::tempdir().unwrap();
        let error = git_common_dir_with(Path::new("/nonexistent/shardrail-git"), directory.path())
            .unwrap_err();
        assert!(error.to_string().contains("git check failed"));
    }

    #[test]
    fn codex_workdir_is_the_default_and_duplicates_collapse() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let allowlist = collect_session_workdirs(
            Some(first.path()),
            &[second.path().to_path_buf(), first.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(allowlist.len(), 2);
        assert_eq!(allowlist[0], fs::canonicalize(first.path()).unwrap());
        assert_eq!(allowlist[1], fs::canonicalize(second.path()).unwrap());
        assert_eq!(format_allowlist(&[]), "(empty)");
    }

    #[test]
    fn requested_workdir_rejects_escape_and_accepts_the_canonical_path() {
        let real = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let canonical = fs::canonicalize(real.path()).unwrap();
        let allowlist = vec![canonical.clone()];

        assert_eq!(
            accept_requested_workdir(canonical.to_str().unwrap(), &allowlist).unwrap(),
            canonical
        );
        assert_eq!(
            accept_requested_workdir("relative/path", &allowlist).unwrap_err(),
            "workdir must be an absolute allowlisted path"
        );
        assert_eq!(
            accept_requested_workdir("", &allowlist).unwrap_err(),
            "workdir is invalid"
        );
        assert_eq!(
            accept_requested_workdir("bad\0path", &allowlist).unwrap_err(),
            "workdir is invalid"
        );

        let with_parent = format!(
            "{}/../{}",
            canonical.display(),
            canonical.file_name().unwrap().to_string_lossy()
        );
        assert_eq!(
            accept_requested_workdir(&with_parent, &allowlist).unwrap_err(),
            "workdir must not contain . or .."
        );
        assert_eq!(
            accept_requested_workdir(
                other.path().canonicalize().unwrap().to_str().unwrap(),
                &allowlist
            )
            .unwrap_err(),
            "workdir is not allowlisted"
        );

        let link_dir = tempfile::tempdir().unwrap();
        let link = link_dir.path().join("alias");
        symlink(real.path(), &link).unwrap();
        assert_eq!(validate_session_workdir(&link).unwrap(), canonical);
        assert_eq!(
            accept_requested_workdir(link.to_str().unwrap(), &allowlist).unwrap_err(),
            "workdir must not contain a symlink"
        );

        let nested = real.path().join("escape");
        symlink(other.path(), &nested).unwrap();
        assert_eq!(
            accept_requested_workdir(nested.to_str().unwrap(), &allowlist).unwrap_err(),
            "workdir must not contain a symlink"
        );
    }
}
