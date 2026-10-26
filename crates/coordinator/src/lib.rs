#[cfg(target_os = "macos")]
use nix::fcntl::{FcntlArg, fcntl};
use nix::fcntl::{OFlag, openat};
use nix::sys::stat::{Mode, fchmod, fstat, mkdirat};
use shardrail_journal::{Journal, JournalError};
use shardrail_protocol::{
    ApprovalDecision, ApprovalResponding, ClientScope, DurableEvent, OptionClass,
};
use shardrail_session_core::{Approval, Session};
use std::fs::{self, File};
use std::io;
use std::os::fd::AsFd;
#[cfg(target_os = "linux")]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum CoordinatorError {
    #[error("journal unavailable; operation was not sent: {0}")]
    Journal(#[from] JournalError),
    #[error("approval is not pending or its decision is not authorized")]
    InvalidApproval,
    #[error("session transition is not valid from the current state")]
    InvalidTransition,
    #[error("data directory is not a directory")]
    InvalidDataDirectory,
    #[error("another Shardrail daemon is already running")]
    AlreadyRunning,
    #[error("journal is unhealthy; operation was not sent")]
    Unhealthy,
    #[error("journal contains an invalid session snapshot: {0}")]
    InvalidSnapshot(#[from] serde_json::Error),
    #[error("data directory I/O error: {0}")]
    Io(#[from] io::Error),
}

pub struct ApprovalResponseRequest<'a> {
    pub decision: ApprovalDecision,
    pub authorized: bool,
    pub now_ms: i64,
    pub provider_limit_secs: Option<u64>,
    pub option_id: Option<String>,
    pub responder: &'a str,
    pub option_class: OptionClass,
    pub client_scope: ClientScope,
}

pub struct ApprovalTimeoutRequest {
    pub now_ms: i64,
    pub provider_limit_secs: Option<u64>,
    pub option_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailClosedPlan {
    pub cancel_turn: bool,
    pub pending_approvals: Vec<Uuid>,
}

/// Owns M01's private data directory and process-lifetime advisory lock.
pub struct DaemonDataDir {
    path: PathBuf,
    _directory: File,
    _lock: File,
}

impl DaemonDataDir {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CoordinatorError> {
        let directory = open_private_directory(path.as_ref())?;
        let anchored_path = descriptor_path(&directory).map_err(CoordinatorError::Io)?;
        let lock_fd = openat(
            directory.as_fd(),
            "daemon.lock",
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(io::Error::other)?;
        let lock_meta = fstat(&lock_fd).map_err(io::Error::other)?;
        if !nix::sys::stat::SFlag::from_bits_truncate(lock_meta.st_mode)
            .contains(nix::sys::stat::SFlag::S_IFREG)
            || lock_meta.st_uid != nix::unistd::geteuid().as_raw()
        {
            return Err(CoordinatorError::InvalidDataDirectory);
        }
        fchmod(&lock_fd, Mode::from_bits_truncate(0o600)).map_err(io::Error::other)?;
        let lock_meta = fstat(&lock_fd).map_err(io::Error::other)?;
        if lock_meta.st_mode & 0o777 != 0o600 {
            return Err(CoordinatorError::InvalidDataDirectory);
        }
        let lock: File = lock_fd.into();
        match lock.try_lock() {
            Ok(()) => Ok(Self {
                path: anchored_path,
                _directory: directory,
                _lock: lock,
            }),
            Err(std::fs::TryLockError::WouldBlock) => Err(CoordinatorError::AlreadyRunning),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub fn journal_path(&self) -> PathBuf {
        self.path.join("journal.sqlite3")
    }

    pub fn directory_path(&self) -> &Path {
        &self.path
    }

    /// Pins path-only APIs to this directory for the daemon process on macOS.
    pub fn pin_path_only_apis(&mut self) -> io::Result<()> {
        pin_data_directory(&self._directory)?;
        #[cfg(target_os = "macos")]
        {
            self.path = PathBuf::from(".");
        }
        Ok(())
    }

    pub fn open_journal(&self) -> Result<Journal, JournalError> {
        Journal::open(self.journal_path())
    }

    pub fn open_journal_entry(&self) -> io::Result<std::os::fd::OwnedFd> {
        let entry = openat(
            self._directory.as_fd(),
            "journal.sqlite3",
            OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(io::Error::other)?;
        let metadata = fstat(&entry).map_err(io::Error::other)?;
        if !nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
            .contains(nix::sys::stat::SFlag::S_IFREG)
            || metadata.st_uid != nix::unistd::geteuid().as_raw()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal must be an owner-owned regular file",
            ));
        }
        fchmod(&entry, Mode::from_bits_truncate(0o600)).map_err(io::Error::other)?;
        Ok(entry)
    }
}

pub fn pin_data_directory(directory: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    nix::unistd::fchdir(directory).map_err(io::Error::other)?;
    Ok(())
}

fn descriptor_path(directory: &File) -> io::Result<PathBuf> {
    #[cfg(target_os = "linux")]
    return Ok(PathBuf::from("/proc/self/fd").join(directory.as_raw_fd().to_string()));
    #[cfg(target_os = "macos")]
    {
        let mut path = PathBuf::new();
        fcntl(directory, FcntlArg::F_GETPATH(&mut path)).map_err(io::Error::other)?;
        Ok(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = directory;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "descriptor-anchored data paths are unsupported on this platform",
        ))
    }
}

pub fn open_private_data_directory(path: impl AsRef<Path>) -> io::Result<(File, PathBuf)> {
    let directory = open_private_directory(path.as_ref())?;
    let anchored_path = descriptor_path(&directory)?;
    Ok((directory, anchored_path))
}

fn open_private_directory(path: &Path) -> io::Result<File> {
    open_private_directory_with(path, || {})
}

fn open_private_directory_with(path: &Path, after_open: impl FnOnce()) -> io::Result<File> {
    open_private_directory_with_hooks(path, || {}, after_open)
}

fn open_private_directory_with_hooks(
    path: &Path,
    before_open: impl FnOnce(),
    after_open: impl FnOnce(),
) -> io::Result<File> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent_path = path.parent().unwrap_or(Path::new("/"));
    fs::create_dir_all(parent_path)?;
    let parent = File::open(parent_path)?;
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "data directory path has no final component",
        )
    })?;
    match mkdirat(parent.as_fd(), name, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
        Err(error) => return Err(io::Error::other(error)),
    }
    before_open();
    let directory_fd = openat(
        parent.as_fd(),
        name,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(io::Error::other)?;
    let directory: File = directory_fd.into();
    after_open();
    let metadata = fstat(&directory).map_err(io::Error::other)?;
    if !nix::sys::stat::SFlag::from_bits_truncate(metadata.st_mode)
        .contains(nix::sys::stat::SFlag::S_IFDIR)
        || metadata.st_uid != nix::unistd::geteuid().as_raw()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "data directory must be owned by the current user",
        ));
    }
    fchmod(&directory, Mode::from_bits_truncate(0o700)).map_err(io::Error::other)?;
    let metadata = fstat(&directory).map_err(io::Error::other)?;
    if metadata.st_mode & 0o777 != 0o700 || metadata.st_uid != nix::unistd::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "data directory ownership or permissions are invalid",
        ));
    }
    Ok(directory)
}

/// Provider effects are invoked only after their event has committed.
pub struct Coordinator {
    journal: Journal,
    session_id: Uuid,
    session: Session,
    healthy: bool,
    fail_closed_effect: Option<Box<dyn FnMut(FailClosedPlan) -> io::Result<()> + Send>>,
}

impl Coordinator {
    /// Runs once after daemon startup and before opening session coordinators.
    /// Returns how many pending approvals were abandoned. They are not answerable.
    pub fn recover_journal(journal: &mut Journal) -> Result<usize, CoordinatorError> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        journal.recover_responding(now_ms)?;
        Ok(journal.recover_pending_approvals(now_ms)?)
    }

    pub fn open(journal: Journal, session_id: Uuid) -> Result<Self, CoordinatorError> {
        let events = journal.events_for_session(session_id)?;
        let session = recover_session(replay_session(&events)?);
        Ok(Self {
            journal,
            session_id,
            session,
            healthy: true,
            fail_closed_effect: None,
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn journal_healthy(&self) -> bool {
        self.healthy
    }

    pub fn set_fail_closed_effect(
        &mut self,
        effect: impl FnMut(FailClosedPlan) -> io::Result<()> + Send + 'static,
    ) {
        self.fail_closed_effect = Some(Box::new(effect));
    }

    pub fn approval_state(&self, approval_id: Uuid) -> Result<Option<String>, CoordinatorError> {
        Ok(self.journal.approval_state(approval_id)?)
    }

    #[cfg(feature = "test-utils")]
    pub fn fail_approval_writes_for_test(&self) -> Result<(), CoordinatorError> {
        Ok(self.journal.fail_approval_writes_for_test()?)
    }

    pub fn transition<T>(
        &mut self,
        now_ms: i64,
        kind: &str,
        payload: serde_json::Value,
        effect: impl FnOnce() -> io::Result<T>,
    ) -> Result<T, CoordinatorError> {
        self.ensure_healthy()?;
        let mut next_session = self.session.clone();
        apply_transition(&mut next_session, kind)
            .map_err(|_| CoordinatorError::InvalidTransition)?;
        let event_kind = match (kind, &next_session.lifecycle) {
            ("session_closed", shardrail_protocol::SessionLifecycle::Closing) => "session_closing",
            ("process_exited", shardrail_protocol::SessionLifecycle::Closed) => "session_closed",
            _ => kind,
        };
        self.transition_to(now_ms, event_kind, payload, next_session, effect)
    }

    /// Persists a state produced by the pure session core before invoking its external effect.
    pub fn transition_session<T>(
        &mut self,
        now_ms: i64,
        kind: &str,
        payload: serde_json::Value,
        next_session: Session,
        effect: impl FnOnce() -> io::Result<T>,
    ) -> Result<T, CoordinatorError> {
        self.ensure_healthy()?;
        self.transition_to(now_ms, kind, payload, next_session, effect)
    }

    pub fn turn_finished<T>(
        &mut self,
        now_ms: i64,
        outcome: shardrail_session_core::TurnOutcome,
        dispatch_guard: bool,
        payload: serde_json::Value,
        effect: impl FnOnce() -> io::Result<T>,
    ) -> Result<T, CoordinatorError> {
        self.ensure_healthy()?;
        let mut next_session = self.session.clone();
        if next_session.turn == shardrail_session_core::TurnState::Idle
            || next_session.lifecycle != shardrail_protocol::SessionLifecycle::Open
        {
            return Err(CoordinatorError::InvalidTransition);
        }
        let dispatch_guard = dispatch_guard
            && next_session.process == shardrail_session_core::ProcessState::Alive
            && next_session.lifecycle == shardrail_protocol::SessionLifecycle::Open;
        next_session.turn_finished(outcome, dispatch_guard);
        self.transition_to(now_ms, "turn_finished", payload, next_session, effect)
    }

    fn transition_to<T>(
        &mut self,
        now_ms: i64,
        kind: &str,
        payload: serde_json::Value,
        next_session: Session,
        effect: impl FnOnce() -> io::Result<T>,
    ) -> Result<T, CoordinatorError> {
        let snapshot =
            serde_json::to_value(&next_session).expect("session serialization is infallible");
        let payload = match payload {
            serde_json::Value::Object(mut values) => {
                values.insert("session_snapshot".into(), snapshot);
                serde_json::Value::Object(values)
            }
            other => serde_json::json!({"data":other,"session_snapshot":snapshot}),
        };
        let journal_epoch = match self.journal.journal_epoch() {
            Ok(epoch) => epoch,
            Err(error) => {
                self.fail_closed();
                return Err(error.into());
            }
        };
        let host_id = match self.journal.host_id() {
            Ok(host_id) => host_id,
            Err(error) => {
                self.fail_closed();
                return Err(error.into());
            }
        };
        let event = DurableEvent {
            event_id: 0,
            journal_epoch,
            host_id,
            session_id: self.session_id,
            schema_version: SCHEMA_VERSION,
            kind: kind.to_owned(),
            occurred_at_ms: now_ms,
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload,
        };
        if let Err(error) = self.journal.append_event(&event) {
            self.fail_closed();
            return Err(error.into());
        }
        let withdrawal_reason = match kind {
            "turn_finished" => Some("turn_end"),
            "process_exited" => Some("process_lost"),
            _ => None,
        };
        if let Some(reason) = withdrawal_reason
            && let Err(error) =
                self.journal
                    .withdraw_pending_approvals(self.session_id, reason, now_ms)
        {
            self.fail_closed();
            return Err(error.into());
        }
        self.session = next_session;
        match effect() {
            Ok(result) => Ok(result),
            Err(error) => {
                self.record_provider_effect_unknown(now_ms);
                Err(error.into())
            }
        }
    }

    pub fn create_approval(&mut self, mut approval: Approval) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if !approval_turn_open(&self.session) {
            return Err(CoordinatorError::InvalidApproval);
        }
        if !approval.session_id.is_nil() && approval.session_id != self.session_id {
            return Err(CoordinatorError::InvalidApproval);
        }
        if approval.session_id.is_nil() {
            approval.session_id = self.session_id;
        }
        if approval.session_id != self.session_id {
            return Err(CoordinatorError::InvalidApproval);
        }
        if let Err(error) = self.journal.create_pending_approval(&approval) {
            self.fail_closed();
            return Err(error.into());
        }
        Ok(())
    }

    pub fn respond_approval<T>(
        &mut self,
        approval: &mut Approval,
        request: ApprovalResponseRequest<'_>,
        effect: impl FnOnce(&ApprovalDecision, Option<&str>) -> io::Result<T>,
    ) -> Result<T, CoordinatorError> {
        self.ensure_healthy()?;
        if !approval_turn_open(&self.session) {
            return Err(CoordinatorError::InvalidApproval);
        }
        if !approval.session_id.is_nil() && approval.session_id != self.session_id {
            return Err(CoordinatorError::InvalidApproval);
        }
        if !scope_allows(
            request.decision.clone(),
            request.option_class,
            request.client_scope,
        ) {
            return Err(CoordinatorError::InvalidApproval);
        }
        let mut candidate = approval.clone();
        if candidate.session_id.is_nil() {
            candidate.session_id = self.session_id;
        }
        if !candidate.respond(
            request.decision.clone(),
            request.authorized,
            request.now_ms,
            request.provider_limit_secs,
        ) {
            if candidate.needs_reconfirmation {
                if let Err(error) = self.journal.update_approval_confirmation(&candidate) {
                    self.fail_closed();
                    return Err(error.into());
                }
                *approval = candidate;
            }
            return Err(CoordinatorError::InvalidApproval);
        }
        let response = ApprovalResponding {
            approval_id: candidate.id,
            decision: candidate
                .decision
                .clone()
                .ok_or(CoordinatorError::InvalidApproval)?,
            option_id: request.option_id,
            responder: request.responder.to_owned(),
            responded_at_ms: request.now_ms,
            option_class: request.option_class,
            client_scope: request.client_scope,
            content_hash: candidate.content_hash.clone(),
            client_label: candidate.client_label.clone(),
        };
        self.write_response(approval, candidate, response, false, effect)
    }

    pub fn reconfirm_approval(
        &mut self,
        approval: &mut Approval,
        authorized: bool,
        workspace_unchanged: bool,
        now_ms: i64,
    ) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        let mut candidate = approval.clone();
        if !candidate.session_id.is_nil() && candidate.session_id != self.session_id {
            return Err(CoordinatorError::InvalidApproval);
        }
        if candidate.session_id.is_nil() {
            candidate.session_id = self.session_id;
        }
        if !candidate.reconfirm(authorized, workspace_unchanged, now_ms) {
            return Err(CoordinatorError::InvalidApproval);
        }
        if let Err(error) = self.journal.update_approval_confirmation(&candidate) {
            self.fail_closed();
            return Err(error.into());
        }
        *approval = candidate;
        Ok(())
    }

    pub fn hold_approval(
        &mut self,
        approval: &mut Approval,
        held: bool,
        now_ms: i64,
    ) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if (!approval.session_id.is_nil() && approval.session_id != self.session_id)
            || approval.state != shardrail_protocol::ApprovalState::Pending
        {
            return Err(CoordinatorError::InvalidApproval);
        }
        let mut candidate = approval.clone();
        if candidate.session_id.is_nil() {
            candidate.session_id = self.session_id;
        }
        candidate.set_held(held);
        candidate.confirmed_at_ms = now_ms;
        if let Err(error) = self.journal.update_approval_confirmation(&candidate) {
            if matches!(
                error,
                JournalError::AlreadyResponded | JournalError::InvalidApproval
            ) {
                return Err(CoordinatorError::InvalidApproval);
            }
            self.fail_closed();
            return Err(error.into());
        }
        *approval = candidate;
        Ok(())
    }

    fn write_response<T>(
        &mut self,
        approval: &mut Approval,
        candidate: Approval,
        response: ApprovalResponding,
        expired: bool,
        effect: impl FnOnce(&ApprovalDecision, Option<&str>) -> io::Result<T>,
    ) -> Result<T, CoordinatorError> {
        let journal_result = if expired {
            self.journal
                .begin_expired_approval_response(&response, self.session_id)
        } else {
            self.journal
                .begin_approval_response(&response, self.session_id)
        };
        if let Err(error) = journal_result {
            if matches!(
                error,
                JournalError::AlreadyResponded | JournalError::InvalidApproval
            ) {
                return Err(CoordinatorError::InvalidApproval);
            }
            self.fail_closed();
            return Err(error.into());
        }
        *approval = candidate;
        match effect(&response.decision, response.option_id.as_deref()) {
            Ok(result) => {
                if let Err(error) = self
                    .journal
                    .mark_response_sent(approval.id, response.responded_at_ms)
                {
                    self.fail_closed();
                    return Err(error.into());
                }
                Ok(result)
            }
            Err(error) => {
                self.record_provider_effect_unknown(response.responded_at_ms);
                Err(error.into())
            }
        }
    }

    pub fn timeout_approval<T>(
        &mut self,
        approval: &mut Approval,
        request: ApprovalTimeoutRequest,
        response_effect: impl FnOnce(&ApprovalDecision, Option<&str>) -> io::Result<T>,
        cancel_turn_effect: impl FnOnce() -> io::Result<()>,
    ) -> Result<T, CoordinatorError> {
        self.ensure_healthy()?;
        if !approval_turn_open(&self.session) {
            return Err(CoordinatorError::InvalidApproval);
        }
        let mut candidate = approval.clone();
        if !candidate.session_id.is_nil() && candidate.session_id != self.session_id {
            return Err(CoordinatorError::InvalidApproval);
        }
        if candidate
            .expire_if_due(request.now_ms, request.provider_limit_secs)
            .is_none()
        {
            return Err(CoordinatorError::InvalidApproval);
        }
        let cancel_turn = request.option_id.is_none();
        let response_decision = if !cancel_turn {
            ApprovalDecision::Deny
        } else {
            ApprovalDecision::Cancel
        };
        candidate.decision = Some(response_decision.clone());
        let response = ApprovalResponding {
            approval_id: candidate.id,
            decision: response_decision,
            option_id: request.option_id,
            responder: "timeout-policy".to_owned(),
            responded_at_ms: request.now_ms,
            option_class: OptionClass::Once,
            client_scope: ClientScope::Full,
            content_hash: candidate.content_hash.clone(),
            client_label: "daemon".to_owned(),
        };
        if cancel_turn {
            let mut next_session = self.session.clone();
            next_session.turn = shardrail_session_core::TurnState::Cancelling;
            self.transition_to(
                request.now_ms,
                "turn_cancel_requested",
                serde_json::json!({"reason":"approval_expired"}),
                next_session,
                || Ok(()),
            )?;
        }
        let response_result =
            self.write_response(approval, candidate, response, true, response_effect);
        if cancel_turn && let Err(error) = cancel_turn_effect() {
            self.record_provider_effect_unknown(request.now_ms);
            return Err(error.into());
        }
        response_result
    }

    pub fn acknowledge_approval(
        &mut self,
        approval_id: Uuid,
        now_ms: i64,
    ) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if let Err(error) = self
            .journal
            .acknowledge_approval(approval_id, self.session_id, now_ms)
        {
            if matches!(
                error,
                JournalError::AlreadyResponded
                    | JournalError::MissingSession
                    | JournalError::InvalidApproval
            ) {
                return Err(CoordinatorError::InvalidApproval);
            }
            self.fail_closed();
            return Err(error.into());
        }
        Ok(())
    }

    pub fn reject_approval_response(
        &mut self,
        approval_id: Uuid,
        now_ms: i64,
    ) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if let Err(error) =
            self.journal
                .reject_approval_response(approval_id, self.session_id, now_ms)
        {
            if matches!(
                error,
                JournalError::AlreadyResponded
                    | JournalError::MissingSession
                    | JournalError::InvalidApproval
            ) {
                return Err(CoordinatorError::InvalidApproval);
            }
            self.fail_closed();
            return Err(error.into());
        }
        Ok(())
    }

    fn ensure_healthy(&self) -> Result<(), CoordinatorError> {
        if self.healthy {
            Ok(())
        } else {
            Err(CoordinatorError::Unhealthy)
        }
    }

    fn fail_closed(&mut self) {
        let pending_approvals = self
            .journal
            .load_approvals(self.session_id)
            .map(|approvals| {
                approvals
                    .into_iter()
                    .filter(|approval| approval.state == shardrail_protocol::ApprovalState::Pending)
                    .map(|approval| approval.id)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(effect) = &mut self.fail_closed_effect {
            let _ = effect(FailClosedPlan {
                cancel_turn: matches!(
                    self.session.turn,
                    shardrail_session_core::TurnState::Running
                        | shardrail_session_core::TurnState::Stalled
                        | shardrail_session_core::TurnState::Cancelling
                ),
                pending_approvals,
            });
        }
        self.healthy = false;
        self.session.journal_failed();
    }

    fn record_provider_effect_unknown(&mut self, now_ms: i64) {
        self.session.mark_desynced();
        let (journal_epoch, host_id) = match (self.journal.journal_epoch(), self.journal.host_id())
        {
            (Ok(epoch), Ok(host_id)) => (epoch, host_id),
            _ => {
                self.fail_closed();
                return;
            }
        };
        let snapshot =
            serde_json::to_value(&self.session).expect("session serialization is infallible");
        let event = DurableEvent {
            event_id: 0,
            journal_epoch,
            host_id,
            session_id: self.session_id,
            schema_version: SCHEMA_VERSION,
            kind: "provider_effect_unknown".into(),
            occurred_at_ms: now_ms,
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload: serde_json::json!({"session_snapshot":snapshot}),
        };
        if self.journal.append_event(&event).is_err() {
            self.fail_closed();
        }
    }
}

fn replay_session(events: &[DurableEvent]) -> Result<Session, CoordinatorError> {
    let mut session = Session::default();
    for event in events {
        if let Some(snapshot) = event.payload.get("session_snapshot") {
            session = serde_json::from_value::<Session>(snapshot.clone())?;
            continue;
        }
        apply_transition(&mut session, &event.kind)
            .map_err(|_| CoordinatorError::InvalidTransition)?;
    }
    Ok(session)
}

fn recover_session(mut session: Session) -> Session {
    if session.process == shardrail_session_core::ProcessState::Alive {
        session.process = shardrail_session_core::ProcessState::DesyncedAlive;
    }
    if session.process == shardrail_session_core::ProcessState::Exited {
        session.turn = shardrail_session_core::TurnState::Idle;
    } else if session.turn != shardrail_session_core::TurnState::Idle {
        session.turn = shardrail_session_core::TurnState::Detached;
    }
    session
}

fn apply_transition(session: &mut Session, kind: &str) -> Result<(), ()> {
    match kind {
        "turn_started" => {
            if session.lifecycle != shardrail_protocol::SessionLifecycle::Open
                || session.turn != shardrail_session_core::TurnState::Idle
                || session.process != shardrail_session_core::ProcessState::Alive
            {
                return Err(());
            }
            session.turn = shardrail_session_core::TurnState::Running;
        }
        "process_started" => {
            if session.lifecycle != shardrail_protocol::SessionLifecycle::Open
                || session.process != shardrail_session_core::ProcessState::Exited
            {
                return Err(());
            }
            session.process = shardrail_session_core::ProcessState::Alive;
        }
        "turn_finished" => {
            if session.turn == shardrail_session_core::TurnState::Idle {
                return Err(());
            }
            session.turn_finished(
                shardrail_session_core::TurnOutcome::Provider,
                session.process == shardrail_session_core::ProcessState::Alive
                    && session.lifecycle == shardrail_protocol::SessionLifecycle::Open,
            );
        }
        "turn_cancel_requested" => {
            if !matches!(
                session.turn,
                shardrail_session_core::TurnState::Running
                    | shardrail_session_core::TurnState::Stalled
            ) || session.process != shardrail_session_core::ProcessState::Alive
            {
                return Err(());
            }
            session.turn = shardrail_session_core::TurnState::Cancelling;
        }
        "process_exited" => {
            session.process_exited();
        }
        "session_closed" | "session_closing" => {
            session.close().map_err(|_| ())?;
        }
        "approval_pending"
        | "approval_confirmation_updated"
        | "approval_responding"
        | "approval_responded_unacked"
        | "approval_delivered_unconfirmed"
        | "approval_withdrawn"
        | "allowed"
        | "denied"
        | "expired"
        | "withdrawn" => {}
        _ => return Err(()),
    }
    Ok(())
}

fn approval_turn_open(session: &Session) -> bool {
    session.lifecycle == shardrail_protocol::SessionLifecycle::Open
        && matches!(
            session.turn,
            shardrail_session_core::TurnState::Running
                | shardrail_session_core::TurnState::Stalled
                | shardrail_session_core::TurnState::Cancelling
        )
}

fn scope_allows(decision: ApprovalDecision, class: OptionClass, scope: ClientScope) -> bool {
    decision != ApprovalDecision::Allow || scope == ClientScope::Full || class == OptionClass::Once
}

#[cfg(test)]
mod tests {
    use super::*;
    use shardrail_journal::Journal;
    use shardrail_session_core::Approval;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    fn open_after_startup(path: impl AsRef<Path>, session_id: Uuid) -> Coordinator {
        let mut journal = Journal::open(path).unwrap();
        Coordinator::recover_journal(&mut journal).unwrap();
        let mut coordinator = Coordinator::open(journal, session_id).unwrap();
        if coordinator
            .journal
            .events_for_session(session_id)
            .unwrap()
            .is_empty()
        {
            coordinator
                .transition(0, "process_started", serde_json::json!({}), || Ok(()))
                .unwrap();
        }
        coordinator
    }

    #[test]
    fn private_directory_lock_hardens_permissions_and_releases_after_crash_like_drop() {
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o777)).unwrap();
        let daemon = DaemonDataDir::open(&data).unwrap();
        assert_eq!(
            fs::metadata(&data).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(data.join("daemon.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(matches!(
            DaemonDataDir::open(&data),
            Err(CoordinatorError::AlreadyRunning)
        ));
        drop(daemon);
        assert!(DaemonDataDir::open(&data).is_ok());

        let fresh_data = dir.path().join("fresh");
        let _fresh_daemon = DaemonDataDir::open(&fresh_data).unwrap();
        assert_eq!(
            fs::metadata(&fresh_data).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn directory_swap_after_open_keeps_operations_on_the_checked_descriptor() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let moved = dir.path().join("data-opened");
        let external = dir.path().join("external");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&external).unwrap();

        let opened = open_private_directory(&data).unwrap();
        let pinned_path = descriptor_path(&opened).unwrap().join("swap-probe");
        fs::rename(&data, &moved).unwrap();
        symlink(&external, &data).unwrap();
        fs::write(&pinned_path, b"pinned").unwrap();

        assert_eq!(fs::read(moved.join("swap-probe")).unwrap(), b"pinned");
        assert!(!external.join("swap-probe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_swap_between_mkdir_and_open_is_refused() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let data = dir.path().join("data");
        let moved = dir.path().join("data-created");
        let external = dir.path().join("external");
        fs::create_dir(&external).unwrap();

        let result = open_private_directory_with_hooks(
            &data,
            || {
                fs::rename(&data, &moved).unwrap();
                symlink(&external, &data).unwrap();
            },
            || {},
        );

        assert!(result.is_err());
        assert_eq!(
            fs::metadata(&external).unwrap().permissions().mode() & 0o777,
            0o755
        );
    }

    #[test]
    fn journal_commit_precedes_effect_and_restart_recovers_approval_unconfirmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let mut journal = Journal::open(&path).unwrap();
        Coordinator::recover_journal(&mut journal).unwrap();
        let mut coordinator = Coordinator::open(journal, id).unwrap();
        coordinator
            .transition(0, "process_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        coordinator
            .transition(0, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(1);
        let approval_id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        coordinator
            .respond_approval(
                &mut approval,
                ApprovalResponseRequest {
                    decision: ApprovalDecision::Allow,
                    authorized: true,
                    now_ms: 2,
                    provider_limit_secs: None,
                    option_id: Some("allow_once".into()),
                    responder: "client",
                    option_class: shardrail_protocol::OptionClass::Once,
                    client_scope: shardrail_protocol::ClientScope::Full,
                },
                |_, _| {
                    let reader = Journal::open(&path).unwrap();
                    assert_eq!(
                        reader.approval_state(approval_id).unwrap().as_deref(),
                        Some("responding")
                    );
                    Ok(())
                },
            )
            .unwrap();
        drop(coordinator);
        let recovered = open_after_startup(path, id);
        assert_eq!(
            recovered.approval_state(approval_id).unwrap().as_deref(),
            Some("delivered_unconfirmed")
        );
        assert!(recovered.journal_healthy());
    }

    #[test]
    fn core_queue_snapshot_replays_and_active_turn_is_detached_on_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        let mut next = coordinator.session().clone();
        next.turn = shardrail_session_core::TurnState::Running;
        next.process = shardrail_session_core::ProcessState::Alive;
        next.queue_instruction("queued while active".into())
            .unwrap();
        coordinator
            .transition_session(1, "turn_started", serde_json::json!({}), next, || Ok(()))
            .unwrap();
        let recovered = open_after_startup(path, session_id);
        assert_eq!(
            recovered.session().turn,
            shardrail_session_core::TurnState::Detached
        );
        assert_eq!(
            recovered.session().queue,
            shardrail_session_core::QueueState::Queued("queued while active".into())
        );
        assert_eq!(
            recovered.session().process,
            shardrail_session_core::ProcessState::DesyncedAlive
        );
    }

    #[test]
    fn turn_completion_keeps_provider_alive_for_later_turns() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        coordinator
            .transition(2, "turn_finished", serde_json::json!({}), || Ok(()))
            .unwrap();
        assert_eq!(
            coordinator.session().turn,
            shardrail_session_core::TurnState::Idle
        );
        assert_eq!(
            coordinator.session().process,
            shardrail_session_core::ProcessState::Alive
        );
        let recovered = Coordinator::open(Journal::open(path).unwrap(), session_id).unwrap();
        assert_eq!(
            recovered.session().process,
            shardrail_session_core::ProcessState::DesyncedAlive
        );
    }

    #[test]
    fn idle_alive_process_is_desynced_after_restart() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let _coordinator = open_after_startup(&path, id);
        let recovered = Coordinator::open(Journal::open(path).unwrap(), id).unwrap();
        assert_eq!(
            recovered.session().turn,
            shardrail_session_core::TurnState::Idle
        );
        assert_eq!(
            recovered.session().process,
            shardrail_session_core::ProcessState::DesyncedAlive
        );
    }

    #[test]
    fn opening_another_session_does_not_run_global_approval_recovery() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).unwrap();
        let approval_id = Uuid::now_v7();
        let mut pending = Approval::new(1);
        pending.id = approval_id;
        pending.session_id = Uuid::now_v7();
        journal.create_pending_approval(&pending).unwrap();
        let response = ApprovalResponding {
            approval_id,
            decision: ApprovalDecision::Allow,
            option_id: Some("allow_once".into()),
            responder: "client".into(),
            responded_at_ms: 2,
            option_class: shardrail_protocol::OptionClass::Once,
            client_scope: shardrail_protocol::ClientScope::Full,
            content_hash: String::new(),
            client_label: String::new(),
        };
        journal
            .begin_approval_response(&response, pending.session_id)
            .unwrap();
        journal.mark_response_sent(approval_id, 2).unwrap();
        let _session_b = Coordinator::open(journal, Uuid::now_v7()).unwrap();
        assert_eq!(
            Journal::open(&path)
                .unwrap()
                .approval_state(approval_id)
                .unwrap()
                .as_deref(),
            Some("responded_unacked")
        );
    }

    #[test]
    fn failed_provider_effect_marks_session_unhealthy_and_desynced() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        assert!(
            coordinator
                .transition(1, "turn_started", serde_json::json!({}), || {
                    Err::<(), _>(io::Error::other("partial provider write"))
                })
                .is_err()
        );
        assert!(coordinator.journal_healthy());
        assert_eq!(
            coordinator.session().turn,
            shardrail_session_core::TurnState::Detached
        );
        assert_eq!(
            coordinator.session().process,
            shardrail_session_core::ProcessState::DesyncedAlive
        );
        let events = Journal::open(&path)
            .unwrap()
            .events_for_session(session_id)
            .unwrap();
        assert_eq!(events.last().unwrap().kind, "provider_effect_unknown");
        let recovered = Coordinator::open(Journal::open(path).unwrap(), session_id).unwrap();
        assert_eq!(
            recovered.session().turn,
            shardrail_session_core::TurnState::Detached
        );
        let mut called = false;
        assert!(
            coordinator
                .transition(2, "turn_started", serde_json::json!({}), || {
                    called = true;
                    Ok(())
                })
                .is_err()
        );
        assert!(!called);
    }

    #[test]
    fn timed_out_cancel_acknowledges_as_expired_and_cancels_turn() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(0, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(0);
        let approval_id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        let cancel_called = std::cell::Cell::new(false);
        coordinator
            .timeout_approval(
                &mut approval,
                ApprovalTimeoutRequest {
                    now_ms: 180_000,
                    provider_limit_secs: None,
                    option_id: None,
                },
                |decision, option| {
                    assert_eq!(decision, &ApprovalDecision::Cancel);
                    assert_eq!(option, None);
                    Ok(())
                },
                || {
                    cancel_called.set(true);
                    Ok(())
                },
            )
            .unwrap();
        assert!(cancel_called.get());
        assert_eq!(
            coordinator.session().turn,
            shardrail_session_core::TurnState::Cancelling
        );
        coordinator.acknowledge_approval(approval_id, 3).unwrap();
        assert_eq!(
            coordinator.approval_state(approval_id).unwrap().as_deref(),
            Some("expired")
        );
    }

    #[test]
    fn failed_timeout_cancel_effect_is_durably_marked_unknown() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(0, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(0);
        coordinator.create_approval(approval.clone()).unwrap();
        let result = coordinator.timeout_approval(
            &mut approval,
            ApprovalTimeoutRequest {
                now_ms: 180_000,
                provider_limit_secs: None,
                option_id: None,
            },
            |_, _| Ok(()),
            || Err(io::Error::other("turn cancel write failed")),
        );
        assert!(result.is_err());
        assert!(coordinator.journal_healthy());
        let events = Journal::open(&path)
            .unwrap()
            .events_for_session(session_id)
            .unwrap();
        assert_eq!(events.last().unwrap().kind, "provider_effect_unknown");
        let recovered = Coordinator::open(Journal::open(path).unwrap(), session_id).unwrap();
        assert_eq!(
            recovered.session().turn,
            shardrail_session_core::TurnState::Detached
        );
    }

    #[test]
    fn provider_rejection_is_recorded_as_withdrawn_reason() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(0, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(1);
        let approval_id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        coordinator
            .respond_approval(
                &mut approval,
                ApprovalResponseRequest {
                    decision: ApprovalDecision::Allow,
                    authorized: true,
                    now_ms: 2,
                    provider_limit_secs: None,
                    option_id: Some("allow_once".into()),
                    responder: "client",
                    option_class: shardrail_protocol::OptionClass::Once,
                    client_scope: shardrail_protocol::ClientScope::Full,
                },
                |_, _| Ok(()),
            )
            .unwrap();
        coordinator
            .reject_approval_response(approval_id, 3)
            .unwrap();
        assert_eq!(
            coordinator.approval_state(approval_id).unwrap().as_deref(),
            Some("withdrawn")
        );
    }

    #[test]
    fn journal_rejects_symlink_without_changing_external_file_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempdir().unwrap();
        let target = dir.path().join("external.db");
        Journal::open(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.path().join("journal.sqlite3");
        symlink(&target, &link).unwrap();
        assert!(Journal::open(&link).is_err());
        assert_eq!(
            fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn expired_approval_without_reject_option_is_journaled_as_cancel() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(0, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(0);
        let approval_id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        coordinator
            .timeout_approval(
                &mut approval,
                ApprovalTimeoutRequest {
                    now_ms: 180_000,
                    provider_limit_secs: None,
                    option_id: None,
                },
                |decision, option| {
                    assert_eq!(decision, &ApprovalDecision::Cancel);
                    assert_eq!(option, None);
                    let journal = Journal::open(&path).unwrap();
                    assert_eq!(
                        journal.approval_state(approval_id).unwrap().as_deref(),
                        Some("responding")
                    );
                    Ok(())
                },
                || Ok(()),
            )
            .unwrap();
        assert_eq!(approval.state, shardrail_protocol::ApprovalState::Expired);
        assert_eq!(approval.decision, Some(ApprovalDecision::Cancel));
    }

    #[test]
    fn regression_stale_held_allow_requires_reconfirmation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(0, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(0);
        approval.set_held(true);
        coordinator.create_approval(approval.clone()).unwrap();
        let result = coordinator.respond_approval(
            &mut approval,
            ApprovalResponseRequest {
                decision: ApprovalDecision::Allow,
                authorized: true,
                now_ms: 1_200_000,
                provider_limit_secs: None,
                option_id: Some("once".into()),
                responder: "client",
                option_class: OptionClass::Once,
                client_scope: ClientScope::Full,
            },
            |_, _| Ok(()),
        );
        assert!(result.is_err());
        assert!(approval.needs_reconfirmation);
        let saved = Journal::open(&path)
            .unwrap()
            .load_approvals(approval.session_id)
            .unwrap();
        assert!(saved[0].needs_reconfirmation);
        assert_eq!(saved[0].state, shardrail_protocol::ApprovalState::Pending);
        coordinator
            .reconfirm_approval(&mut approval, true, true, 1_200_001)
            .unwrap();
        let reloaded = Journal::open(&path)
            .unwrap()
            .load_approvals(approval.session_id)
            .unwrap();
        assert!(!reloaded[0].needs_reconfirmation);
        assert_eq!(reloaded[0].confirmed_at_ms, 1_200_001);
    }

    #[test]
    fn regression_process_exit_interrupts_active_turn_and_allows_close() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        coordinator
            .transition(2, "process_exited", serde_json::json!({}), || Ok(()))
            .unwrap();
        assert_eq!(
            coordinator.session().turn,
            shardrail_session_core::TurnState::Idle
        );
        assert_eq!(
            coordinator.session().last_turn_outcome,
            Some(shardrail_session_core::TurnOutcome::Interrupted)
        );
        coordinator
            .transition(3, "session_closed", serde_json::json!({}), || Ok(()))
            .unwrap();
    }

    #[test]
    fn regression_exited_process_restart_recovers_turn_idle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, id);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        coordinator
            .transition(2, "process_exited", serde_json::json!({}), || Ok(()))
            .unwrap();
        drop(coordinator);
        let recovered = Coordinator::open(Journal::open(path).unwrap(), id).unwrap();
        assert_eq!(
            recovered.session().process,
            shardrail_session_core::ProcessState::Exited
        );
        assert_eq!(
            recovered.session().turn,
            shardrail_session_core::TurnState::Idle
        );
    }

    #[test]
    fn regression_turn_finish_advances_queued_instruction() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut next = coordinator.session().clone();
        next.queue_instruction("next task".into()).unwrap();
        coordinator
            .transition_session(2, "queue_instruction", serde_json::json!({}), next, || {
                Ok(())
            })
            .unwrap();
        coordinator
            .turn_finished(
                3,
                shardrail_session_core::TurnOutcome::Provider,
                true,
                serde_json::json!({}),
                || Ok(()),
            )
            .unwrap();
        assert_eq!(
            coordinator.session().queue,
            shardrail_session_core::QueueState::Dispatching("next task".into())
        );
    }

    #[test]
    fn queued_instruction_does_not_dispatch_after_interrupted_turn() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut next = coordinator.session().clone();
        next.queue_instruction("next task".into()).unwrap();
        coordinator
            .transition_session(2, "queue_instruction", serde_json::json!({}), next, || {
                Ok(())
            })
            .unwrap();
        coordinator
            .turn_finished(
                3,
                shardrail_session_core::TurnOutcome::Interrupted,
                true,
                serde_json::json!({}),
                || Ok(()),
            )
            .unwrap();
        assert_eq!(
            coordinator.session().queue,
            shardrail_session_core::QueueState::Queued("next task".into())
        );
    }

    #[test]
    fn regression_turn_start_does_not_revive_desynced_process() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        let mut next = coordinator.session().clone();
        next.mark_desynced();
        coordinator
            .transition_session(
                1,
                "process_desynced",
                serde_json::json!({}),
                next,
                || Ok(()),
            )
            .unwrap();
        assert!(
            coordinator
                .transition(2, "turn_started", serde_json::json!({}), || Ok(()))
                .is_err()
        );
        assert_eq!(
            coordinator.session().process,
            shardrail_session_core::ProcessState::DesyncedAlive
        );
    }

    #[test]
    fn regression_approval_events_are_session_scoped_and_reloadable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(2);
        approval.set_held(true);
        let id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        let journal = Journal::open(&path).unwrap();
        assert_eq!(
            journal
                .events_for_session(session_id)
                .unwrap()
                .last()
                .unwrap()
                .kind,
            "approval_pending"
        );
        let loaded = journal.load_approvals(session_id).unwrap();
        assert_eq!(loaded[0].id, id);
        assert_eq!(loaded[0].session_id, session_id);
        assert_eq!(loaded[0].created_at_ms, 2);
        assert!(loaded[0].held);
    }

    #[test]
    fn regression_approval_is_refused_while_turn_idle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        let approval = Approval::new(1);
        assert!(coordinator.create_approval(approval).is_err());
    }

    #[test]
    fn regression_approval_response_is_refused_after_turn_becomes_idle() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(2);
        coordinator.create_approval(approval.clone()).unwrap();
        coordinator
            .transition(3, "turn_finished", serde_json::json!({}), || Ok(()))
            .unwrap();
        assert!(
            coordinator
                .respond_approval(
                    &mut approval,
                    ApprovalResponseRequest {
                        decision: ApprovalDecision::Allow,
                        authorized: true,
                        now_ms: 4,
                        provider_limit_secs: None,
                        option_id: Some("once".into()),
                        responder: "client",
                        option_class: OptionClass::Once,
                        client_scope: ClientScope::Full,
                    },
                    |_, _| Ok(()),
                )
                .is_err()
        );
    }

    #[test]
    fn regression_limited_client_cannot_allow_widening_option() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(2);
        coordinator.create_approval(approval.clone()).unwrap();
        assert!(
            coordinator
                .respond_approval(
                    &mut approval,
                    ApprovalResponseRequest {
                        decision: ApprovalDecision::Allow,
                        authorized: true,
                        now_ms: 3,
                        provider_limit_secs: None,
                        option_id: Some("widen".into()),
                        responder: "limited",
                        option_class: OptionClass::Widening,
                        client_scope: ClientScope::Limited,
                    },
                    |_, _| Ok(()),
                )
                .is_err()
        );
        assert_eq!(
            coordinator.approval_state(approval.id).unwrap().as_deref(),
            Some("pending")
        );
    }

    #[test]
    fn regression_provider_effect_failure_is_recorded_as_desync_not_journal_failure() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(2);
        coordinator.create_approval(approval.clone()).unwrap();
        assert!(
            coordinator
                .respond_approval(
                    &mut approval,
                    ApprovalResponseRequest {
                        decision: ApprovalDecision::Allow,
                        authorized: true,
                        now_ms: 3,
                        provider_limit_secs: None,
                        option_id: Some("once".into()),
                        responder: "client",
                        option_class: OptionClass::Once,
                        client_scope: ClientScope::Full,
                    },
                    |_, _| Err::<(), _>(io::Error::other("pipe broken"))
                )
                .is_err()
        );
        assert!(coordinator.session.journal_available);
        assert_eq!(
            coordinator.session.process,
            shardrail_session_core::ProcessState::DesyncedAlive
        );
        let events = Journal::open(path)
            .unwrap()
            .events_for_session(session_id)
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.kind == "provider_effect_unknown")
        );
    }

    #[test]
    fn regression_rejected_close_is_not_journaled() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        assert!(
            coordinator
                .transition(2, "session_closed", serde_json::json!({}), || Ok(()))
                .is_err()
        );
        assert!(
            !Journal::open(path)
                .unwrap()
                .events_for_session(session_id)
                .unwrap()
                .iter()
                .any(|event| event.kind == "session_closed")
        );
    }

    #[test]
    fn regression_closed_session_rejects_process_and_turn_start() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator = open_after_startup(&path, Uuid::now_v7());
        coordinator
            .transition(1, "session_closed", serde_json::json!({}), || Ok(()))
            .unwrap();
        assert!(
            coordinator
                .transition(2, "process_started", serde_json::json!({}), || Ok(()))
                .is_err()
        );
        assert!(
            coordinator
                .transition(3, "turn_started", serde_json::json!({}), || Ok(()))
                .is_err()
        );
        assert_eq!(
            coordinator.session().process,
            shardrail_session_core::ProcessState::Alive
        );
        assert!(
            Journal::open(path)
                .unwrap()
                .events_for_session(coordinator.session_id)
                .unwrap()
                .iter()
                .any(|event| event.kind == "session_closing")
        );
        coordinator
            .transition(4, "process_exited", serde_json::json!({}), || Ok(()))
            .unwrap();
        assert_eq!(
            coordinator.session().lifecycle,
            shardrail_protocol::SessionLifecycle::Closed
        );
    }

    #[test]
    fn regression_turn_and_cancel_transitions_require_live_state_and_known_kinds() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut coordinator =
            Coordinator::open(Journal::open(&path).unwrap(), Uuid::now_v7()).unwrap();
        assert!(matches!(
            coordinator.transition(1, "turn_started", serde_json::json!({}), || Ok(())),
            Err(CoordinatorError::InvalidTransition)
        ));
        assert!(matches!(
            coordinator.transition(1, "turn_cancel_requested", serde_json::json!({}), || Ok(())),
            Err(CoordinatorError::InvalidTransition)
        ));
        assert!(matches!(
            coordinator.transition(1, "misspelled_transition", serde_json::json!({}), || Ok(())),
            Err(CoordinatorError::InvalidTransition)
        ));
    }

    #[test]
    fn regression_pending_approvals_withdraw_when_turn_ends_or_process_exits() {
        for ending in ["turn_finished", "process_exited"] {
            let dir = tempdir().unwrap();
            let path = dir.path().join("journal.db");
            let session = Uuid::now_v7();
            let mut coordinator = open_after_startup(&path, session);
            coordinator
                .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
                .unwrap();
            let approval = Approval::new(2);
            let id = approval.id;
            coordinator.create_approval(approval.clone()).unwrap();
            coordinator
                .transition(3, ending, serde_json::json!({}), || Ok(()))
                .unwrap();
            assert_eq!(
                coordinator.approval_state(id).unwrap().as_deref(),
                Some("withdrawn")
            );
            assert!(matches!(
                coordinator.timeout_approval(
                    &mut Approval {
                        session_id: session,
                        ..approval
                    },
                    ApprovalTimeoutRequest {
                        now_ms: 200_000,
                        provider_limit_secs: None,
                        option_id: None
                    },
                    |_, _| Ok(()),
                    || Ok(())
                ),
                Err(CoordinatorError::InvalidApproval)
            ));
        }
    }

    #[test]
    fn regression_unknown_ack_does_not_fail_close_and_foreign_approval_is_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session);
        assert!(matches!(
            coordinator.acknowledge_approval(Uuid::now_v7(), 4),
            Err(CoordinatorError::InvalidApproval)
        ));
        assert!(coordinator.journal_healthy());
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut foreign = Approval::new(2);
        foreign.session_id = Uuid::now_v7();
        assert!(matches!(
            coordinator.respond_approval(
                &mut foreign,
                ApprovalResponseRequest {
                    decision: ApprovalDecision::Allow,
                    authorized: true,
                    now_ms: 3,
                    provider_limit_secs: None,
                    option_id: Some("once".into()),
                    responder: "client",
                    option_class: OptionClass::Once,
                    client_scope: ClientScope::Full,
                },
                |_, _| Ok(())
            ),
            Err(CoordinatorError::InvalidApproval)
        ));
    }

    #[test]
    fn regression_unknown_response_does_not_fail_close() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut unknown = Approval::new(2);
        unknown.session_id = session;

        assert!(matches!(
            coordinator.respond_approval(
                &mut unknown,
                ApprovalResponseRequest {
                    decision: ApprovalDecision::Allow,
                    authorized: true,
                    now_ms: 3,
                    provider_limit_secs: None,
                    option_id: Some("once".into()),
                    responder: "client",
                    option_class: OptionClass::Once,
                    client_scope: ClientScope::Full,
                },
                |_, _| Ok(())
            ),
            Err(CoordinatorError::InvalidApproval)
        ));
        assert!(coordinator.journal_healthy());
    }

    #[test]
    fn regression_hold_choice_is_persisted_and_timeout_cancel_matches_memory() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(2);
        let id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        coordinator.hold_approval(&mut approval, true, 3).unwrap();
        assert!(
            Journal::open(&path)
                .unwrap()
                .load_approvals(session)
                .unwrap()[0]
                .held
        );
        coordinator
            .timeout_approval(
                &mut approval,
                ApprovalTimeoutRequest {
                    now_ms: 7_200_002,
                    provider_limit_secs: None,
                    option_id: None,
                },
                |_, _| Ok(()),
                || Ok(()),
            )
            .unwrap();
        assert_eq!(approval.decision, Some(ApprovalDecision::Cancel));
        coordinator.acknowledge_approval(id, 7_200_003).unwrap();
    }

    #[cfg(feature = "test-utils")]
    #[test]
    fn regression_journal_failure_invokes_protocol_fail_closed_hook() {
        use std::sync::{Arc, Mutex};
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let session_id = Uuid::now_v7();
        let mut coordinator = open_after_startup(&path, session_id);
        coordinator
            .transition(1, "turn_started", serde_json::json!({}), || Ok(()))
            .unwrap();
        let mut approval = Approval::new(2);
        coordinator.create_approval(approval.clone()).unwrap();
        let seen = Arc::new(Mutex::new(None));
        let hook_seen = seen.clone();
        coordinator.set_fail_closed_effect(move |plan| {
            *hook_seen.lock().unwrap() = Some(plan);
            Ok(())
        });
        coordinator.fail_approval_writes_for_test().unwrap();
        assert!(
            coordinator
                .respond_approval(
                    &mut approval,
                    ApprovalResponseRequest {
                        decision: ApprovalDecision::Allow,
                        authorized: true,
                        now_ms: 3,
                        provider_limit_secs: None,
                        option_id: Some("once".into()),
                        responder: "client",
                        option_class: OptionClass::Once,
                        client_scope: ClientScope::Full,
                    },
                    |_, _| Ok(())
                )
                .is_err()
        );
        let plan = seen.lock().unwrap().clone().unwrap();
        assert!(plan.cancel_turn);
        assert_eq!(plan.pending_approvals, [approval.id]);
    }
}
