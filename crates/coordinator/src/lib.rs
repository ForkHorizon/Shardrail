use shardrail_journal::{Journal, JournalError};
use shardrail_protocol::{ApprovalDecision, ApprovalResponding, DurableEvent};
use shardrail_session_core::{Approval, Session};
use std::fs::{self, File, OpenOptions};
use std::io;
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
    pub held: bool,
    pub option_id: Option<String>,
    pub responder: &'a str,
}

pub struct ApprovalTimeoutRequest {
    pub now_ms: i64,
    pub provider_limit_secs: Option<u64>,
    pub held: bool,
    pub option_id: Option<String>,
}

/// Owns M01's private data directory and process-lifetime advisory lock.
pub struct DaemonDataDir {
    path: PathBuf,
    _lock: File,
}

impl DaemonDataDir {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, CoordinatorError> {
        let path = path.as_ref();
        create_private_dir(path)?;
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(CoordinatorError::InvalidDataDirectory);
        }
        set_private_dir_mode(path)?;

        let lock_path = path.join("daemon.lock");
        if fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(CoordinatorError::InvalidDataDirectory);
        }
        let lock = open_private_file(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => Ok(Self {
                path: path.to_path_buf(),
                _lock: lock,
            }),
            Err(std::fs::TryLockError::WouldBlock) => Err(CoordinatorError::AlreadyRunning),
            Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub fn journal_path(&self) -> PathBuf {
        self.path.join("journal.sqlite3")
    }

    pub fn open_journal(&self) -> Result<Journal, JournalError> {
        Journal::open(self.journal_path())
    }
}

/// Provider effects are invoked only after their event has committed.
pub struct Coordinator {
    journal: Journal,
    session_id: Uuid,
    session: Session,
    healthy: bool,
}

impl Coordinator {
    /// Runs once after daemon startup and before opening session coordinators.
    pub fn recover_journal(journal: &mut Journal) -> Result<(), CoordinatorError> {
        journal.recover_responding()?;
        journal.recover_pending_approvals()?;
        Ok(())
    }

    pub fn open(journal: Journal, session_id: Uuid) -> Result<Self, CoordinatorError> {
        let events = journal.events_for_session(session_id)?;
        let session = recover_session(replay_session(&events)?);
        Ok(Self {
            journal,
            session_id,
            session,
            healthy: true,
        })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn journal_healthy(&self) -> bool {
        self.healthy
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
        apply_transition(&mut next_session, kind);
        self.transition_to(now_ms, kind, payload, next_session, effect)
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
        let event = DurableEvent {
            event_id: 0,
            session_id: self.session_id,
            schema_version: SCHEMA_VERSION,
            kind: kind.to_owned(),
            occurred_at_ms: now_ms,
            payload,
        };
        if let Err(error) = self.journal.append_event(&event) {
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

    pub fn create_approval(&mut self, approval: Approval) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if let Err(error) = self.journal.create_pending_approval(approval.id) {
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
        let mut candidate = approval.clone();
        if !candidate.respond(
            request.decision,
            request.authorized,
            request.now_ms,
            request.provider_limit_secs,
            request.held,
        ) {
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
        };
        self.write_response(approval, candidate, response, false, effect)
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
            self.journal.begin_expired_approval_response(&response)
        } else {
            self.journal.begin_approval_response(&response)
        };
        if let Err(error) = journal_result {
            self.fail_closed();
            return Err(error.into());
        }
        *approval = candidate;
        match effect(&response.decision, response.option_id.as_deref()) {
            Ok(result) => {
                if let Err(error) = self.journal.mark_response_sent(approval.id) {
                    self.fail_closed();
                    return Err(error.into());
                }
                Ok(result)
            }
            Err(error) => {
                self.fail_closed();
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
        let mut candidate = approval.clone();
        if candidate
            .expire_if_due(request.now_ms, request.provider_limit_secs, request.held)
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
        let response = ApprovalResponding {
            approval_id: candidate.id,
            decision: response_decision,
            option_id: request.option_id,
            responder: "timeout-policy".to_owned(),
            responded_at_ms: request.now_ms,
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
        let terminal = cancel_turn;
        let response_result =
            self.write_response(approval, candidate, response, terminal, response_effect);
        if cancel_turn && let Err(error) = cancel_turn_effect() {
            self.record_provider_effect_unknown(request.now_ms);
            return Err(error.into());
        }
        response_result
    }

    pub fn acknowledge_approval(&mut self, approval_id: Uuid) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if let Err(error) = self.journal.acknowledge_approval(approval_id) {
            self.fail_closed();
            return Err(error.into());
        }
        Ok(())
    }

    pub fn reject_approval_response(&mut self, approval_id: Uuid) -> Result<(), CoordinatorError> {
        self.ensure_healthy()?;
        if let Err(error) = self.journal.reject_approval_response(approval_id) {
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
        self.healthy = false;
        self.session.journal_failed();
    }

    fn record_provider_effect_unknown(&mut self, now_ms: i64) {
        self.session.mark_desynced();
        self.healthy = false;
        let snapshot =
            serde_json::to_value(&self.session).expect("session serialization is infallible");
        let event = DurableEvent {
            event_id: 0,
            session_id: self.session_id,
            schema_version: SCHEMA_VERSION,
            kind: "provider_effect_unknown".into(),
            occurred_at_ms: now_ms,
            payload: serde_json::json!({"session_snapshot":snapshot}),
        };
        let _ = self.journal.append_event(&event);
    }
}

fn replay_session(events: &[DurableEvent]) -> Result<Session, CoordinatorError> {
    let mut session = Session::default();
    for event in events {
        if let Some(snapshot) = event.payload.get("session_snapshot") {
            session = serde_json::from_value::<Session>(snapshot.clone())?;
            continue;
        }
        apply_transition(&mut session, &event.kind);
    }
    Ok(session)
}

fn recover_session(mut session: Session) -> Session {
    if session.process == shardrail_session_core::ProcessState::Alive {
        session.process = shardrail_session_core::ProcessState::DesyncedAlive;
    }
    if session.turn != shardrail_session_core::TurnState::Idle {
        session.turn = shardrail_session_core::TurnState::Detached;
    }
    session
}

fn apply_transition(session: &mut Session, kind: &str) {
    match kind {
        "turn_started" => {
            session.turn = shardrail_session_core::TurnState::Running;
            session.process = shardrail_session_core::ProcessState::Alive;
        }
        "process_started" => {
            session.process = shardrail_session_core::ProcessState::Alive;
        }
        "turn_finished" => {
            session.turn = shardrail_session_core::TurnState::Idle;
        }
        "turn_cancel_requested" => {
            session.turn = shardrail_session_core::TurnState::Cancelling;
        }
        "process_exited" => {
            session.process_exited();
        }
        "session_closed" => {
            let _ = session.close();
        }
        _ => {}
    }
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn set_private_dir_mode(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir_mode(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn open_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

#[cfg(not(unix))]
fn open_private_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
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
        Coordinator::open(journal, session_id).unwrap()
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

    #[test]
    fn journal_commit_precedes_effect_and_restart_recovers_approval_unconfirmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let mut journal = Journal::open(&path).unwrap();
        Coordinator::recover_journal(&mut journal).unwrap();
        let mut coordinator = Coordinator::open(journal, id).unwrap();
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
                    held: false,
                    option_id: Some("allow_once".into()),
                    responder: "client",
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
        let mut coordinator = open_after_startup(&path, id);
        coordinator
            .transition(1, "process_started", serde_json::json!({}), || Ok(()))
            .unwrap();
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
        journal.create_pending_approval(approval_id).unwrap();
        let response = ApprovalResponding {
            approval_id,
            decision: ApprovalDecision::Allow,
            option_id: Some("allow_once".into()),
            responder: "client".into(),
            responded_at_ms: 2,
        };
        journal.begin_approval_response(&response).unwrap();
        journal.mark_response_sent(approval_id).unwrap();
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
        assert!(!coordinator.journal_healthy());
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
                    held: false,
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
        coordinator.acknowledge_approval(approval_id).unwrap();
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
                held: false,
                option_id: None,
            },
            |_, _| Ok(()),
            || Err(io::Error::other("turn cancel write failed")),
        );
        assert!(result.is_err());
        assert!(!coordinator.journal_healthy());
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
                    held: false,
                    option_id: Some("allow_once".into()),
                    responder: "client",
                },
                |_, _| Ok(()),
            )
            .unwrap();
        coordinator.reject_approval_response(approval_id).unwrap();
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
        let mut approval = Approval::new(0);
        let approval_id = approval.id;
        coordinator.create_approval(approval.clone()).unwrap();
        coordinator
            .timeout_approval(
                &mut approval,
                ApprovalTimeoutRequest {
                    now_ms: 180_000,
                    provider_limit_secs: None,
                    held: false,
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
        assert_eq!(approval.decision, Some(ApprovalDecision::Deny));
    }
}
