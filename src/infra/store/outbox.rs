//! The send side of `state.db`: what is waiting, and what one answer from the server changes.
use super::{Result, Store};
use rusqlite::{TransactionBehavior, params};

/// One buffered observation, as stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub id: i64,
    pub payload: String,
    /// Unix ms of when it entered the buffer.
    pub queued_at: i64,
}

/// What is read for one batch: observations and coverage reports, oldest first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub items: Vec<Queued>,
    /// `(id, payload)`.
    pub coverage: Vec<(i64, String)>,
}

/// Everything one answer changes, applied in one transaction: either all of it or none.
#[derive(Clone, Debug, Default)]
pub struct Change {
    pub remove_items: Vec<i64>,
    /// Back to the end of the queue with the same age.
    pub requeue: Vec<Queued>,
    pub remove_coverage: Vec<i64>,
    /// Counts to add by code.
    pub rejected: Vec<(String, u64)>,
    /// Drop every buffered observation of this (source, external session id).
    pub discard_session: Option<(String, String)>,
}

impl Store {
    /// The oldest observations of the given sources with `id <= up_to`, at most `limit`.
    /// `up_to` keeps an item put back this run from being taken again in the same run.
    pub fn queued(&self, sources: &[&str], up_to: i64, limit: usize) -> Result<Vec<Queued>> {
        let mut out = Vec::new();
        let mut q = self.conn.prepare_cached(
            "SELECT id, payload, queued_at FROM spool
             WHERE id <= ?1 AND json_extract(payload, '$.source') = ?2 ORDER BY id LIMIT ?3",
        )?;
        // One query per source, merged by id: the list of sources is two names at most.
        for source in sources {
            let rows = q.query_map(params![up_to, source, limit as i64], |r| {
                Ok(Queued {
                    id: r.get(0)?,
                    payload: r.get(1)?,
                    queued_at: r.get(2)?,
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        out.sort_by_key(|i| i.id);
        out.truncate(limit);
        Ok(out)
    }

    /// The oldest coverage reports of the given sources, at most `limit`.
    pub fn queued_coverage(&self, sources: &[&str], limit: usize) -> Result<Vec<(i64, String)>> {
        let mut out = Vec::new();
        let mut q = self.conn.prepare_cached(
            "SELECT id, payload FROM pending_coverage
             WHERE json_extract(payload, '$.source') = ?1 ORDER BY id LIMIT ?2",
        )?;
        for source in sources {
            let rows = q.query_map(params![source, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        out.sort_by_key(|c| c.0);
        out.truncate(limit);
        Ok(out)
    }

    /// How many observations of a source are buffered.
    pub fn queued_count(&self, source: &str) -> Result<u64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM spool WHERE json_extract(payload, '$.source') = ?1",
            [source],
            |r| r.get(0),
        )?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// The highest id ever given in the buffer: the end of the queue as this run found it.
    /// Anything put back later gets a higher one.
    pub fn queue_end(&self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'spool'), 0)",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn apply(&mut self, change: &Change) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        {
            let mut delete = tx.prepare_cached("DELETE FROM spool WHERE id = ?1")?;
            for id in &change.remove_items {
                delete.execute([id])?;
            }
            // Put back after the end, in the order they had, keeping how old they are.
            let mut insert =
                tx.prepare_cached("INSERT INTO spool(payload, queued_at) VALUES(?1, ?2)")?;
            for item in &change.requeue {
                delete.execute([item.id])?;
                insert.execute(params![item.payload, item.queued_at])?;
            }
            let mut delete = tx.prepare_cached("DELETE FROM pending_coverage WHERE id = ?1")?;
            for id in &change.remove_coverage {
                delete.execute([id])?;
            }
            let mut count = tx.prepare_cached(
                "INSERT INTO rejected(code, count) VALUES(?1, ?2)
                 ON CONFLICT(code) DO UPDATE SET count = count + excluded.count",
            )?;
            for (code, n) in &change.rejected {
                count.execute(params![code, *n as i64])?;
            }
        }
        if let Some((source, session)) = &change.discard_session {
            tx.execute(
                "DELETE FROM spool WHERE json_extract(payload, '$.source') = ?1
                   AND json_extract(payload, '$.externalSessionId') = ?2",
                params![source, session],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn sync_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_state(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn sync_delete(&self, keys: &[&str]) -> Result<()> {
        for key in keys {
            self.conn
                .execute("DELETE FROM sync_state WHERE key = ?1", [key])?;
        }
        Ok(())
    }
}
