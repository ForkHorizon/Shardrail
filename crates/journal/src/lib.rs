use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use shardrail_protocol::{ApprovalDecision, ApprovalResponding, DurableEvent};
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum JournalError {
    #[error("journal unavailable: {0}")]
    Unavailable(#[from] rusqlite::Error),
    #[error("journal unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("approval response is already recorded")]
    AlreadyResponded,
}

pub struct Journal(Connection);

impl Journal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        set_private_file_mode(path.as_ref())?;
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                kind TEXT NOT NULL,
                occurred_at_ms INTEGER NOT NULL,
                payload TEXT NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT, 'append-only'); END;
             CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT, 'append-only'); END;
             CREATE TABLE IF NOT EXISTS approvals (
                approval_id TEXT PRIMARY KEY,
                state TEXT NOT NULL,
                decision TEXT,
                option_id TEXT,
                responder TEXT,
                responded_at_ms INTEGER
             );
             CREATE TABLE IF NOT EXISTS approval_events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                approval_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                data TEXT NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS approval_events_no_update BEFORE UPDATE ON approval_events BEGIN SELECT RAISE(ABORT, 'append-only'); END;
             CREATE TRIGGER IF NOT EXISTS approval_events_no_delete BEFORE DELETE ON approval_events BEGIN SELECT RAISE(ABORT, 'append-only'); END;",
        )?;
        Ok(Self(connection))
    }

    pub fn append_event(&mut self, event: &DurableEvent) -> Result<i64, JournalError> {
        self.0.execute(
            "INSERT INTO events(session_id,schema_version,kind,occurred_at_ms,payload) VALUES (?1,?2,?3,?4,?5)",
            params![event.session_id.to_string(), event.schema_version, event.kind, event.occurred_at_ms, event.payload.to_string()],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    pub fn events_for_session(&self, session_id: Uuid) -> Result<Vec<DurableEvent>, JournalError> {
        let mut statement = self.0.prepare(
            "SELECT event_id,session_id,schema_version,kind,occurred_at_ms,payload FROM events WHERE session_id=?1 ORDER BY event_id",
        )?;
        let events = statement
            .query_map([session_id.to_string()], |row| {
                let session_id: String = row.get(1)?;
                let payload: String = row.get(5)?;
                Ok(DurableEvent {
                    event_id: row.get(0)?,
                    session_id: Uuid::parse_str(&session_id).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            1,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    schema_version: row.get(2)?,
                    kind: row.get(3)?,
                    occurred_at_ms: row.get(4)?,
                    payload: serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            5,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    pub fn create_pending_approval(&mut self, approval_id: Uuid) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO approvals(approval_id,state) VALUES (?1,'pending')",
            [approval_id.to_string()],
        )?;
        tx.execute(
            "INSERT INTO approval_events(approval_id,kind,data) VALUES (?1,'approval_pending','{}')",
            [approval_id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn begin_approval_response(
        &mut self,
        response: &ApprovalResponding,
    ) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT state FROM approvals WHERE approval_id=?1",
                [response.approval_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if exists.as_deref() != Some("pending") {
            return Err(JournalError::AlreadyResponded);
        }
        tx.execute(
            "UPDATE approvals SET state='responding',decision=?2,option_id=?3,responder=?4,responded_at_ms=?5 WHERE approval_id=?1 AND state='pending'",
            params![response.approval_id.to_string(), serde_json::to_string(&response.decision).expect("enum serialization"), response.option_id, response.responder, response.responded_at_ms],
        )?;
        tx.execute(
            "INSERT INTO approval_events(approval_id,kind,data) VALUES (?1,'approval_responding',?2)",
            params![response.approval_id.to_string(), serde_json::to_string(response).expect("approval serialization")],
        )?;
        tx.commit()?;
        self.0.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")?;
        Ok(())
    }

    pub fn recover_responding(&mut self) -> Result<usize, JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement =
                tx.prepare("SELECT approval_id FROM approvals WHERE state='responding'")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for id in &ids {
            tx.execute(
                "UPDATE approvals SET state='delivered_unconfirmed' WHERE approval_id=?1 AND state='responding'",
                [id],
            )?;
            tx.execute(
                "INSERT INTO approval_events(approval_id,kind,data) VALUES (?1,'approval_delivered_unconfirmed','{}')",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    pub fn recover_pending_approvals(&mut self) -> Result<usize, JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement =
                tx.prepare("SELECT approval_id FROM approvals WHERE state='pending'")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for id in &ids {
            tx.execute(
                "UPDATE approvals SET state='withdrawn' WHERE approval_id=?1 AND state='pending'",
                [id],
            )?;
            tx.execute(
                "INSERT INTO approval_events(approval_id,kind,data) VALUES (?1,'approval_withdrawn','{\"reason\":\"daemon_restart\"}')",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    pub fn mark_response_sent(&mut self, approval_id: Uuid) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE approvals SET state='responded_unacked' WHERE approval_id=?1 AND state='responding'",
            [approval_id.to_string()],
        )?;
        if changed == 0 {
            return Err(JournalError::AlreadyResponded);
        }
        tx.execute(
            "INSERT INTO approval_events(approval_id,kind,data) VALUES (?1,'approval_responded_unacked','{}')",
            [approval_id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn acknowledge_approval(&mut self, approval_id: Uuid) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let decision: String = tx.query_row(
            "SELECT decision FROM approvals WHERE approval_id=?1 AND state='responded_unacked'",
            [approval_id.to_string()],
            |row| row.get(0),
        )?;
        let terminal = if decision == "\"allow\"" {
            "allowed"
        } else {
            "denied"
        };
        tx.execute(
            "UPDATE approvals SET state=?2 WHERE approval_id=?1 AND state='responded_unacked'",
            params![approval_id.to_string(), terminal],
        )?;
        tx.execute(
            "INSERT INTO approval_events(approval_id,kind,data) VALUES (?1,?2,'{}')",
            params![approval_id.to_string(), terminal],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn approval_state(&self, id: Uuid) -> Result<Option<String>, JournalError> {
        Ok(self
            .0
            .query_row(
                "SELECT state FROM approvals WHERE approval_id=?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?)
    }

    #[cfg(test)]
    pub fn simulate_disk_full(&self) -> Result<(), JournalError> {
        let page_count: i64 = self
            .0
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        self.0.pragma_update(None, "max_page_count", page_count)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn simulate_corruption(path: &Path) -> std::io::Result<()> {
        std::fs::write(path, b"not a sqlite database")
    }
}

#[cfg(unix)]
fn set_private_file_mode(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    if path == Path::new(":memory:") {
        return Ok(());
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_mode(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub fn decision_label(decision: &ApprovalDecision) -> &'static str {
    match decision {
        ApprovalDecision::Allow => "allow",
        ApprovalDecision::Deny => "deny",
        ApprovalDecision::Cancel => "cancel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn crash_after_provider_send_before_ack_recovers_as_unconfirmed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let response = ApprovalResponding {
            approval_id: id,
            decision: ApprovalDecision::Allow,
            option_id: Some("once".into()),
            responder: "client".into(),
            responded_at_ms: 1,
        };
        let mut journal = Journal::open(&path).unwrap();
        journal.create_pending_approval(id).unwrap();
        journal.begin_approval_response(&response).unwrap(); // Write-ahead record is durable before the provider send.
        let provider_received_response = true; // Crash occurs after the write, before the ack record.
        assert!(provider_received_response);
        drop(journal);
        let mut recovered = Journal::open(&path).unwrap();
        assert_eq!(recovered.recover_responding().unwrap(), 1);
        assert_eq!(
            recovered.approval_state(id).unwrap().as_deref(),
            Some("delivered_unconfirmed")
        );
        let event_kinds = {
            let mut statement = recovered
                .0
                .prepare("SELECT kind FROM approval_events WHERE approval_id=?1 ORDER BY event_id")
                .unwrap();
            statement
                .query_map([id.to_string()], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            event_kinds,
            [
                "approval_pending",
                "approval_responding",
                "approval_delivered_unconfirmed"
            ]
        );
    }

    #[test]
    fn acknowledged_allow_is_a_terminal_approval_event() {
        let dir = tempdir().unwrap();
        let mut journal = Journal::open(dir.path().join("journal.db")).unwrap();
        let id = Uuid::now_v7();
        journal.create_pending_approval(id).unwrap();
        let response = ApprovalResponding {
            approval_id: id,
            decision: ApprovalDecision::Allow,
            option_id: Some("once".into()),
            responder: "client".into(),
            responded_at_ms: 1,
        };
        journal.begin_approval_response(&response).unwrap();
        journal.mark_response_sent(id).unwrap();
        journal.acknowledge_approval(id).unwrap();
        assert_eq!(
            journal.approval_state(id).unwrap().as_deref(),
            Some("allowed")
        );
        assert!(journal.mark_response_sent(id).is_err());
    }

    #[test]
    fn pending_approval_is_withdrawn_on_recovery_never_allowed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let mut journal = Journal::open(&path).unwrap();
        journal.create_pending_approval(id).unwrap();
        drop(journal);
        let mut recovered = Journal::open(&path).unwrap();
        assert_eq!(recovered.recover_pending_approvals().unwrap(), 1);
        assert_eq!(
            recovered.approval_state(id).unwrap().as_deref(),
            Some("withdrawn")
        );
    }

    #[test]
    fn full_database_rejects_writes_and_corruption_refuses_to_open() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).unwrap();
        journal.simulate_disk_full().unwrap();
        let event = DurableEvent {
            event_id: 0,
            session_id: Uuid::now_v7(),
            schema_version: 1,
            kind: "turn_started".into(),
            occurred_at_ms: 1,
            payload: serde_json::json!({"x":"x".repeat(32_000)}),
        };
        assert!(journal.append_event(&event).is_err());
        drop(journal);
        let corrupt = dir.path().join("corrupt.db");
        Journal::simulate_corruption(&corrupt).unwrap();
        assert!(Journal::open(corrupt).is_err());
    }

    #[test]
    fn append_returns_the_committed_event_cursor() {
        let dir = tempdir().unwrap();
        let mut journal = Journal::open(dir.path().join("journal.db")).unwrap();
        let event = DurableEvent {
            event_id: 0,
            session_id: Uuid::now_v7(),
            schema_version: 1,
            kind: "turn_started".into(),
            occurred_at_ms: 1,
            payload: serde_json::json!({}),
        };
        let cursor = journal.append_event(&event).unwrap();
        let stored = journal.events_for_session(event.session_id).unwrap();
        assert_eq!(stored[0].event_id, cursor);
        assert_eq!(cursor, 1);
    }

    #[cfg(unix)]
    #[test]
    fn journal_file_is_created_private_and_existing_files_are_hardened() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        drop(Journal::open(&path).unwrap());
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        drop(Journal::open(&path).unwrap());
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
