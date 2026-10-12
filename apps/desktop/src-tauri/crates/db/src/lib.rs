//! Shop Server database access. The SQL it runs is also tested by tools/test_sqlite_ledger.py.
//!
//! Rules: only the server process opens this file; all writes go through one writer thread;
//! migrations run inside a transaction after a backup.

pub mod pairing;
pub mod roles;
pub mod users;
pub use rusqlite;

use rusqlite::Connection;
use std::{path::Path, sync::mpsc, thread};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("writer thread stopped")]
    WriterStopped,
}

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Append new migrations here; versions must be strictly increasing.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "foundation",
        sql: include_str!("../migrations/0001_foundation.sql"),
    },
    Migration {
        version: 2,
        name: "pairing_attempts",
        sql: include_str!("../migrations/0002_pairing_attempts.sql"),
    },
    Migration {
        version: 3,
        name: "entitlements",
        sql: include_str!("../migrations/0003_entitlements.sql"),
    },
    Migration {
        version: 4,
        name: "users_sessions",
        sql: include_str!("../migrations/0004_users_sessions.sql"),
    },
];

pub fn open(path: &Path) -> Result<Connection, DbError> {
    let conn = Connection::open(path)?;
    configure(&conn)?;
    Ok(conn)
}

pub fn open_in_memory() -> Result<Connection, DbError> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<(), DbError> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(())
}

pub fn current_version(conn: &Connection) -> Result<i64, DbError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    Ok(conn.query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))?)
}

/// Write a consistent copy of the database. Call this before `migrate` on a file database.
pub fn backup_to(conn: &Connection, dest: &Path) -> Result<(), DbError> {
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()])?;
    Ok(())
}

/// Apply pending migrations, each in its own transaction. Returns the versions applied.
pub fn migrate(conn: &mut Connection) -> Result<Vec<i64>, DbError> {
    let current = current_version(conn)?;
    let mut applied = Vec::new();
    for m in MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.commit()?;
        applied.push(m.version);
    }
    Ok(applied)
}

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// Single-writer queue: every write runs on one thread, in order, so two cashiers can never
/// interleave a stock check and a deduction.
pub struct Writer {
    tx: mpsc::Sender<Job>,
}

impl Writer {
    pub fn spawn(mut conn: Connection) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                job(&mut conn);
            }
        });
        Writer { tx }
    }

    pub fn run<T, F>(&self, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let (rtx, rrx) = mpsc::sync_channel(1);
        self.tx
            .send(Box::new(move |c| {
                let _ = rtx.send(f(c));
            }))
            .map_err(|_| DbError::WriterStopped)?;
        rrx.recv().map_err(|_| DbError::WriterStopped)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_once_and_is_idempotent() {
        let mut c = open_in_memory().unwrap();
        assert_eq!(migrate(&mut c).unwrap(), vec![1, 2, 3, 4]);
        assert_eq!(current_version(&c).unwrap(), 4);
        assert!(migrate(&mut c).unwrap().is_empty());
    }

    #[test]
    fn writer_serializes_jobs_and_rejects_overdraw() {
        let mut c = open_in_memory().unwrap();
        migrate(&mut c).unwrap();
        c.execute_batch(
            "INSERT INTO tenants(id,name) VALUES('t','T');
             INSERT INTO stock_locations(id,tenant_id,name,code) VALUES('l','t','Main','M');
             INSERT INTO products(id,tenant_id,sku,name) VALUES('p','t','S','P');",
        )
        .unwrap();
        let w = Writer::spawn(c);
        let ins = |delta: i64, key: &'static str| {
            w.run(move |c| {
                c.execute(
                    "INSERT INTO stock_movements(id,tenant_id,location_id,product_id,qty_delta,movement_type,source_type,source_id,idempotency_key)
                     VALUES (?1,'t','l','p',?2,'test','test',?1,?1)",
                    rusqlite::params![key, delta],
                )?;
                Ok(())
            })
        };
        assert!(ins(10, "a").is_ok());
        assert!(ins(-11, "b").is_err());
        assert!(ins(10, "a").is_err()); // duplicate key
    }
}
