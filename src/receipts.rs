//! Durable, bounded retry suppression. No message bodies or bearer tokens are stored.
//! A reservation without a completion is uncertain, not safe to retry automatically.
use crate::domain::{Failure, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::{path::Path, sync::Mutex, time::Duration};
const RETENTION: u64 = 24 * 60 * 60;
const MAX_RECORDS: i64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub request_key: String,
    pub outcome: String,
    pub replayed: bool,
    pub retry_window_seconds: u64,
    pub recipient_consumption: String,
}
pub enum Reservation {
    New,
    Existing(Receipt),
}
pub struct Receipts {
    connection: Mutex<Connection>,
}
impl Receipts {
    pub fn open(path: &Path) -> Result<Self> {
        if path.exists() {
            let m = std::fs::symlink_metadata(path).map_err(|_| db_error())?;
            if !m.is_file() || m.permissions().mode() & 0o077 != 0 {
                return Err(db_error());
            }
        }
        let conn = Connection::open(path).map_err(|_| db_error())?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| db_error())?;
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(|_| db_error())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS sends (
                principal TEXT NOT NULL, request_key TEXT NOT NULL, fingerprint TEXT NOT NULL,
                outcome TEXT NOT NULL, expires INTEGER NOT NULL,
                PRIMARY KEY(principal,request_key));",
        )
        .map_err(|_| db_error())?;
        Ok(Self {
            connection: Mutex::new(conn),
        })
    }
    pub fn existing(
        &self,
        principal: &str,
        key: &str,
        fingerprint: &str,
        now: u64,
    ) -> Result<Option<Receipt>> {
        let conn = self.connection.lock().map_err(|_| db_error())?;
        let found:Option<(String,String)>=conn.query_row(
            "SELECT fingerprint,outcome FROM sends WHERE principal=?1 AND request_key=?2 AND expires>?3",
            params![principal,key,now],|r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_| db_error())?;
        found
            .map(|(f, o)| match_record(key, fingerprint, &f, &o))
            .transpose()
    }
    /// Commits uncertainty BEFORE dispatching the external effect.
    pub fn reserve(
        &self,
        principal: &str,
        key: &str,
        fingerprint: &str,
        now: u64,
    ) -> Result<Reservation> {
        let mut conn = self.connection.lock().map_err(|_| db_error())?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| db_error())?;
        tx.execute("DELETE FROM sends WHERE expires<=?1", params![now])
            .map_err(|_| db_error())?;
        let found: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint,outcome FROM sends WHERE principal=?1 AND request_key=?2",
                params![principal, key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|_| db_error())?;
        if let Some((f, o)) = found {
            return Ok(Reservation::Existing(match_record(
                key,
                fingerprint,
                &f,
                &o,
            )?));
        }
        let count: i64 = tx
            .query_row("SELECT COUNT(*) FROM sends", [], |r| r.get(0))
            .map_err(|_| db_error())?;
        if count >= MAX_RECORDS {
            return Err(Failure::new(
                "receipt_capacity",
                "Retry receipt storage is full; no new input was dispatched.",
            ));
        }
        tx.execute("INSERT INTO sends(principal,request_key,fingerprint,outcome,expires) VALUES(?1,?2,?3,'outcome_unknown',?4)",params![principal,key,fingerprint,now+RETENTION]).map_err(|_| db_error())?;
        tx.commit().map_err(|_| db_error())?;
        Ok(Reservation::New)
    }
    pub fn accepted(&self, principal: &str, key: &str) -> Result<()> {
        let conn = self.connection.lock().map_err(|_| db_error())?;
        let changed=conn.execute("UPDATE sends SET outcome='accepted_by_client' WHERE principal=?1 AND request_key=?2",params![principal,key]).map_err(|_| db_error())?;
        if changed != 1 {
            return Err(db_error());
        }
        Ok(())
    }
}
fn match_record(key: &str, expected: &str, actual: &str, outcome: &str) -> Result<Receipt> {
    if expected != actual {
        return Err(Failure::new(
            "request_key_conflict",
            "This request key was already used for different input.",
        ));
    }
    Ok(receipt(key, outcome, true))
}
pub fn receipt(key: &str, outcome: &str, replayed: bool) -> Receipt {
    Receipt {
        request_key: key.into(),
        outcome: outcome.into(),
        replayed,
        retry_window_seconds: RETENTION,
        recipient_consumption: "unknown".into(),
    }
}
fn db_error() -> Failure {
    Failure::new(
        "receipt_storage",
        "Receipt storage is unavailable; do not assume an external effect did not occur.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_retains_uncertainty_and_rejects_conflicts() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("receipts.sqlite");
        {
            let db = Receipts::open(&path).unwrap();
            assert!(matches!(
                db.reserve("p", "key", "hash", 100).unwrap(),
                Reservation::New
            ));
        }
        {
            let db = Receipts::open(&path).unwrap();
            assert_eq!(
                db.existing("p", "key", "hash", 101)
                    .unwrap()
                    .unwrap()
                    .outcome,
                "outcome_unknown"
            );
            assert!(db.reserve("p", "key", "different", 101).is_err());
            db.accepted("p", "key").unwrap();
            assert_eq!(
                db.existing("p", "key", "hash", 102)
                    .unwrap()
                    .unwrap()
                    .outcome,
                "accepted_by_client"
            );
            assert!(db
                .existing("another", "key", "hash", 102)
                .unwrap()
                .is_none());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
