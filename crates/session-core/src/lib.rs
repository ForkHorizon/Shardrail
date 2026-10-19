use shardrail_protocol::{ApprovalDecision, ApprovalState, SessionLifecycle};
use std::time::Duration;
use uuid::Uuid;

pub const APPROVAL_DENY_AFTER: Duration = Duration::from_secs(180);
pub const APPROVAL_MAX_HOLD: Duration = Duration::from_secs(2 * 60 * 60);
pub const APPROVAL_RECONFIRM_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Approval {
    pub id: Uuid,
    pub state: ApprovalState,
    pub created_at_ms: i64,
    pub confirmed_at_ms: i64,
    pub needs_reconfirmation: bool,
    pub decision: Option<ApprovalDecision>,
}

impl Approval {
    pub fn new(created_at_ms: i64) -> Self {
        Self {
            id: Uuid::now_v7(),
            state: ApprovalState::Pending,
            created_at_ms,
            confirmed_at_ms: created_at_ms,
            needs_reconfirmation: false,
            decision: None,
        }
    }

    pub fn respond(
        &mut self,
        decision: ApprovalDecision,
        authorized: bool,
        now_ms: i64,
        provider_limit_secs: Option<u64>,
        held: bool,
    ) -> bool {
        if self
            .expire_if_due(now_ms, provider_limit_secs, held)
            .is_some()
        {
            return false;
        }
        if self.state != ApprovalState::Pending
            || !authorized
            || (self.needs_reconfirmation && decision == ApprovalDecision::Allow)
        {
            return false;
        }
        self.state = ApprovalState::Responding;
        self.decision = Some(decision);
        self.confirmed_at_ms = now_ms;
        true
    }

    pub fn timeout(&mut self) -> Option<ApprovalDecision> {
        if self.state != ApprovalState::Pending {
            return None;
        }
        self.state = ApprovalState::Expired;
        self.decision = Some(ApprovalDecision::Deny);
        self.decision.clone()
    }

    pub fn disconnect(&mut self) {
        // A client disconnect does not change provider or response delivery state.
    }

    pub fn expire_if_due(
        &mut self,
        now_ms: i64,
        provider_limit_secs: Option<u64>,
        held: bool,
    ) -> Option<ApprovalDecision> {
        let (start, limit) = if held {
            (
                self.created_at_ms,
                self.provider_limit_secs(provider_limit_secs)
                    .min(APPROVAL_MAX_HOLD)
                    .as_millis() as i64,
            )
        } else {
            (
                self.confirmed_at_ms,
                self.provider_limit_secs(provider_limit_secs).as_millis() as i64,
            )
        };
        (now_ms.saturating_sub(start) >= limit)
            .then(|| self.timeout())
            .flatten()
    }

    pub fn workspace_changed(&mut self) {
        if self.state == ApprovalState::Pending {
            self.needs_reconfirmation = true;
        }
    }

    pub fn reconfirm(&mut self, authorized: bool, workspace_unchanged: bool, now_ms: i64) -> bool {
        if self.state != ApprovalState::Pending || !authorized || !workspace_unchanged {
            return false;
        }
        self.needs_reconfirmation = false;
        self.confirmed_at_ms = now_ms;
        true
    }
    pub fn mark_stale(&mut self, now_ms: i64) {
        if self.state == ApprovalState::Pending
            && now_ms.saturating_sub(self.confirmed_at_ms)
                >= APPROVAL_RECONFIRM_AFTER.as_millis() as i64
        {
            self.needs_reconfirmation = true;
        }
    }
    pub fn provider_limit_secs(&self, limit_secs: Option<u64>) -> Duration {
        Duration::from_secs(limit_secs.map_or(APPROVAL_DENY_AFTER.as_secs(), |limit| {
            limit.min(APPROVAL_DENY_AFTER.as_secs())
        }))
    }
}

pub fn respond_write_ahead<E>(
    write_journal: impl FnOnce() -> Result<(), E>,
    write_provider: impl FnOnce() -> Result<(), E>,
) -> Result<(), E> {
    write_journal()?;
    write_provider()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Provider,
    Interrupted,
    KilledMidTool,
    UnknownAfterRestart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueState {
    Empty,
    Queued(String),
    Dispatching(String),
    Sent,
    Discarded,
    Edited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Idle,
    Running,
    Stalled,
    Cancelling,
    Detached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Alive,
    Exited,
    DesyncedAlive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub lifecycle: SessionLifecycle,
    pub turn: TurnState,
    pub process: ProcessState,
    pub queue: QueueState,
    pub journal_available: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            lifecycle: SessionLifecycle::Open,
            turn: TurnState::Idle,
            process: ProcessState::Exited,
            queue: QueueState::Empty,
            journal_available: true,
        }
    }
}

impl Session {
    pub fn queue_instruction(&mut self, text: String) -> Result<(), &'static str> {
        if !self.journal_available {
            return Err("journal unavailable");
        }
        if self.turn == TurnState::Idle {
            return Err("session is idle; dispatch as a command");
        }
        if matches!(self.queue, QueueState::Queued(_)) {
            return Err("one instruction is already queued");
        }
        self.queue = QueueState::Queued(text);
        Ok(())
    }
    pub fn turn_finished(&mut self, outcome: TurnOutcome, dispatch_guard: bool) -> Option<String> {
        self.turn = TurnState::Idle;
        match (&self.queue, outcome, dispatch_guard, self.journal_available) {
            (QueueState::Queued(text), TurnOutcome::Provider, true, true) => {
                let text = text.clone();
                self.queue = QueueState::Dispatching(text.clone());
                Some(text)
            }
            _ => None,
        }
    }
    pub fn journal_failed(&mut self) {
        self.journal_available = false;
        if matches!(self.turn, TurnState::Running | TurnState::Stalled)
            && self.process == ProcessState::Alive
        {
            self.turn = TurnState::Cancelling;
        }
    }
    pub fn mark_desynced(&mut self) {
        self.process = ProcessState::DesyncedAlive;
        self.turn = TurnState::Detached;
    }
    pub fn request_automatic_kill(&self) -> bool {
        self.process != ProcessState::DesyncedAlive
    }
    pub fn close(&mut self) -> Result<(), &'static str> {
        if self.lifecycle != SessionLifecycle::Open {
            return Err("session is not open");
        }
        if self.turn != TurnState::Idle {
            return Err("cancel the active turn first");
        }
        self.queue = QueueState::Discarded;
        self.lifecycle = if self.process == ProcessState::Exited {
            SessionLifecycle::Closed
        } else {
            SessionLifecycle::Closing
        };
        Ok(())
    }
    pub fn process_exited(&mut self) {
        self.process = ProcessState::Exited;
        if self.lifecycle == SessionLifecycle::Closing {
            self.lifecycle = SessionLifecycle::Closed;
        }
    }
    pub fn purge(&mut self) -> Result<(), &'static str> {
        if self.lifecycle != SessionLifecycle::Closed {
            return Err("only closed sessions can be purged");
        }
        self.lifecycle = SessionLifecycle::Purging;
        Ok(())
    }

    pub fn purge_finished(&mut self) -> Result<(), &'static str> {
        if self.lifecycle != SessionLifecycle::Purging {
            return Err("session is not being purged");
        }
        self.lifecycle = SessionLifecycle::Purged;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn approval_defaults_and_timeouts_never_allow() {
        let mut approval = Approval::new(0);
        assert_eq!(approval.decision, None);
        assert_eq!(approval.timeout(), Some(ApprovalDecision::Deny));
        assert!(!approval.respond(ApprovalDecision::Allow, true, 1, None, false));
        assert_eq!(
            approval.provider_limit_secs(Some(10)),
            Duration::from_secs(10)
        );
        assert_eq!(approval.provider_limit_secs(None), Duration::from_secs(180));
        let mut disconnected = Approval::new(0);
        disconnected.disconnect();
        assert_eq!(disconnected.state, ApprovalState::Pending);
        assert_eq!(disconnected.decision, None);
        assert!(disconnected.respond(ApprovalDecision::Allow, true, 1, None, false));
    }

    #[test]
    fn stale_or_changed_workspace_requires_reconfirmation() {
        let mut approval = Approval::new(0);
        approval.mark_stale(APPROVAL_RECONFIRM_AFTER.as_millis() as i64);
        assert!(!approval.respond(ApprovalDecision::Allow, true, 1, None, false));
        let mut changed = Approval::new(0);
        changed.workspace_changed();
        assert!(!changed.respond(ApprovalDecision::Allow, true, 1, None, false));
        assert!(changed.reconfirm(true, true, 2));
        assert!(changed.respond(ApprovalDecision::Allow, true, 3, None, false));
    }

    #[test]
    fn approval_responses_expire_atomically_at_the_deadline() {
        let mut provider_limited = Approval::new(0);
        assert!(!provider_limited.respond(ApprovalDecision::Allow, true, 10_000, Some(10), false));
        assert_eq!(provider_limited.state, ApprovalState::Expired);
        assert_eq!(provider_limited.decision, Some(ApprovalDecision::Deny));

        let mut default_limit = Approval::new(0);
        assert!(!default_limit.respond(ApprovalDecision::Allow, true, 180_001, None, false));
        assert_eq!(default_limit.state, ApprovalState::Expired);
        assert_eq!(default_limit.decision, Some(ApprovalDecision::Deny));

        let mut held = Approval::new(0);
        assert!(!held.respond(ApprovalDecision::Allow, true, 3_600_000, Some(3600), true));
        assert_eq!(held.decision, Some(ApprovalDecision::Deny));
    }

    #[test]
    fn client_disconnect_does_not_change_response_delivery_state() {
        for state in [
            ApprovalState::Responding,
            ApprovalState::RespondedUnacked,
            ApprovalState::DeliveredUnconfirmed,
        ] {
            let mut approval = Approval::new(0);
            approval.state = state.clone();
            approval.decision = Some(ApprovalDecision::Allow);
            approval.disconnect();
            assert_eq!(approval.state, state);
            assert_eq!(approval.decision, Some(ApprovalDecision::Allow));
        }
    }

    #[test]
    fn approval_expires_at_the_provider_limit_or_two_hour_hold_limit() {
        let mut prompt = Approval::new(0);
        assert_eq!(prompt.expire_if_due(9_999, Some(10), false), None);
        assert_eq!(
            prompt.expire_if_due(10_000, Some(10), false),
            Some(ApprovalDecision::Deny)
        );
        let mut held = Approval::new(0);
        assert_eq!(
            held.expire_if_due(7_200_000, None, true),
            Some(ApprovalDecision::Deny)
        );
    }

    #[test]
    fn journal_write_precedes_provider_send() {
        let steps = std::cell::RefCell::new(Vec::new());
        let result = respond_write_ahead(
            || {
                steps.borrow_mut().push("journal");
                Ok::<_, ()>(())
            },
            || {
                steps.borrow_mut().push("provider");
                Err(())
            },
        );
        assert_eq!(result, Err(()));
        assert_eq!(*steps.borrow(), ["journal", "provider"]);

        steps.borrow_mut().clear();
        let result = respond_write_ahead(
            || {
                steps.borrow_mut().push("journal");
                Err::<(), _>(())
            },
            || {
                steps.borrow_mut().push("provider");
                Ok(())
            },
        );
        assert_eq!(result, Err(()));
        assert_eq!(*steps.borrow(), ["journal"]);
    }

    #[test]
    fn queue_waits_for_normal_turn_and_session_close_requires_exit_before_purge() {
        let mut session = Session {
            turn: TurnState::Running,
            process: ProcessState::Alive,
            ..Default::default()
        };
        session.queue_instruction("next".into()).unwrap();
        assert_eq!(session.turn_finished(TurnOutcome::Interrupted, true), None);
        session.close().unwrap();
        assert!(session.purge().is_err());
        session.process_exited();
        assert_eq!(session.lifecycle, SessionLifecycle::Closed);
        session.purge().unwrap();
        session.purge_finished().unwrap();
        assert_eq!(session.lifecycle, SessionLifecycle::Purged);
    }

    #[test]
    fn closing_an_already_exited_session_closes_it_immediately() {
        let mut session = Session::default();
        session.close().unwrap();
        assert_eq!(session.lifecycle, SessionLifecycle::Closed);
        session.purge().unwrap();
    }

    #[test]
    fn desynced_process_is_not_auto_killed_and_journal_failure_blocks_dispatch() {
        let mut session = Session {
            turn: TurnState::Running,
            process: ProcessState::Alive,
            ..Default::default()
        };
        session.mark_desynced();
        assert!(!session.request_automatic_kill());
        assert!(session.queue_instruction("wait".into()).is_ok());
        session.journal_failed();
        assert!(session.queue_instruction("blocked".into()).is_err());
    }

    #[test]
    fn journal_failure_only_cancels_an_active_live_turn() {
        let mut idle = Session::default();
        idle.journal_failed();
        assert_eq!(idle.turn, TurnState::Idle);
        assert_eq!(idle.process, ProcessState::Exited);
        assert!(!idle.journal_available);

        let mut running = Session {
            turn: TurnState::Running,
            process: ProcessState::Alive,
            ..Default::default()
        };
        running.journal_failed();
        assert_eq!(running.turn, TurnState::Cancelling);
    }

    proptest! {
        #[test]
        fn timeouts_are_never_allow(age in 0u64..100_000) {
            let mut approval = Approval::new(0);
            if age >= 180 { prop_assert_eq!(approval.timeout(), Some(ApprovalDecision::Deny)); }
            prop_assert_ne!(approval.decision, Some(ApprovalDecision::Allow));
        }
    }
}
