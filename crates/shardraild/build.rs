use std::env;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let helper = Path::new(&out_dir).join("chdir-fd");
    let status = Command::new("cc")
        .arg("-Os")
        .arg("-o")
        .arg(&helper)
        .arg("chdir_fd.c")
        .status()
        .expect("cc is required to build the workdir pin helper");
    assert!(status.success(), "chdir-fd helper failed to compile");
    println!("cargo:rustc-env=SHARDRAIL_CHDIR_FD={}", helper.display());

    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let (repo_root, git_dir) = shardrail_checkout(&manifest);
    let testrepo = repo_root.join("spikes/s1-testrepo");
    println!(
        "cargo:rustc-env=SHARDRAIL_REPO_ROOT={}",
        repo_root.display()
    );
    println!("cargo:rustc-env=SHARDRAIL_GIT_DIR={}", git_dir.display());
    println!(
        "cargo:rustc-env=SHARDRAIL_S1_TESTREPO={}",
        testrepo.display()
    );

    println!("cargo:rerun-if-changed=chdir_fd.c");
    println!("cargo:rerun-if-changed=build.rs");
}

/// The checkout that contains this crate, via `git rev-parse --git-common-dir`.
/// Worktrees resolve to the main repository, not the worktree directory.
fn shardrail_checkout(manifest: &Path) -> (PathBuf, PathBuf) {
    let output = Command::new("git")
        .arg("-C")
        .arg(manifest)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .expect("git rev-parse --git-common-dir");
    assert!(
        output.status.success(),
        "git rev-parse --git-common-dir failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("git common dir is utf-8");
    let reported = PathBuf::from(text.trim());
    let git_dir = if reported.is_absolute() {
        reported
    } else {
        manifest.join(reported)
    };
    let git_dir = fs::canonicalize(&git_dir).unwrap_or(git_dir);
    let git_dir = logical_path(&git_dir);
    let repo_root = git_dir
        .parent()
        .expect("git common dir has a parent")
        .to_path_buf();
    (logical_path(&repo_root), git_dir)
}

fn logical_path(canonical: &Path) -> PathBuf {
    let data = Path::new("/System/Volumes/Data");
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
