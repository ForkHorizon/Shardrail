use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use shardrail_protocol::{ApprovalDecision, ApprovalResponding, ApprovalState, DurableEvent};
use shardrail_session_core::Approval;
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
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
    #[error("approval must be linked to a session")]
    MissingSession,
    #[error("approval is not pending or does not belong to this session")]
    InvalidApproval,
    #[error("journal epoch or host id mismatch")]
    EpochMismatch,
}

pub struct Journal(Connection);

pub struct EventWithReceipt {
    pub event: DurableEvent,
    pub receipt: Option<(u128, i64)>,
}

static MONOTONIC_ORIGIN: OnceLock<Instant> = OnceLock::new();

impl Journal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, JournalError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "journal path has no filename",
            )
        })?;
        let path = parent.canonicalize()?.join(name);
        reject_non_regular_path(&path)?;
        set_private_file_mode(&path)?;
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
             "CREATE TABLE IF NOT EXISTS events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                journal_epoch TEXT NOT NULL DEFAULT '',
                host_id TEXT NOT NULL DEFAULT '',
                session_id TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                kind TEXT NOT NULL,
                occurred_at_ms INTEGER NOT NULL,
                provider_ref TEXT,
                causation TEXT,
                redaction_flags TEXT NOT NULL DEFAULT '[]',
                payload TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS approvals (
                approval_id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                state TEXT NOT NULL,
                decision TEXT,
                terminal_state TEXT,
                terminal_reason TEXT,
                option_id TEXT,
                responder TEXT,
                responded_at_ms INTEGER,
                created_at_ms INTEGER NOT NULL,
                confirmed_at_ms INTEGER NOT NULL,
                needs_reconfirmation INTEGER NOT NULL,
                held INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS approval_events (
                event_id INTEGER PRIMARY KEY AUTOINCREMENT,
                approval_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS journal_meta (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                journal_epoch TEXT NOT NULL,
                host_id TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS event_receipts (
                event_id INTEGER PRIMARY KEY,
                receipt_monotonic_ns TEXT NOT NULL,
                receipt_wall_clock_ms INTEGER NOT NULL,
                FOREIGN KEY(event_id) REFERENCES events(event_id)
             );
             CREATE TRIGGER IF NOT EXISTS approval_events_no_update BEFORE UPDATE ON approval_events BEGIN SELECT RAISE(ABORT, 'append-only'); END;
             CREATE TRIGGER IF NOT EXISTS approval_events_no_delete BEFORE DELETE ON approval_events BEGIN SELECT RAISE(ABORT, 'append-only'); END;",
        )?;
        let legacy_events = ensure_columns(&connection)?;
        connection.execute_batch("CREATE INDEX IF NOT EXISTS approvals_by_session ON approvals(session_id,created_at_ms); CREATE INDEX IF NOT EXISTS approval_events_by_session ON approval_events(session_id,event_id);")?;
        connection.execute(
            "INSERT OR IGNORE INTO journal_meta(singleton,journal_epoch,host_id) VALUES (1,?1,?2)",
            params![Uuid::now_v7().to_string(), Uuid::now_v7().to_string()],
        )?;
        let epoch: String = connection.query_row(
            "SELECT journal_epoch FROM journal_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        let host: String = connection.query_row(
            "SELECT host_id FROM journal_meta WHERE singleton=1",
            [],
            |row| row.get(0),
        )?;
        if legacy_events {
            connection.execute_batch("DROP TRIGGER IF EXISTS events_no_update")?;
            connection.execute(
                "UPDATE events SET journal_epoch=?1,host_id=?2",
                params![epoch, host],
            )?;
        }
        connection.execute_batch("CREATE TRIGGER IF NOT EXISTS events_no_update BEFORE UPDATE ON events BEGIN SELECT RAISE(ABORT, 'append-only'); END; CREATE TRIGGER IF NOT EXISTS events_no_delete BEFORE DELETE ON events BEGIN SELECT RAISE(ABORT, 'append-only'); END;")?;
        Ok(Self(connection))
    }

    pub fn journal_epoch(&self) -> Result<Uuid, JournalError> {
        self.meta_id("journal_epoch")
    }

    pub fn host_id(&self) -> Result<Uuid, JournalError> {
        self.meta_id("host_id")
    }

    fn meta_id(&self, field: &str) -> Result<Uuid, JournalError> {
        let value: String = self.0.query_row(
            &format!("SELECT {field} FROM journal_meta WHERE singleton=1"),
            [],
            |row| row.get(0),
        )?;
        Uuid::parse_str(&value).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
            .into()
        })
    }

    pub fn append_event(&mut self, event: &DurableEvent) -> Result<i64, JournalError> {
        if event.journal_epoch != self.journal_epoch()? || event.host_id != self.host_id()? {
            return Err(JournalError::EpochMismatch);
        }
        let (payload, redacted) = redact_value(event.payload.clone());
        let mut flags = event.redaction_flags.clone();
        if redacted && !flags.iter().any(|flag| flag == "redacted") {
            flags.push("redacted".into());
        }
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO events(journal_epoch,host_id,session_id,schema_version,kind,occurred_at_ms,provider_ref,causation,redaction_flags,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![event.journal_epoch.to_string(), event.host_id.to_string(), event.session_id.to_string(), event.schema_version, event.kind, event.occurred_at_ms, event.provider_ref, event.causation, serde_json::to_string(&flags).expect("redaction flags serialize"), payload.to_string()],
        )?;
        let event_id = tx.last_insert_rowid();
        let monotonic_ns = MONOTONIC_ORIGIN
            .get_or_init(Instant::now)
            .elapsed()
            .as_nanos();
        let wall_clock_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as i64);
        tx.execute(
            "INSERT INTO event_receipts(event_id,receipt_monotonic_ns,receipt_wall_clock_ms) VALUES (?1,?2,?3)",
            params![event_id, monotonic_ns.to_string(), wall_clock_ms],
        )?;
        tx.commit()?;
        Ok(event_id)
    }

    pub fn event_receipt(&self, event_id: i64) -> Result<Option<(u128, i64)>, JournalError> {
        let receipt = self
            .0
            .query_row(
                "SELECT receipt_monotonic_ns,receipt_wall_clock_ms FROM event_receipts WHERE event_id=?1",
                [event_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        receipt
            .map(|(monotonic, wall)| {
                monotonic
                    .parse::<u128>()
                    .map(|monotonic| (monotonic, wall))
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
            })
            .transpose()
            .map_err(Into::into)
    }

    pub fn high_water(&self) -> Result<i64, JournalError> {
        Ok(self
            .0
            .query_row("SELECT COALESCE(MAX(event_id),0) FROM events", [], |row| {
                row.get(0)
            })?)
    }

    pub fn created_session_for_command(
        &self,
        client_id: Uuid,
        command_id: Uuid,
    ) -> Result<Option<(Uuid, i64)>, JournalError> {
        let row = self
            .0
            .query_row(
                "SELECT json_extract(payload,'$.session_id'),event_id FROM events WHERE kind='session_created' AND json_extract(payload,'$.client_id')=?1 AND json_extract(payload,'$.command_id')=?2 ORDER BY event_id LIMIT 1",
                params![client_id.to_string(), command_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        row.map(|(session_id, event_id)| {
            Uuid::parse_str(&session_id)
                .map(|session_id| (session_id, event_id))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
        })
        .transpose()
        .map_err(Into::into)
    }

    pub fn events_after_with_receipts(
        &self,
        cursor: i64,
        limit: usize,
    ) -> Result<Vec<EventWithReceipt>, JournalError> {
        let mut statement = self.0.prepare(
            "SELECT e.event_id,e.journal_epoch,e.host_id,e.session_id,e.schema_version,e.kind,e.occurred_at_ms,e.provider_ref,e.causation,e.redaction_flags,e.payload,r.receipt_monotonic_ns,r.receipt_wall_clock_ms FROM events e LEFT JOIN event_receipts r ON r.event_id=e.event_id WHERE e.event_id>?1 ORDER BY e.event_id LIMIT ?2",
        )?;
        let rows = statement.query_map(params![cursor, limit as i64], |row| {
            let parse_uuid = |index| -> rusqlite::Result<Uuid> {
                let value: String = row.get(index)?;
                Uuid::parse_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            };
            let flags: String = row.get(9)?;
            let payload: String = row.get(10)?;
            let monotonic: Option<String> = row.get(11)?;
            let wall: Option<i64> = row.get(12)?;
            let receipt = monotonic
                .map(|value| value.parse::<u128>())
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        11,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?
                .zip(wall);
            Ok(EventWithReceipt {
                event: DurableEvent {
                    event_id: row.get(0)?,
                    journal_epoch: parse_uuid(1)?,
                    host_id: parse_uuid(2)?,
                    session_id: parse_uuid(3)?,
                    schema_version: row.get(4)?,
                    kind: row.get(5)?,
                    occurred_at_ms: row.get(6)?,
                    provider_ref: row.get(7)?,
                    causation: row.get(8)?,
                    redaction_flags: serde_json::from_str(&flags).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    payload: serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            10,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                },
                receipt,
            })
        })?;
        let events = rows.collect::<Result<Vec<_>, _>>()?;
        let epoch = self.journal_epoch()?;
        let host_id = self.host_id()?;
        if events
            .iter()
            .any(|item| item.event.journal_epoch != epoch || item.event.host_id != host_id)
        {
            return Err(JournalError::EpochMismatch);
        }
        Ok(events)
    }

    pub fn events_for_session(&self, session_id: Uuid) -> Result<Vec<DurableEvent>, JournalError> {
        let mut statement = self.0.prepare(
            "SELECT event_id,journal_epoch,host_id,session_id,schema_version,kind,occurred_at_ms,provider_ref,causation,redaction_flags,payload FROM events WHERE session_id=?1 ORDER BY event_id",
        )?;
        let events = statement
            .query_map([session_id.to_string()], |row| {
                let parse_uuid = |index| -> rusqlite::Result<Uuid> {
                    let value: String = row.get(index)?;
                    Uuid::parse_str(&value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            index,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                };
                let payload: String = row.get(10)?;
                let flags: String = row.get(9)?;
                Ok(DurableEvent {
                    event_id: row.get(0)?,
                    journal_epoch: parse_uuid(1)?,
                    host_id: parse_uuid(2)?,
                    session_id: parse_uuid(3)?,
                    schema_version: row.get(4)?,
                    kind: row.get(5)?,
                    occurred_at_ms: row.get(6)?,
                    provider_ref: row.get(7)?,
                    causation: row.get(8)?,
                    redaction_flags: serde_json::from_str(&flags).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            9,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                    payload: serde_json::from_str(&payload).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            10,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let epoch = self.journal_epoch()?;
        let host_id = self.host_id()?;
        if events
            .iter()
            .any(|event| event.journal_epoch != epoch || event.host_id != host_id)
        {
            return Err(JournalError::EpochMismatch);
        }
        Ok(events)
    }

    pub fn events_after(&self, cursor: i64) -> Result<Vec<DurableEvent>, JournalError> {
        let mut statement = self.0.prepare(
            "SELECT event_id,journal_epoch,host_id,session_id,schema_version,kind,occurred_at_ms,provider_ref,causation,redaction_flags,payload FROM events WHERE event_id>?1 ORDER BY event_id",
        )?;
        let rows = statement.query_map([cursor], |row| {
            let parse_uuid = |index| -> rusqlite::Result<Uuid> {
                let value: String = row.get(index)?;
                Uuid::parse_str(&value).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })
            };
            let flags: String = row.get(9)?;
            let payload: String = row.get(10)?;
            Ok(DurableEvent {
                event_id: row.get(0)?,
                journal_epoch: parse_uuid(1)?,
                host_id: parse_uuid(2)?,
                session_id: parse_uuid(3)?,
                schema_version: row.get(4)?,
                kind: row.get(5)?,
                occurred_at_ms: row.get(6)?,
                provider_ref: row.get(7)?,
                causation: row.get(8)?,
                redaction_flags: serde_json::from_str(&flags).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                payload: serde_json::from_str(&payload).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        10,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
            })
        })?;
        let events = rows.collect::<Result<Vec<_>, _>>()?;
        let epoch = self.journal_epoch()?;
        let host_id = self.host_id()?;
        if events
            .iter()
            .any(|event| event.journal_epoch != epoch || event.host_id != host_id)
        {
            return Err(JournalError::EpochMismatch);
        }
        Ok(events)
    }

    pub fn create_pending_approval(&mut self, approval: &Approval) -> Result<(), JournalError> {
        if approval.session_id.is_nil() {
            return Err(JournalError::MissingSession);
        }
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO approvals(approval_id,session_id,state,created_at_ms,confirmed_at_ms,needs_reconfirmation,held) VALUES (?1,?2,'pending',?3,?4,?5,?6)",
            params![approval.id.to_string(), approval.session_id.to_string(), approval.created_at_ms, approval.confirmed_at_ms, approval.needs_reconfirmation, approval.held],
        )?;
        insert_approval_event(
            &tx,
            approval.id,
            approval.session_id,
            "approval_pending",
            "{}",
        )?;
        insert_session_event(
            &tx,
            approval.session_id,
            "approval_pending",
            approval.created_at_ms,
            serde_json::json!({"approval_id":approval.id}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn load_approvals(&self, session_id: Uuid) -> Result<Vec<Approval>, JournalError> {
        let mut statement = self.0.prepare("SELECT approval_id,state,created_at_ms,confirmed_at_ms,needs_reconfirmation,held,decision FROM approvals WHERE session_id=?1 ORDER BY created_at_ms,approval_id")?;
        let rows = statement.query_map([session_id.to_string()], |row| {
            let id: String = row.get(0)?;
            let state: String = row.get(1)?;
            let decision: Option<String> = row.get(6)?;
            Ok((
                id,
                state,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                decision,
            ))
        })?;
        rows.map(|row| {
            let (id, state, created_at_ms, confirmed_at_ms, needs_reconfirmation, held, decision) =
                row?;
            let state: ApprovalState =
                serde_json::from_str(&format!("\"{state}\"")).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok::<Approval, rusqlite::Error>(Approval {
                id: Uuid::parse_str(&id).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                session_id,
                state,
                created_at_ms,
                confirmed_at_ms,
                needs_reconfirmation,
                held,
                decision: decision
                    .map(|value| serde_json::from_str(&value))
                    .transpose()
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            6,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
    }

    pub fn update_approval_confirmation(
        &mut self,
        approval: &Approval,
    ) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute("UPDATE approvals SET confirmed_at_ms=?2,needs_reconfirmation=?3,held=?4 WHERE approval_id=?1 AND session_id=?5 AND state='pending'", params![approval.id.to_string(), approval.confirmed_at_ms, approval.needs_reconfirmation, approval.held, approval.session_id.to_string()])?;
        if changed == 0 {
            return Err(JournalError::AlreadyResponded);
        }
        insert_approval_event(
            &tx,
            approval.id,
            approval.session_id,
            "approval_confirmation_updated",
            &serde_json::to_string(approval).expect("approval serializes"),
        )?;
        insert_session_event(
            &tx,
            approval.session_id,
            "approval_confirmation_updated",
            approval.confirmed_at_ms,
            serde_json::json!({"approval_id":approval.id,"needs_reconfirmation":approval.needs_reconfirmation,"held":approval.held}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn begin_approval_response(
        &mut self,
        response: &ApprovalResponding,
        session: Uuid,
    ) -> Result<(), JournalError> {
        let (terminal, reason) = match response.decision {
            ApprovalDecision::Allow => ("allowed", None),
            ApprovalDecision::Deny => ("denied", None),
            ApprovalDecision::Cancel => ("withdrawn", Some("cancelled")),
        };
        self.begin_approval_response_with_terminal(response, session, terminal, reason)
    }

    pub fn begin_expired_approval_response(
        &mut self,
        response: &ApprovalResponding,
        session: Uuid,
    ) -> Result<(), JournalError> {
        self.begin_approval_response_with_terminal(response, session, "expired", None)
    }

    fn begin_approval_response_with_terminal(
        &mut self,
        response: &ApprovalResponding,
        session: Uuid,
        terminal: &str,
        terminal_reason: Option<&str>,
    ) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: Option<String> = tx
            .query_row(
                "SELECT state FROM approvals WHERE approval_id=?1 AND session_id=?2",
                params![response.approval_id.to_string(), session.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if exists.as_deref() != Some("pending") {
            if exists.is_none() {
                return Err(JournalError::InvalidApproval);
            }
            return Err(JournalError::AlreadyResponded);
        }
        tx.execute(
            "UPDATE approvals SET state='responding',decision=?2,terminal_state=?3,terminal_reason=?4,option_id=?5,responder=?6,responded_at_ms=?7,confirmed_at_ms=?7 WHERE approval_id=?1 AND session_id=?8 AND state='pending'",
            params![response.approval_id.to_string(), serde_json::to_string(&response.decision).expect("enum serialization"), terminal, terminal_reason, response.option_id, response.responder, response.responded_at_ms, session.to_string()],
        )?;
        let session_id: String = tx.query_row(
            "SELECT session_id FROM approvals WHERE approval_id=?1",
            [response.approval_id.to_string()],
            |row| row.get(0),
        )?;
        let session_id = Uuid::parse_str(&session_id).expect("stored session UUID");
        let data = serde_json::to_string(response).expect("approval serialization");
        insert_approval_event(
            &tx,
            response.approval_id,
            session_id,
            "approval_responding",
            &data,
        )?;
        insert_session_event(
            &tx,
            session_id,
            "approval_responding",
            response.responded_at_ms,
            serde_json::from_str(&data).expect("approval response JSON"),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn recover_responding(&mut self, now_ms: i64) -> Result<usize, JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = {
            let mut statement =
                tx.prepare("SELECT approval_id FROM approvals WHERE state IN ('responding','responded_unacked')")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        for id in &ids {
            tx.execute(
                "UPDATE approvals SET state='delivered_unconfirmed' WHERE approval_id=?1 AND state IN ('responding','responded_unacked')",
                [id],
            )?;
            let approval_id = Uuid::parse_str(id).expect("stored approval UUID");
            if let Some(session_id) = approval_session_id(&tx, id)? {
                insert_approval_event(
                    &tx,
                    approval_id,
                    session_id,
                    "approval_delivered_unconfirmed",
                    "{}",
                )?;
                insert_session_event(
                    &tx,
                    session_id,
                    "approval_delivered_unconfirmed",
                    now_ms,
                    serde_json::json!({"approval_id":approval_id}),
                )?;
            } else {
                insert_unlinked_approval_event(
                    &tx,
                    approval_id,
                    "approval_delivered_unconfirmed",
                    "{}",
                )?;
            }
        }
        tx.commit()?;
        Ok(ids.len())
    }

    pub fn recover_pending_approvals(&mut self, now_ms: i64) -> Result<usize, JournalError> {
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
            let approval_id = Uuid::parse_str(id).expect("stored approval UUID");
            if let Some(session_id) = approval_session_id(&tx, id)? {
                insert_approval_event(
                    &tx,
                    approval_id,
                    session_id,
                    "approval_withdrawn",
                    "{\"reason\":\"daemon_restart\"}",
                )?;
                insert_session_event(
                    &tx,
                    session_id,
                    "approval_withdrawn",
                    now_ms,
                    serde_json::json!({"approval_id":approval_id,"reason":"daemon_restart"}),
                )?;
            } else {
                insert_unlinked_approval_event(
                    &tx,
                    approval_id,
                    "approval_withdrawn",
                    "{\"reason\":\"daemon_restart\"}",
                )?;
            }
        }
        tx.commit()?;
        Ok(ids.len())
    }

    pub fn mark_response_sent(
        &mut self,
        approval_id: Uuid,
        now_ms: i64,
    ) -> Result<(), JournalError> {
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
        let session_id: String = tx.query_row(
            "SELECT session_id FROM approvals WHERE approval_id=?1",
            [approval_id.to_string()],
            |row| row.get(0),
        )?;
        let session_id = Uuid::parse_str(&session_id).expect("stored session UUID");
        insert_approval_event(
            &tx,
            approval_id,
            session_id,
            "approval_responded_unacked",
            "{}",
        )?;
        insert_session_event(
            &tx,
            session_id,
            "approval_responded_unacked",
            now_ms,
            serde_json::json!({"approval_id":approval_id}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn acknowledge_approval(
        &mut self,
        approval_id: Uuid,
        session: Uuid,
        now_ms: i64,
    ) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let found: Option<(String, Option<String>)> = tx.query_row(
            "SELECT terminal_state,terminal_reason FROM approvals WHERE approval_id=?1 AND session_id=?2 AND state='responded_unacked'",
            params![approval_id.to_string(), session.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        let (terminal, reason) = found.ok_or(JournalError::InvalidApproval)?;
        let changed = tx.execute(
            "UPDATE approvals SET state=?2 WHERE approval_id=?1 AND session_id=?3 AND state='responded_unacked'",
            params![approval_id.to_string(), terminal, session.to_string()],
        )?;
        if changed == 0 {
            return Err(JournalError::InvalidApproval);
        }
        let session_id: String = tx.query_row(
            "SELECT session_id FROM approvals WHERE approval_id=?1",
            [approval_id.to_string()],
            |row| row.get(0),
        )?;
        let session_id = Uuid::parse_str(&session_id).expect("stored session UUID");
        let data = serde_json::json!({"reason":reason}).to_string();
        insert_approval_event(&tx, approval_id, session_id, &terminal, &data)?;
        insert_session_event(
            &tx,
            session_id,
            &terminal,
            now_ms,
            serde_json::json!({"approval_id":approval_id,"reason":reason}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn reject_approval_response(
        &mut self,
        approval_id: Uuid,
        session: Uuid,
        now_ms: i64,
    ) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE approvals SET state='withdrawn' WHERE approval_id=?1 AND session_id=?2 AND state='responded_unacked'",
            params![approval_id.to_string(), session.to_string()],
        )?;
        if changed == 0 {
            return Err(JournalError::InvalidApproval);
        }
        let session_id: String = tx.query_row(
            "SELECT session_id FROM approvals WHERE approval_id=?1",
            [approval_id.to_string()],
            |row| row.get(0),
        )?;
        let session_id = Uuid::parse_str(&session_id).expect("stored session UUID");
        insert_approval_event(
            &tx,
            approval_id,
            session_id,
            "approval_withdrawn",
            "{\"reason\":\"rejected_by_provider\"}",
        )?;
        insert_session_event(
            &tx,
            session_id,
            "approval_withdrawn",
            now_ms,
            serde_json::json!({"approval_id":approval_id,"reason":"rejected_by_provider"}),
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

    pub fn withdraw_pending_approvals(
        &mut self,
        session_id: Uuid,
        reason: &str,
        now_ms: i64,
    ) -> Result<(), JournalError> {
        let tx = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let approvals = {
            let mut statement = tx.prepare(
                "SELECT approval_id,state FROM approvals WHERE session_id=?1 AND state IN ('pending','responded_unacked')",
            )?;
            statement
                .query_map([session_id.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (id, state) in approvals {
            let approval_id = Uuid::parse_str(&id).expect("stored approval UUID");
            let (kind, next_state, terminal_reason) = if state == "pending" {
                ("approval_withdrawn", "withdrawn", Some(reason))
            } else {
                (
                    "approval_delivered_unconfirmed",
                    "delivered_unconfirmed",
                    None,
                )
            };
            tx.execute(
                "UPDATE approvals SET state=?3 WHERE approval_id=?1 AND session_id=?2",
                params![id, session_id.to_string(), next_state],
            )?;
            let data = serde_json::json!({"approval_id":approval_id,"reason":terminal_reason});
            insert_approval_event(&tx, approval_id, session_id, kind, &data.to_string())?;
            insert_session_event(&tx, session_id, kind, now_ms, data)?;
        }
        tx.commit()?;
        Ok(())
    }

    #[cfg(feature = "test-utils")]
    pub fn simulate_disk_full(&self) -> Result<(), JournalError> {
        let page_count: i64 = self
            .0
            .query_row("PRAGMA page_count", [], |row| row.get(0))?;
        self.0.pragma_update(None, "max_page_count", page_count)?;
        Ok(())
    }

    #[cfg(feature = "test-utils")]
    pub fn fail_approval_writes_for_test(&self) -> Result<(), JournalError> {
        self.0.execute_batch(
            "CREATE TEMP TRIGGER fail_approval_writes BEFORE INSERT ON approval_events
             WHEN NEW.kind='approval_responding'
             BEGIN SELECT RAISE(ABORT, 'injected journal failure'); END;",
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn simulate_corruption(path: &Path) -> std::io::Result<()> {
        std::fs::write(path, b"not a sqlite database")
    }
}

fn redact_value(mut value: serde_json::Value) -> (serde_json::Value, bool) {
    fn redact(value: &mut serde_json::Value, changed: &mut bool) {
        match value {
            serde_json::Value::String(text) => {
                let lower = text.to_ascii_lowercase();
                let secret = lower.contains("authorization:")
                    || lower.contains("bearer ")
                    || lower.contains("sk-live-")
                    || lower.contains("sk-ant-")
                    || lower.contains("ghp_")
                    || lower.contains("github_pat_")
                    || lower.contains("xoxb-")
                    || lower.contains("xoxp-")
                    || text.contains("AKIA")
                    || lower.contains("api_key=")
                    || lower.contains("api-key:");
                if secret {
                    *text = "[REDACTED]".into();
                    *changed = true;
                }
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    redact(value, changed);
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    let normalized_key = key
                        .chars()
                        .filter(|character| character.is_ascii_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>();
                    if matches!(
                        normalized_key.as_str(),
                        "authorization"
                            | "apikey"
                            | "token"
                            | "secret"
                            | "password"
                            | "passwd"
                            | "cookie"
                            | "setcookie"
                            | "auth"
                            | "credential"
                            | "credentials"
                            | "privatekey"
                            | "accesskey"
                            | "clientsecret"
                    ) {
                        if !value.is_null() {
                            *value = serde_json::Value::String("[REDACTED]".into());
                            *changed = true;
                        }
                    } else {
                        redact(value, changed);
                    }
                }
            }
            _ => {}
        }
    }
    let mut changed = false;
    redact(&mut value, &mut changed);
    (value, changed)
}

fn ensure_columns(connection: &Connection) -> Result<bool, JournalError> {
    let mut legacy_events = false;
    for (table, name, declaration) in [
        ("events", "journal_epoch", "TEXT NOT NULL DEFAULT ''"),
        ("events", "host_id", "TEXT NOT NULL DEFAULT ''"),
        ("events", "provider_ref", "TEXT"),
        ("events", "causation", "TEXT"),
        ("events", "redaction_flags", "TEXT NOT NULL DEFAULT '[]'"),
        ("approvals", "session_id", "TEXT NOT NULL DEFAULT ''"),
        ("approvals", "terminal_state", "TEXT"),
        ("approvals", "terminal_reason", "TEXT"),
        ("approvals", "created_at_ms", "INTEGER NOT NULL DEFAULT 0"),
        ("approvals", "confirmed_at_ms", "INTEGER NOT NULL DEFAULT 0"),
        (
            "approvals",
            "needs_reconfirmation",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("approvals", "held", "INTEGER NOT NULL DEFAULT 0"),
        ("approval_events", "session_id", "TEXT NOT NULL DEFAULT ''"),
    ] {
        let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == name) {
            if table == "events" && matches!(name, "journal_epoch" | "host_id") {
                legacy_events = true;
            }
            connection.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {name} {declaration}"),
                [],
            )?;
        }
    }
    Ok(legacy_events)
}

fn insert_approval_event(
    tx: &rusqlite::Transaction<'_>,
    approval_id: Uuid,
    session_id: Uuid,
    kind: &str,
    data: &str,
) -> Result<(), JournalError> {
    tx.execute(
        "INSERT INTO approval_events(approval_id,session_id,kind,data) VALUES (?1,?2,?3,?4)",
        params![approval_id.to_string(), session_id.to_string(), kind, data],
    )?;
    Ok(())
}

fn insert_unlinked_approval_event(
    tx: &rusqlite::Transaction<'_>,
    approval_id: Uuid,
    kind: &str,
    data: &str,
) -> Result<(), JournalError> {
    tx.execute(
        "INSERT INTO approval_events(approval_id,session_id,kind,data) VALUES (?1,'',?2,?3)",
        params![approval_id.to_string(), kind, data],
    )?;
    Ok(())
}

fn approval_session_id(
    tx: &rusqlite::Transaction<'_>,
    approval_id: &str,
) -> Result<Option<Uuid>, JournalError> {
    let value: String = tx.query_row(
        "SELECT session_id FROM approvals WHERE approval_id=?1",
        [approval_id],
        |row| row.get(0),
    )?;
    if value.is_empty() {
        Ok(None)
    } else {
        Uuid::parse_str(&value).map(Some).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
            .into()
        })
    }
}

fn insert_session_event(
    tx: &rusqlite::Transaction<'_>,
    session_id: Uuid,
    kind: &str,
    occurred_at_ms: i64,
    payload: serde_json::Value,
) -> Result<(), JournalError> {
    let (epoch, host): (String, String) = tx.query_row(
        "SELECT journal_epoch,host_id FROM journal_meta WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    tx.execute("INSERT INTO events(journal_epoch,host_id,session_id,schema_version,kind,occurred_at_ms,redaction_flags,payload) VALUES (?1,?2,?3,1,?4,?5,'[]',?6)", params![epoch, host, session_id.to_string(), kind, occurred_at_ms, payload.to_string()])?;
    Ok(())
}

fn reject_non_regular_path(path: &Path) -> Result<(), JournalError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "journal path must be a regular file, not a symlink",
            )
            .into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
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
    use shardrail_protocol::{ClientScope, OptionClass};
    use tempfile::tempdir;

    fn pending_approval(id: Uuid) -> Approval {
        let mut approval = Approval::new(1);
        approval.id = id;
        approval.session_id = Uuid::now_v7();
        approval
    }

    #[test]
    fn open_adds_terminal_outcome_column_to_existing_journal() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE approvals (
                    approval_id TEXT PRIMARY KEY,
                    state TEXT NOT NULL,
                    decision TEXT,
                    option_id TEXT,
                    responder TEXT,
                    responded_at_ms INTEGER
                 );",
            )
            .unwrap();
        drop(connection);
        let journal = Journal::open(&path).unwrap();
        let mut statement = journal.0.prepare("PRAGMA table_info(approvals)").unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let has_column = columns.iter().any(|name| name == "terminal_state");
        assert!(has_column);
    }

    #[test]
    fn open_backfills_envelope_for_existing_event_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let session_id = Uuid::now_v7();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE events (event_id INTEGER PRIMARY KEY AUTOINCREMENT,session_id TEXT NOT NULL,schema_version INTEGER NOT NULL,kind TEXT NOT NULL,occurred_at_ms INTEGER NOT NULL,payload TEXT NOT NULL);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO events(session_id,schema_version,kind,occurred_at_ms,payload) VALUES (?1,1,'legacy',9,'{}')",
                [session_id.to_string()],
            )
            .unwrap();
        drop(connection);
        let journal = Journal::open(&path).unwrap();
        let events = journal.events_for_session(session_id).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "legacy");
        assert_eq!(events[0].journal_epoch, journal.journal_epoch().unwrap());
        assert_eq!(events[0].host_id, journal.host_id().unwrap());
    }

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
            option_class: OptionClass::Once,
            client_scope: ClientScope::Full,
        };
        let mut journal = Journal::open(&path).unwrap();
        let approval = pending_approval(id);
        let session_id = approval.session_id;
        journal.create_pending_approval(&approval).unwrap();
        journal
            .begin_approval_response(&response, session_id)
            .unwrap(); // Write-ahead record is durable before the provider send.
        let provider_received_response = true; // Crash occurs after the write, before the ack record.
        assert!(provider_received_response);
        drop(journal);
        let mut recovered = Journal::open(&path).unwrap();
        assert_eq!(recovered.recover_responding(3).unwrap(), 1);
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
        let approval = pending_approval(id);
        let session_id = approval.session_id;
        journal.create_pending_approval(&approval).unwrap();
        let response = ApprovalResponding {
            approval_id: id,
            decision: ApprovalDecision::Allow,
            option_id: Some("once".into()),
            responder: "client".into(),
            responded_at_ms: 1,
            option_class: OptionClass::Once,
            client_scope: ClientScope::Full,
        };
        journal
            .begin_approval_response(&response, session_id)
            .unwrap();
        journal.mark_response_sent(id, 2).unwrap();
        journal.acknowledge_approval(id, session_id, 3).unwrap();
        assert_eq!(
            journal.approval_state(id).unwrap().as_deref(),
            Some("allowed")
        );
        assert!(journal.mark_response_sent(id, 4).is_err());
    }

    #[test]
    fn cancelled_response_is_withdrawn_with_cancelled_reason() {
        let dir = tempdir().unwrap();
        let mut journal = Journal::open(dir.path().join("journal.db")).unwrap();
        let id = Uuid::now_v7();
        let approval = pending_approval(id);
        let session_id = approval.session_id;
        journal.create_pending_approval(&approval).unwrap();
        let response = ApprovalResponding {
            approval_id: id,
            decision: ApprovalDecision::Cancel,
            option_id: None,
            responder: "timeout-policy".into(),
            responded_at_ms: 10,
            option_class: OptionClass::Once,
            client_scope: ClientScope::Full,
        };
        journal
            .begin_approval_response(&response, session_id)
            .unwrap();
        journal.mark_response_sent(id, 2).unwrap();
        journal.acknowledge_approval(id, session_id, 3).unwrap();
        assert_eq!(
            journal.approval_state(id).unwrap().as_deref(),
            Some("withdrawn")
        );
        let event = journal
            .events_for_session(session_id)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(event.kind, "withdrawn");
        assert_eq!(event.payload["reason"], "cancelled");
    }

    #[test]
    fn pending_approval_is_withdrawn_on_recovery_never_allowed() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let id = Uuid::now_v7();
        let mut journal = Journal::open(&path).unwrap();
        journal
            .create_pending_approval(&pending_approval(id))
            .unwrap();
        drop(journal);
        let mut recovered = Journal::open(&path).unwrap();
        assert_eq!(recovered.recover_pending_approvals(3).unwrap(), 1);
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
            journal_epoch: journal.journal_epoch().unwrap(),
            host_id: journal.host_id().unwrap(),
            session_id: Uuid::now_v7(),
            schema_version: 1,
            kind: "turn_started".into(),
            occurred_at_ms: 1,
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
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
            journal_epoch: journal.journal_epoch().unwrap(),
            host_id: journal.host_id().unwrap(),
            session_id: Uuid::now_v7(),
            schema_version: 1,
            kind: "turn_started".into(),
            occurred_at_ms: 1,
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload: serde_json::json!({}),
        };
        let cursor = journal.append_event(&event).unwrap();
        let stored = journal.events_for_session(event.session_id).unwrap();
        assert_eq!(stored[0].event_id, cursor);
        assert_eq!(cursor, 1);
        let (monotonic_ns, wall_clock_ms) = journal.event_receipt(cursor).unwrap().unwrap();
        assert!(wall_clock_ms > 0);
        assert!(monotonic_ns > 0);
    }

    #[test]
    fn journal_epoch_is_stable_and_mismatched_events_are_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).unwrap();
        let epoch = journal.journal_epoch().unwrap();
        let host_id = journal.host_id().unwrap();
        let mut event = DurableEvent {
            event_id: 0,
            journal_epoch: epoch,
            host_id,
            session_id: Uuid::now_v7(),
            schema_version: 1,
            kind: "event".into(),
            occurred_at_ms: 1,
            provider_ref: Some("provider:session".into()),
            causation: Some("command-id".into()),
            redaction_flags: vec!["redacted".into()],
            payload: serde_json::json!({}),
        };
        journal.append_event(&event).unwrap();
        let stored = journal.events_for_session(event.session_id).unwrap();
        assert_eq!(stored[0].journal_epoch, epoch);
        assert_eq!(stored[0].host_id, host_id);
        assert_eq!(stored[0].causation.as_deref(), Some("command-id"));
        assert_eq!(stored[0].redaction_flags, ["redacted"]);
        event.journal_epoch = Uuid::now_v7();
        assert!(journal.append_event(&event).is_err());
        drop(journal);
        let reopened = Journal::open(path).unwrap();
        assert_eq!(reopened.journal_epoch().unwrap(), epoch);
        assert_eq!(reopened.host_id().unwrap(), host_id);
    }

    #[test]
    fn secret_shaped_event_values_are_redacted_before_persistence() {
        let dir = tempdir().unwrap();
        let mut journal = Journal::open(dir.path().join("journal.db")).unwrap();
        let session_id = Uuid::now_v7();
        let secret = "Bearer sk-live-example-secret";
        let event = DurableEvent {
            event_id: 0,
            journal_epoch: journal.journal_epoch().unwrap(),
            host_id: journal.host_id().unwrap(),
            session_id,
            schema_version: 1,
            kind: "provider_output".into(),
            occurred_at_ms: 10,
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload: serde_json::json!({"text": format!("Authorization: {secret}")}),
        };
        journal.append_event(&event).unwrap();
        let stored = journal.events_for_session(session_id).unwrap().remove(0);
        assert!(!stored.payload.to_string().contains(secret));
        assert_eq!(stored.payload["text"], "[REDACTED]");
        assert_eq!(stored.redaction_flags, ["redacted"]);
    }

    #[test]
    fn common_credential_fields_and_token_prefixes_are_redacted() {
        let (payload, redacted) = redact_value(serde_json::json!({
            "cookie": "session=private-value",
            "password": "hunter2",
            "nested": {"client_secret": "private-value"},
            "output": "GitHub token ghp_example-private-token",
        }));

        assert!(redacted);
        assert_eq!(payload["cookie"], "[REDACTED]");
        assert_eq!(payload["password"], "[REDACTED]");
        assert_eq!(payload["nested"]["client_secret"], "[REDACTED]");
        assert_eq!(payload["output"], "[REDACTED]");
    }

    #[test]
    fn approval_lifecycle_events_use_supplied_timestamps() {
        let dir = tempdir().unwrap();
        let mut journal = Journal::open(dir.path().join("journal.db")).unwrap();
        let approval = pending_approval(Uuid::now_v7());
        let id = approval.id;
        let session_id = approval.session_id;
        journal.create_pending_approval(&approval).unwrap();
        let response = ApprovalResponding {
            approval_id: id,
            decision: ApprovalDecision::Deny,
            option_id: None,
            responder: "client".into(),
            responded_at_ms: 12,
            option_class: OptionClass::Once,
            client_scope: ClientScope::Full,
        };
        journal
            .begin_approval_response(&response, session_id)
            .unwrap();
        journal.mark_response_sent(id, 13).unwrap();
        journal.acknowledge_approval(id, session_id, 14).unwrap();
        let events = journal.events_for_session(session_id).unwrap();
        assert_eq!(events.last().unwrap().occurred_at_ms, 14);
        assert!(events.iter().all(|event| event.occurred_at_ms != 0));
    }

    #[test]
    fn reopening_does_not_update_existing_immutable_event_rows() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).unwrap();
        let session_id = Uuid::now_v7();
        let event = DurableEvent {
            event_id: 0,
            journal_epoch: journal.journal_epoch().unwrap(),
            host_id: journal.host_id().unwrap(),
            session_id,
            schema_version: 1,
            kind: "original".into(),
            occurred_at_ms: 4,
            provider_ref: None,
            causation: None,
            redaction_flags: Vec::new(),
            payload: serde_json::json!({}),
        };
        journal.append_event(&event).unwrap();
        drop(journal);
        let reopened = Journal::open(&path).unwrap();
        assert_eq!(
            reopened.events_for_session(session_id).unwrap()[0].kind,
            "original"
        );
        assert!(
            reopened
                .0
                .execute("UPDATE events SET kind='changed'", [])
                .is_err()
        );
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
