use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;
use uuid::Uuid;

#[allow(dead_code)]
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[error("worktree write lease for {workdir:?} held by session {holder}")]
pub struct LeaseConflictError {
    pub holder: Uuid,
    pub workdir: PathBuf,
    pub acquired_at_ms: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseEntry {
    pub session_id: Uuid,
    pub workdir: PathBuf,
    pub acquired_at_ms: i64,
}

#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct WorkspaceLeaseManager {
    leases: Mutex<HashMap<PathBuf, LeaseEntry>>,
}

#[allow(dead_code)]
impl WorkspaceLeaseManager {
    pub fn new() -> Self {
        Self {
            leases: Mutex::new(HashMap::new()),
        }
    }

    fn normalize_path(path: &Path) -> PathBuf {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }

    pub fn check_conflict(
        &self,
        session_id: Uuid,
        workdir: &Path,
    ) -> Result<(), LeaseConflictError> {
        let canonical = Self::normalize_path(workdir);
        let guard = self.leases.lock().expect("workspace lease lock poisoned");

        if let Some(existing) = guard.get(&canonical)
            && existing.session_id != session_id
        {
            return Err(LeaseConflictError {
                holder: existing.session_id,
                workdir: canonical,
                acquired_at_ms: existing.acquired_at_ms,
            });
        }
        Ok(())
    }

    pub fn try_acquire(
        &self,
        session_id: Uuid,
        workdir: &Path,
        now_ms: i64,
    ) -> Result<(), LeaseConflictError> {
        let canonical = Self::normalize_path(workdir);
        let mut guard = self.leases.lock().expect("workspace lease lock poisoned");

        if let Some(existing) = guard.get(&canonical) {
            if existing.session_id == session_id {
                return Ok(());
            }
            return Err(LeaseConflictError {
                holder: existing.session_id,
                workdir: canonical,
                acquired_at_ms: existing.acquired_at_ms,
            });
        }

        guard.insert(
            canonical.clone(),
            LeaseEntry {
                session_id,
                workdir: canonical,
                acquired_at_ms: now_ms,
            },
        );
        Ok(())
    }

    pub fn release(&self, session_id: Uuid, workdir: &Path) -> bool {
        let canonical = Self::normalize_path(workdir);
        let mut guard = self.leases.lock().expect("workspace lease lock poisoned");
        if let Some(entry) = guard.get(&canonical)
            && entry.session_id == session_id
        {
            guard.remove(&canonical);
            return true;
        }
        false
    }

    pub fn release_all_for_session(&self, session_id: Uuid) -> Vec<PathBuf> {
        let mut guard = self.leases.lock().expect("workspace lease lock poisoned");
        let to_remove: Vec<PathBuf> = guard
            .iter()
            .filter(|(_, entry)| entry.session_id == session_id)
            .map(|(path, _)| path.clone())
            .collect();

        for path in &to_remove {
            guard.remove(path);
        }
        to_remove
    }

    pub fn holder(&self, workdir: &Path) -> Option<Uuid> {
        let canonical = Self::normalize_path(workdir);
        let guard = self.leases.lock().expect("workspace lease lock poisoned");
        guard.get(&canonical).map(|entry| entry.session_id)
    }

    pub fn is_held_by(&self, session_id: Uuid, workdir: &Path) -> bool {
        self.holder(workdir) == Some(session_id)
    }

    pub fn active_leases_count(&self) -> usize {
        let guard = self.leases.lock().expect("workspace lease lock poisoned");
        guard.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_acquire_and_release_single_session() {
        let manager = WorkspaceLeaseManager::new();
        let session = Uuid::now_v7();
        let dir = tempdir().unwrap();

        assert!(manager.try_acquire(session, dir.path(), 1000).is_ok());
        assert_eq!(manager.holder(dir.path()), Some(session));
        assert!(manager.is_held_by(session, dir.path()));

        // Re-acquiring by same session is idempotent
        assert!(manager.try_acquire(session, dir.path(), 1050).is_ok());

        // Release
        assert!(manager.release(session, dir.path()));
        assert_eq!(manager.holder(dir.path()), None);
        assert!(!manager.is_held_by(session, dir.path()));
    }

    #[test]
    fn test_collision_refused() {
        let manager = WorkspaceLeaseManager::new();
        let session1 = Uuid::now_v7();
        let session2 = Uuid::now_v7();
        let dir = tempdir().unwrap();

        assert!(manager.try_acquire(session1, dir.path(), 1000).is_ok());

        let err = manager.try_acquire(session2, dir.path(), 2000).unwrap_err();
        assert_eq!(err.holder, session1);
        assert_eq!(err.acquired_at_ms, 1000);

        // Session 2 releasing does nothing
        assert!(!manager.release(session2, dir.path()));
        assert_eq!(manager.holder(dir.path()), Some(session1));

        // Session 1 releasing allows Session 2 to acquire
        assert!(manager.release(session1, dir.path()));
        assert!(manager.try_acquire(session2, dir.path(), 3000).is_ok());
        assert_eq!(manager.holder(dir.path()), Some(session2));
    }

    #[test]
    fn test_release_all_for_session() {
        let manager = WorkspaceLeaseManager::new();
        let session = Uuid::now_v7();
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();

        assert!(manager.try_acquire(session, dir1.path(), 100).is_ok());
        assert!(manager.try_acquire(session, dir2.path(), 100).is_ok());
        assert_eq!(manager.active_leases_count(), 2);

        let released = manager.release_all_for_session(session);
        assert_eq!(released.len(), 2);
        assert_eq!(manager.active_leases_count(), 0);
    }
}
