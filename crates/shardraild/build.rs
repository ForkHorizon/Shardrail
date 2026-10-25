use std::env;
use std::path::Path;
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
    println!("cargo:rerun-if-changed=chdir_fd.c");
}
