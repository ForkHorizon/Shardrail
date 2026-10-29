use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DrainStatus {
    #[default]
    Active,
    Draining {
        started_at_ms: i64,
        grace_timeout_ms: u64,
        reason: String,
    },
    Completed {
        completed_at_ms: i64,
    },
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrainBlocker {
    pub session_id: Uuid,
    pub reason: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DrainError {
    #[error("daemon is draining for upgrade: {0}")]
    Draining(String),
}

#[derive(Debug, Default)]
pub struct DrainCoordinator {
    status: Mutex<DrainStatus>,
}

#[allow(dead_code)]
impl DrainCoordinator {
    pub fn new() -> Self {
        Self {
            status: Mutex::new(DrainStatus::Active),
        }
    }

    pub fn is_draining(&self) -> bool {
        let guard = self.status.lock().expect("drain status lock poisoned");
        matches!(*guard, DrainStatus::Draining { .. })
    }

    pub fn start_drain(&self, grace_timeout_ms: u64, reason: &str, now_ms: i64) {
        let mut guard = self.status.lock().expect("drain status lock poisoned");
        if matches!(*guard, DrainStatus::Active) {
            *guard = DrainStatus::Draining {
                started_at_ms: now_ms,
                grace_timeout_ms,
                reason: reason.to_string(),
            };
        }
    }

    pub fn complete_drain(&self, now_ms: i64) {
        let mut guard = self.status.lock().expect("drain status lock poisoned");
        *guard = DrainStatus::Completed {
            completed_at_ms: now_ms,
        };
    }

    pub fn can_accept_new_session(&self) -> Result<(), DrainError> {
        let guard = self.status.lock().expect("drain status lock poisoned");
        if let DrainStatus::Draining { ref reason, .. } = *guard {
            return Err(DrainError::Draining(format!(
                "new sessions are rejected ({reason})"
            )));
        }
        if let DrainStatus::Completed { .. } = *guard {
            return Err(DrainError::Draining("daemon is shutting down".into()));
        }
        Ok(())
    }

    pub fn can_accept_new_turn(&self) -> Result<(), DrainError> {
        let guard = self.status.lock().expect("drain status lock poisoned");
        if let DrainStatus::Draining { ref reason, .. } = *guard {
            return Err(DrainError::Draining(format!(
                "new turns are rejected ({reason})"
            )));
        }
        if let DrainStatus::Completed { .. } = *guard {
            return Err(DrainError::Draining("daemon is shutting down".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drain_lifecycle_and_rejections() {
        let coordinator = DrainCoordinator::new();
        assert!(!coordinator.is_draining());
        assert!(coordinator.can_accept_new_session().is_ok());
        assert!(coordinator.can_accept_new_turn().is_ok());

        coordinator.start_drain(30000, "maintenance", 1000);
        assert!(coordinator.is_draining());

        let session_err = coordinator.can_accept_new_session().unwrap_err();
        assert_eq!(
            session_err,
            DrainError::Draining("new sessions are rejected (maintenance)".into())
        );

        let turn_err = coordinator.can_accept_new_turn().unwrap_err();
        assert_eq!(
            turn_err,
            DrainError::Draining("new turns are rejected (maintenance)".into())
        );

        coordinator.complete_drain(1050);
        assert!(!coordinator.is_draining());
        assert!(coordinator.can_accept_new_session().is_err());
    }
}
