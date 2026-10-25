//! Startup allowlist for session working directories.
//!
//! Paths are canonicalized before they are stored. On macOS, a firmlink spelling
//! such as `/System/Volumes/Data/Users/...` is folded to the `/Users/...` path
//! when both names are the same directory object. A request must name one of
//! those canonical paths exactly: relative paths, `.` / `..`, and any symlink
//! component are rejected before the canonical comparison.
//!
//! The admitted directory is an open file descriptor. Child processes start in
//! that directory object via `chdir-fd`, which calls `fchdir` before `exec`.
//! Replacing the path after admission does not move the child.

use nix::fcntl::{AT_FDCWD, FcntlArg, FdFlag, OFlag, fcntl, openat};
use nix::sys::stat::Mode;
use std::env;
use std::fs::{self, File};
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const SHARDRAIL_PREFIX: &str = "/Users/daliys/Daliys/Shardrail";
const SHARDRAIL_GIT: &str = "/Users/daliys/Daliys/Shardrail/.git";
const S1_TESTREPO: &str = "/Users/daliys/Daliys/Shardrail/spikes/s1-testrepo";
const DATA_VOLUME: &str = "/System/Volumes/Data";

#[derive(Clone, Debug)]
pub(crate) struct AllowedWorkdir {
    pub(crate) path: PathBuf,
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

pub(crate) struct PinnedWorkdir {
    pub(crate) path: PathBuf,
    pub(crate) directory: File,
}

impl std::fmt::Debug for PinnedWorkdir {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinnedWorkdir")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// Clears `FD_CLOEXEC` for the duration of one spawn so the child inherits the
/// directory descriptor. The flag is restored when this guard drops.
pub(crate) struct InheritedDirFd<'a> {
    file: &'a File,
}

impl<'a> InheritedDirFd<'a> {
    pub(crate) fn acquire(file: &'a File) -> io::Result<Self> {
        fcntl(file.as_fd(), FcntlArg::F_SETFD(FdFlag::empty())).map_err(io::Error::from)?;
        Ok(Self { file })
    }

    pub(crate) fn raw(&self) -> i32 {
        self.file.as_raw_fd()
    }
}

impl Drop for InheritedDirFd<'_> {
    fn drop(&mut self) {
        let _ = fcntl(self.file.as_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC));
    }
}

pub(crate) fn chdir_helper() -> &'static str {
    env!("SHARDRAIL_CHDIR_FD")
}

pub(crate) fn collect_session_workdirs(
    codex_workdir: Option<&Path>,
    session_workdirs: &[PathBuf],
) -> io::Result<Vec<AllowedWorkdir>> {
    let mut allowlist = Vec::new();
    if let Some(path) = codex_workdir {
        push_unique(&mut allowlist, validate_session_workdir(path)?);
    }
    for path in session_workdirs {
        push_unique(&mut allowlist, validate_session_workdir(path)?);
    }
    Ok(allowlist)
}

pub(crate) fn format_allowlist(paths: &[AllowedWorkdir]) -> String {
    if paths.is_empty() {
        "(empty)".to_owned()
    } else {
        paths
            .iter()
            .map(|entry| entry.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(crate) fn validate_session_workdir(path: &Path) -> io::Result<AllowedWorkdir> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "session workdir {} cannot be resolved: {error}",
                path.display()
            ),
        )
    })?;
    let logical = logical_path(&canonical);
    if !logical.is_dir() {
        return Err(workdir_error(&logical, "is not a directory"));
    }
    let shardrail_worktree = git_common_is_shardrail(&logical)?;
    if let Some(message) = rejection_message(&logical, shardrail_worktree) {
        return Err(workdir_error(&logical, message));
    }
    let directory = open_directory_nofollow(&logical)?;
    let metadata = directory.metadata()?;
    Ok(AllowedWorkdir {
        path: logical,
        dev: metadata.dev(),
        ino: metadata.ino(),
    })
}

/// Accept a client-supplied workdir only when it is exactly an allowlisted path
/// and still the same directory object recorded at startup.
pub(crate) fn accept_requested_workdir(
    raw: &str,
    allowlist: &[AllowedWorkdir],
) -> Result<PinnedWorkdir, &'static str> {
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
    let logical = logical_path(&canonical);
    let Some(entry) = allowlist.iter().find(|entry| entry.path == logical) else {
        return Err("workdir is not allowlisted");
    };
    pin_allowed(entry)
}

pub(crate) fn pin_allowed(entry: &AllowedWorkdir) -> Result<PinnedWorkdir, &'static str> {
    let directory =
        open_directory_nofollow(&entry.path).map_err(|_| "workdir is not allowlisted")?;
    let metadata = directory
        .metadata()
        .map_err(|_| "workdir is not allowlisted")?;
    if metadata.dev() != entry.dev || metadata.ino() != entry.ino || !metadata.is_dir() {
        return Err("workdir is not allowlisted");
    }
    Ok(PinnedWorkdir {
        path: entry.path.clone(),
        directory,
    })
}

fn push_unique(allowlist: &mut Vec<AllowedWorkdir>, entry: AllowedWorkdir) {
    if !allowlist
        .iter()
        .any(|existing| existing.path == entry.path || same_identity(existing, &entry))
    {
        allowlist.push(entry);
    }
}

fn same_identity(left: &AllowedWorkdir, right: &AllowedWorkdir) -> bool {
    left.dev == right.dev && left.ino == right.ino
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
    let Some(home) = env::var_os("HOME") else {
        return false;
    };
    let home = PathBuf::from(home);
    if canonical == home || same_dir(canonical, &home) {
        return true;
    }
    fs::canonicalize(&home).is_ok_and(|real| {
        let real = logical_path(&real);
        canonical == real || same_dir(canonical, &real)
    })
}

fn shardrail_prefix_rejected(canonical: &Path) -> bool {
    if s1_testrepo_exception(canonical) {
        return false;
    }
    let prefix = Path::new(SHARDRAIL_PREFIX);
    canonical == prefix || canonical.starts_with(prefix) || inside_directory(canonical, prefix)
}

fn s1_testrepo_exception(canonical: &Path) -> bool {
    let exception = Path::new(S1_TESTREPO);
    if canonical == exception || canonical.starts_with(exception) {
        return true;
    }
    if let Ok(real) = fs::canonicalize(exception) {
        let real = logical_path(&real);
        if canonical == real || canonical.starts_with(&real) {
            return true;
        }
    }
    inside_directory(canonical, exception)
}

fn logical_path(canonical: &Path) -> PathBuf {
    let data = Path::new(DATA_VOLUME);
    if let Ok(rest) = canonical.strip_prefix(data) {
        let logical = Path::new("/").join(rest);
        if same_dir(canonical, &logical) {
            return logical;
        }
    }
    canonical.to_path_buf()
}

fn same_dir(left: &Path, right: &Path) -> bool {
    match (fs::metadata(left), fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

fn inside_directory(path: &Path, root: &Path) -> bool {
    let Ok(root_meta) = fs::metadata(root) else {
        return false;
    };
    let mut cursor = path.to_path_buf();
    loop {
        if let Ok(meta) = fs::metadata(&cursor)
            && meta.dev() == root_meta.dev()
            && meta.ino() == root_meta.ino()
        {
            return true;
        }
        if !cursor.pop() {
            return false;
        }
    }
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
    let resolved = fs::canonicalize(&resolved).unwrap_or(resolved);
    Ok(Some(logical_path(&resolved)))
}

fn common_is_shardrail_git(common: &Path) -> bool {
    let common = logical_path(&fs::canonicalize(common).unwrap_or_else(|_| common.to_path_buf()));
    let expected = Path::new(SHARDRAIL_GIT);
    if common == expected || same_dir(&common, expected) {
        return true;
    }
    fs::canonicalize(expected).is_ok_and(|real| {
        let real = logical_path(&real);
        common == real || same_dir(&common, &real)
    })
}

fn directory_flags() -> OFlag {
    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW
}

fn open_directory_nofollow(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session workdir must be absolute",
        ));
    }
    let mut current: Option<OwnedFd> = None;
    for component in path.components() {
        current = Some(match component {
            Component::RootDir => open_root()?,
            Component::Normal(name) => {
                let parent = current.as_ref().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "session workdir is not absolute",
                    )
                })?;
                openat(parent, Path::new(name), directory_flags(), Mode::empty())
                    .map_err(io::Error::from)?
            }
            Component::CurDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "session workdir must not contain . or ..",
                ));
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "session workdir is not absolute",
                ));
            }
        });
    }
    let Some(fd) = current else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session workdir is empty",
        ));
    };
    Ok(File::from(fd))
}

fn open_root() -> io::Result<OwnedFd> {
    openat(AT_FDCWD, "/", directory_flags(), Mode::empty()).map_err(io::Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::process::Stdio;

    fn canonical_path(path: &Path) -> PathBuf {
        logical_path(&fs::canonicalize(path).unwrap())
    }

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
            assert_eq!(accepted.path, canonical_path(testrepo));
            assert!(!git_common_is_shardrail(testrepo).unwrap());
        }

        let directory = tempfile::tempdir().unwrap();
        let accepted = validate_session_workdir(directory.path()).unwrap();
        assert_eq!(accepted.path, canonical_path(directory.path()));
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
    fn data_volume_firmlinks_to_home_and_the_repo_are_refused() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let data_home = Path::new(DATA_VOLUME).join(home.strip_prefix("/").unwrap());
        if !same_dir(&home, &data_home) {
            return;
        }
        let home_error = validate_session_workdir(&data_home).unwrap_err();
        assert!(
            home_error.to_string().contains("refusing $HOME"),
            "{home_error}"
        );

        let repo = Path::new(SHARDRAIL_PREFIX);
        let data_repo = Path::new(DATA_VOLUME).join(repo.strip_prefix("/").unwrap());
        let repo_error = validate_session_workdir(&data_repo).unwrap_err();
        assert!(
            repo_error
                .to_string()
                .contains("refusing a path inside the Shardrail repository"),
            "{repo_error}"
        );

        let nested = repo.join("crates");
        let data_nested = Path::new(DATA_VOLUME).join(nested.strip_prefix("/").unwrap());
        if data_nested.is_dir() {
            let nested_error = validate_session_workdir(&data_nested).unwrap_err();
            assert!(
                nested_error
                    .to_string()
                    .contains("refusing a path inside the Shardrail repository"),
                "{nested_error}"
            );
        }

        let testrepo = Path::new(S1_TESTREPO);
        let data_testrepo = Path::new(DATA_VOLUME).join(testrepo.strip_prefix("/").unwrap());
        if data_testrepo.is_dir() && same_dir(testrepo, &data_testrepo) {
            let accepted = validate_session_workdir(&data_testrepo).unwrap();
            assert_eq!(accepted.path, canonical_path(testrepo));
        }
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
        assert_eq!(allowlist[0].path, canonical_path(first.path()));
        assert_eq!(allowlist[1].path, canonical_path(second.path()));
        assert_eq!(format_allowlist(&[]), "(empty)");
    }

    #[test]
    fn requested_workdir_rejects_escape_and_accepts_the_canonical_path() {
        let real = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let canonical = canonical_path(real.path());
        let allowlist = vec![validate_session_workdir(real.path()).unwrap()];

        assert_eq!(
            accept_requested_workdir(canonical.to_str().unwrap(), &allowlist)
                .unwrap()
                .path,
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
        assert_eq!(validate_session_workdir(&link).unwrap().path, canonical);
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

    #[test]
    fn replaced_allowlisted_directory_is_rejected_before_pin() {
        let directory = tempfile::tempdir().unwrap();
        let allowed = validate_session_workdir(directory.path()).unwrap();
        let original = allowed.path.clone();
        let saved = original.with_file_name(format!(
            "{}-saved",
            original.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(&original, &saved).unwrap();
        fs::create_dir(&original).unwrap();
        let error = accept_requested_workdir(original.to_str().unwrap(), &[allowed]).unwrap_err();
        assert_eq!(error, "workdir is not allowlisted");
    }

    #[test]
    fn pinned_directory_survives_replacement_before_spawn() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("PINNED"), b"yes").unwrap();
        let allowed = validate_session_workdir(directory.path()).unwrap();
        let pinned = pin_allowed(&allowed).unwrap();
        let saved = pinned.path.with_file_name(format!(
            "{}-saved",
            pinned.path.file_name().unwrap().to_string_lossy()
        ));
        fs::rename(&pinned.path, &saved).unwrap();
        fs::create_dir(&pinned.path).unwrap();
        fs::write(pinned.path.join("SWAPPED"), b"yes").unwrap();

        let held = InheritedDirFd::acquire(&pinned.directory).unwrap();
        let output = Command::new(chdir_helper())
            .arg(held.raw().to_string())
            .arg("/bin/sh")
            .arg("-c")
            .arg("if test -f PINNED; then echo PINNED; fi; if test -f SWAPPED; then echo SWAPPED; fi")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        drop(held);
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "status {:?} stderr {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("PINNED"), "{text}");
        assert!(!text.contains("SWAPPED"), "{text}");
    }
}
