use nix::sys::signal::{Signal, kill, killpg};
use nix::unistd::Pid;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread::sleep;
use std::time::Duration;
use uuid::Uuid;

#[allow(dead_code)]
#[derive(Debug)]
pub struct ProcessContainment {
    pub session_id: Uuid,
    sentinel_path: PathBuf,
    sentinel_file: Option<File>,
    pgid: Option<i32>,
}

#[allow(dead_code)]
impl ProcessContainment {
    /// Create a new containment sentinel for a session.
    /// Opens the sentinel file with O_CLOEXEC cleared (inherited across fork/exec).
    pub fn new(session_id: Uuid) -> io::Result<Self> {
        let scratch = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let sentinel_path = scratch.join(format!("shardrail-sentinel-{}.tmp", session_id));

        // Create or truncate the sentinel file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(0) // Ensure O_CLOEXEC is not forced
            .open(&sentinel_path)?;

        #[cfg(unix)]
        {
            use nix::fcntl::{FcntlArg, FdFlag, fcntl};
            use std::os::fd::AsFd;
            // Clear FD_CLOEXEC explicitly so descendants inherit the descriptor
            let _ = fcntl(file.as_fd(), FcntlArg::F_SETFD(FdFlag::empty()));
        }

        Ok(Self {
            session_id,
            sentinel_path,
            sentinel_file: Some(file),
            pgid: None,
        })
    }

    pub fn set_pgid(&mut self, pgid: i32) {
        self.pgid = Some(pgid);
    }

    /// Find all alive PIDs holding the sentinel path, excluding the current process.
    pub fn find_contained_pids(&self) -> Vec<i32> {
        Self::pids_holding_path(&self.sentinel_path)
    }

    /// Query PIDs holding a given sentinel file path using `lsof -t`.
    pub fn pids_holding_path(path: &Path) -> Vec<i32> {
        if !path.exists() {
            return Vec::new();
        }

        let my_pid = std::process::id() as i32;
        let output = match Command::new("/usr/sbin/lsof").arg("-t").arg(path).output() {
            Ok(out) => out,
            Err(_) => return Vec::new(),
        };

        if !output.status.success() {
            return Vec::new();
        }

        let text = String::from_utf8_lossy(&output.stdout);
        text.lines()
            .filter_map(|line| line.trim().parse::<i32>().ok())
            .filter(|&pid| pid > 0 && pid != my_pid)
            .collect()
    }

    /// Terminate all processes in the contained process group and all holding the sentinel fd.
    pub fn terminate(&mut self) {
        // 1. Signal the process group if known
        if let Some(pgid) = self.pgid
            && pgid > 0
        {
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGTERM);
        }

        // 2. Discover and signal all PIDs holding the sentinel file
        let contained = self.find_contained_pids();
        for &pid in &contained {
            let _ = kill(Pid::from_raw(pid), Signal::SIGTERM);
        }

        // 3. Grace period for clean exit
        if self.pgid.is_some() || !contained.is_empty() {
            sleep(Duration::from_millis(50));
        }

        // 4. Force kill remaining alive processes
        if let Some(pgid) = self.pgid
            && pgid > 0
        {
            let _ = killpg(Pid::from_raw(pgid), Signal::SIGKILL);
        }

        let remaining = self.find_contained_pids();
        for &pid in &remaining {
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
        }

        // 5. Release sentinel descriptor and file
        self.sentinel_file = None;
        let _ = fs::remove_file(&self.sentinel_path);
    }
}

impl Drop for ProcessContainment {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_containment_creation_and_cleanup() {
        let session = Uuid::now_v7();
        let mut containment = ProcessContainment::new(session).unwrap();
        assert!(containment.sentinel_path.exists());

        containment.terminate();
        assert!(!containment.sentinel_path.exists());
    }

    #[test]
    fn test_contained_pid_detection() {
        let session = Uuid::now_v7();
        let containment = ProcessContainment::new(session).unwrap();

        // Spawn a background sleep process opening the sentinel file
        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "sleep 5 < \"{}\"",
                containment.sentinel_path.display()
            ))
            .spawn()
            .unwrap();

        // Give process a moment to open the file
        sleep(Duration::from_millis(100));

        let pids = containment.find_contained_pids();
        assert!(
            !pids.is_empty(),
            "expected at least one PID holding sentinel file"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
