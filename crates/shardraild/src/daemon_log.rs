use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_LIMIT: u64 = 1024 * 1024;
const FILE_COUNT: usize = 5;

#[derive(Clone, Copy)]
enum Platform {
    Macos,
    Linux,
    Other,
}

fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::Other
    }
}

fn selected_dir(
    platform: Platform,
    override_dir: Option<PathBuf>,
    home: Option<&Path>,
    xdg_state_home: Option<&Path>,
    service: bool,
) -> Option<PathBuf> {
    if let Some(path) = override_dir {
        return Some(path);
    }
    match platform {
        Platform::Macos => home.map(|home| home.join("Library/Logs/Shardrail")),
        Platform::Linux if service => None,
        Platform::Linux => xdg_state_home
            .filter(|path| path.is_absolute())
            .map(Path::to_path_buf)
            .or_else(|| home.map(|home| home.join(".local/state")))
            .map(|state| state.join("shardrail/logs")),
        Platform::Other => None,
    }
}

pub fn configured_dir(override_dir: Option<PathBuf>, disabled: bool) -> Option<PathBuf> {
    let env_override = std::env::var_os("SHARDRAIL_LOG_DIR").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let xdg = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    let service =
        std::env::var_os("JOURNAL_STREAM").is_some() || std::env::var_os("INVOCATION_ID").is_some();
    configured_dir_from(
        platform(),
        override_dir,
        env_override,
        home,
        xdg,
        service,
        disabled,
    )
}

fn configured_dir_from(
    platform: Platform,
    override_dir: Option<PathBuf>,
    env_override: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg: Option<PathBuf>,
    service: bool,
    disabled: bool,
) -> Option<PathBuf> {
    if disabled {
        return None;
    }
    selected_dir(
        platform,
        override_dir.or(env_override),
        home.as_deref(),
        xdg.as_deref(),
        service,
    )
}

pub struct Logger {
    state: Mutex<State>,
}

struct State {
    directory: Option<PathBuf>,
    file: Option<File>,
    failed: bool,
}

impl Logger {
    pub fn new(directory: Option<PathBuf>) -> Self {
        let directory = directory
            .map(|path| absolute_directory(path, &std::env::current_dir().unwrap_or_default()));
        let logger = Self {
            state: Mutex::new(State {
                directory: directory.clone(),
                file: None,
                failed: false,
            }),
        };
        if let Some(directory) = directory {
            let mut state = logger.state.lock().unwrap();
            match open_log(&directory) {
                Ok(file) => state.file = Some(file),
                Err(error) => {
                    state.failed = true;
                    state.directory = None;
                    let _ = writeln!(io::stderr(), "shardraild log file disabled: {error}");
                }
            }
        }
        logger
    }

    pub fn log(&self, level: &str, component: &str, message: &str) {
        let line = format!("{} {level} {component} {}\n", timestamp(), redact(message));
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let _ = io::stderr().lock().write_all(line.as_bytes());
        if let Some(directory) = state.directory.clone() {
            let result = if state.file.is_none() {
                open_log(&directory).map(|file| state.file = Some(file))
            } else {
                Ok(())
            };
            if let Err(error) = result {
                if !state.failed {
                    state.failed = true;
                    let _ = writeln!(io::stderr(), "shardraild log file disabled: {error}");
                }
                state.directory = None;
                state.file = None;
                return;
            }
            let result = (|| {
                let mut file = state.file.take().expect("opened above");
                let len = file.metadata()?.len();
                if len.saturating_add(line.len() as u64) > FILE_LIMIT {
                    drop(file);
                    self.rotate(&directory)?;
                    file = open_log(&directory)?;
                }
                file.write_all(line.as_bytes())?;
                file.flush()?;
                Ok::<_, io::Error>(file)
            })();
            match result {
                Err(error) => {
                    if !state.failed {
                        state.failed = true;
                        let _ = writeln!(io::stderr(), "shardraild log file disabled: {error}");
                    }
                    state.directory = None;
                    state.file = None;
                }
                Ok(file) => state.file = Some(file),
            }
        }
    }

    fn rotate(&self, directory: &Path) -> io::Result<()> {
        let oldest = directory.join(format!("shardraild.log.{}", FILE_COUNT - 1));
        match fs::remove_file(oldest) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        for index in (1..FILE_COUNT - 1).rev() {
            let from = directory.join(format!("shardraild.log.{index}"));
            let to = directory.join(format!("shardraild.log.{}", index + 1));
            match fs::rename(from, to) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        let active = directory.join("shardraild.log");
        let first = directory.join("shardraild.log.1");
        match fs::rename(active, first) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let mut total = 0;
        for index in (1..FILE_COUNT).rev() {
            let path = directory.join(format!("shardraild.log.{index}"));
            match fs::metadata(&path) {
                Ok(metadata) => {
                    total += metadata.len();
                    if total > FILE_LIMIT * (FILE_COUNT - 1) as u64 {
                        fs::remove_file(path)?;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

fn absolute_directory(path: PathBuf, current_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        current_dir.join(path)
    }
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

pub fn init(directory: Option<PathBuf>) {
    let _ = LOGGER.set(Logger::new(directory));
}

pub fn log(level: &str, component: &str, message: &str) {
    if let Some(logger) = LOGGER.get() {
        logger.log(level, component, message);
    } else {
        let _ = writeln!(
            io::stderr(),
            "{} {level} {component} {}",
            timestamp(),
            redact(message)
        );
    }
}

fn open_log(directory: &Path) -> io::Result<File> {
    fs::create_dir_all(directory)?;
    #[cfg(unix)]
    let directory_fd = nix::fcntl::open(
        directory,
        nix::fcntl::OFlag::O_RDONLY
            | nix::fcntl::OFlag::O_DIRECTORY
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::empty(),
    )
    .map_err(io::Error::from)?;
    #[cfg(unix)]
    {
        let metadata = nix::sys::stat::fstat(&directory_fd).map_err(io::Error::from)?;
        if !nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
            .contains(nix::sys::stat::SFlag::S_IFDIR)
            || metadata.st_uid != nix::unistd::geteuid().as_raw()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log directory must be owned by the current user",
            ));
        }
    }
    #[cfg(unix)]
    nix::sys::stat::fchmod(
        &directory_fd,
        nix::sys::stat::Mode::from_bits_truncate(0o700),
    )
    .map_err(io::Error::from)?;
    #[cfg(unix)]
    let file_fd = nix::fcntl::openat(
        &directory_fd,
        "shardraild.log",
        nix::fcntl::OFlag::O_WRONLY
            | nix::fcntl::OFlag::O_APPEND
            | nix::fcntl::OFlag::O_CREAT
            | nix::fcntl::OFlag::O_NOFOLLOW
            | nix::fcntl::OFlag::O_CLOEXEC
            | nix::fcntl::OFlag::O_NONBLOCK,
        nix::sys::stat::Mode::from_bits_truncate(0o600),
    )
    .map_err(io::Error::from)?;
    #[cfg(unix)]
    let file = {
        let metadata = nix::sys::stat::fstat(&file_fd).map_err(io::Error::from)?;
        if !nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
            .contains(nix::sys::stat::SFlag::S_IFREG)
            || metadata.st_uid != nix::unistd::geteuid().as_raw()
            || metadata.st_nlink != 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log must be an owner-owned regular file without hard links",
            ));
        }
        File::from(file_fd)
    };
    #[cfg(not(unix))]
    let file = File::options()
        .create(true)
        .append(true)
        .open(directory.join("shardraild.log"))?;
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn redact(message: &str) -> String {
    let message = message
        .chars()
        .filter(|character| !character.is_control())
        .take(512)
        .collect::<String>();
    let lower = message.to_ascii_lowercase();
    if [
        "enrollment_code",
        "enrollment code",
        "private_key=",
        "private_key:",
        "private-key=",
        "private-key:",
        "private key=",
        "privatekey=",
        "authorization:",
        "authorization=",
        "bearer ",
        "api_key=",
        "api_key:",
        "api-key=",
        "api-key:",
        "signature=",
        "nonce=",
        "token=",
        "access_token=",
        "secret=",
        "auth=",
        "credential=",
        "password=",
        "cookie=",
        "prompt=",
        "tool_output=",
        "tool-output=",
        "enrollment code ",
        "sk-live-",
        "sk-ant-",
        "ghp_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "akia",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        return "[REDACTED]".into();
    }
    let mut output = String::with_capacity(message.len());
    let mut token = String::new();
    for ch in message.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_alphanumeric() || "_+=-".contains(ch) {
            token.push(ch);
        } else {
            if token.len() >= 32 && entropy(&token) >= 3.0 && !is_uuid(&token) {
                output.push_str("[REDACTED]");
            } else {
                output.push_str(&token);
            }
            token.clear();
            output.push(ch);
        }
    }
    output.trim_end().to_owned()
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && [8, 13, 18, 23]
            .iter()
            .all(|index| value.as_bytes()[*index] == b'-')
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn entropy(value: &str) -> f64 {
    let mut counts = [0u32; 128];
    for byte in value.bytes().filter(|byte| *byte < 128) {
        counts[byte as usize] += 1;
    }
    let len = value.len() as f64;
    counts
        .into_iter()
        .filter(|count| *count != 0)
        .map(|count| {
            let probability = f64::from(count) / len;
            -probability * probability.log2()
        })
        .sum()
}

fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_seconds / 3600,
        (day_seconds % 3600) / 60,
        day_seconds % 60,
        now.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_platform_paths_and_overrides() {
        let home = Path::new("/home/user");
        let xdg = Path::new("/state");
        assert_eq!(
            selected_dir(Platform::Macos, None, Some(home), None, false),
            Some(home.join("Library/Logs/Shardrail"))
        );
        assert_eq!(
            selected_dir(Platform::Linux, None, Some(home), Some(xdg), false),
            Some(xdg.join("shardrail/logs"))
        );
        assert_eq!(
            selected_dir(Platform::Linux, None, Some(home), None, false),
            Some(home.join(".local/state/shardrail/logs"))
        );
        assert_eq!(
            selected_dir(
                Platform::Linux,
                None,
                Some(home),
                Some(Path::new("relative")),
                false
            ),
            Some(home.join(".local/state/shardrail/logs"))
        );
        assert_eq!(selected_dir(Platform::Macos, None, None, None, false), None);
        assert_eq!(selected_dir(Platform::Linux, None, None, None, false), None);
        assert_eq!(
            selected_dir(Platform::Linux, None, Some(home), Some(xdg), true),
            None
        );
        assert_eq!(
            selected_dir(
                Platform::Linux,
                Some(PathBuf::from("/tmp/logs")),
                None,
                None,
                true
            ),
            Some(PathBuf::from("/tmp/logs"))
        );
        assert_eq!(configured_dir(None, true), None);
        assert_eq!(
            configured_dir_from(
                Platform::Macos,
                None,
                Some(PathBuf::from("/env")),
                None,
                None,
                false,
                false
            ),
            Some(PathBuf::from("/env"))
        );
        assert_eq!(
            configured_dir_from(
                Platform::Linux,
                None,
                None,
                Some(home.to_path_buf()),
                None,
                true,
                false
            ),
            None
        );
        assert_eq!(
            configured_dir_from(
                Platform::Linux,
                None,
                None,
                Some(home.to_path_buf()),
                None,
                false,
                true
            ),
            None
        );
    }

    #[test]
    fn writes_private_files_and_rotates_within_count_and_total_cap() {
        let dir = tempfile::tempdir().unwrap();
        let logger = Logger::new(Some(dir.path().into()));
        for index in 1..FILE_COUNT {
            let path = dir.path().join(format!("shardraild.log.{index}"));
            fs::write(&path, b"rotated").unwrap();
            #[cfg(unix)]
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fs::write(
            dir.path().join("shardraild.log"),
            vec![b'x'; FILE_LIMIT as usize],
        )
        .unwrap();
        logger.log("INFO", "test", "rotation boundary");
        assert_eq!(
            &fs::read(dir.path().join("shardraild.log.1")).unwrap()[..4],
            b"xxxx"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("shardraild.log.4")).unwrap(),
            "rotated"
        );
        let files = fs::read_dir(dir.path())
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(files.len() <= FILE_COUNT);
        let total = files
            .iter()
            .map(|entry| entry.metadata().unwrap().len())
            .sum::<u64>();
        assert!(total <= FILE_LIMIT * FILE_COUNT as u64);
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(dir.path().join("shardraild.log"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn redacts_enrollment_and_key_like_values_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let logger = Logger::new(Some(dir.path().into()));
        logger.log("WARN", "test", "enrollment_code=123456");
        let key = "Qm9tV2l6a0p4Y2xRbFh2N1N4bE5vV2V5WjRrU2J1R3A";
        logger.log("WARN", "test", &format!("session ended: {key}"));
        for sensitive in [
            "private_key=short-value",
            "signature=short-value",
            "nonce=short-value",
            "auth=short-value",
            "prompt=private body",
            "tool_output=private body",
            "authorization=secret-material",
        ] {
            logger.log("WARN", "test", sensitive);
        }
        let host = "c3c783db-5e04-4b53-8d09-b6b80153beba";
        logger.log(
            "INFO",
            "daemon",
            &format!("client authenticated; host {host}"),
        );
        logger.log("WARN", "client", "invalid signature on hello");
        let contents = fs::read_to_string(dir.path().join("shardraild.log")).unwrap();
        assert!(!contents.contains("123456"));
        assert!(!contents.contains(key));
        assert!(!contents.contains("short-value"));
        assert!(!contents.contains("private body"));
        assert!(!contents.contains("secret-material"));
        assert!(contents.contains(host));
        assert!(contents.contains("client authenticated"));
        assert!(contents.contains("invalid signature on hello"));
    }

    #[test]
    fn failed_open_keeps_logging_to_stderr_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        fs::write(&file, "x").unwrap();
        let logger = Logger::new(Some(file));
        logger.log("ERROR", "test", "still reported");
        let state = logger.state.lock().unwrap();
        assert!(state.failed);
        assert!(state.directory.is_none());
        assert!(state.file.is_none());
    }

    #[test]
    fn failed_rotation_falls_back_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let logger = Logger::new(Some(dir.path().into()));
        fs::write(
            dir.path().join("shardraild.log"),
            vec![b'x'; FILE_LIMIT as usize],
        )
        .unwrap();
        #[cfg(unix)]
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o500)).unwrap();
        logger.log("ERROR", "test", "rotation failure");
        let state = logger.state.lock().unwrap();
        assert!(state.failed);
        assert!(state.directory.is_none());
        assert!(state.file.is_none());
        drop(state);
        #[cfg(unix)]
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn refuses_fifo_and_hard_link_without_blocking_or_modifying_target() {
        let dir = tempfile::tempdir().unwrap();
        let fifo_dir = dir.path().join("fifo");
        fs::create_dir(&fifo_dir).unwrap();
        nix::unistd::mkfifo(
            fifo_dir.join("shardraild.log").as_path(),
            nix::sys::stat::Mode::from_bits_truncate(0o600),
        )
        .unwrap();
        let started = std::time::Instant::now();
        let logger = Logger::new(Some(fifo_dir));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(logger.state.lock().unwrap().failed);

        let link_dir = dir.path().join("link");
        fs::create_dir(&link_dir).unwrap();
        let sentinel = dir.path().join("sentinel");
        fs::write(&sentinel, "unchanged").unwrap();
        fs::hard_link(&sentinel, link_dir.join("shardraild.log")).unwrap();
        let mode = fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777;
        let logger = Logger::new(Some(link_dir));
        assert!(logger.state.lock().unwrap().failed);
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
        assert_eq!(
            fs::metadata(&sentinel).unwrap().permissions().mode() & 0o777,
            mode
        );
    }

    #[test]
    fn stores_relative_log_directory_as_absolute_and_drops_oversized_old_generations() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            absolute_directory(PathBuf::from("logs"), dir.path()),
            dir.path().join("logs")
        );

        let logger = Logger::new(Some(dir.path().into()));
        fs::write(
            dir.path().join("shardraild.log"),
            vec![b'x'; FILE_LIMIT as usize],
        )
        .unwrap();
        fs::write(
            dir.path().join("shardraild.log.1"),
            vec![b'y'; (FILE_LIMIT * FILE_COUNT as u64) as usize],
        )
        .unwrap();
        logger.log("INFO", "test", "cap marker");
        let total = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        assert!(total <= FILE_LIMIT * FILE_COUNT as u64);
    }
}
