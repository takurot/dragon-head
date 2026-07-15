//! Immutable append-only audit trail of HITL resolutions.
//!
//! Spec ACT-05 requires every approval decision to be traceable to "approver
//! user ID, timestamp, and Outcome Projection data". Rather than widen the
//! shared `core_runtime::AuditEvent` enum for a reference-only consumer, the
//! bridge keeps its own NDJSON log — one immutable JSON record per line,
//! opened append-only and `fsync`'d on every write.

use anyhow::{Context, Result};
use core_runtime::OutcomeProjection;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use uuid::Uuid;

use crate::lock::Decision;

const MAX_RECENT_AUDIT_RECORDS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    Approved,
    Rejected,
}

impl From<Decision> for AuditDecision {
    fn from(decision: Decision) -> Self {
        match decision {
            Decision::Approved => AuditDecision::Approved,
            Decision::Rejected => AuditDecision::Rejected,
        }
    }
}

/// One immutable record of a resolved HITL approval request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: Uuid,
    pub decision: AuditDecision,
    pub decided_by: String,
    pub decided_at_ms: u64,
    pub outcome_projection: Option<OutcomeProjection>,
}

/// Append-only NDJSON audit trail.
///
/// Each [`record`](Self::record) call appends exactly one JSON line and
/// `fsync`s before returning, so a crash never leaves a torn or missing entry.
pub struct BridgeAuditTrail {
    path: PathBuf,
    state: Mutex<AuditState>,
}

#[derive(Default)]
struct AuditState {
    initialized: bool,
    recent_records: VecDeque<AuditRecord>,
}

impl AuditState {
    fn remember(&mut self, record: AuditRecord) {
        self.recent_records.push_back(record);
        while self.recent_records.len() > MAX_RECENT_AUDIT_RECORDS {
            self.recent_records.pop_front();
        }
    }
}

impl BridgeAuditTrail {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            state: Mutex::new(AuditState::default()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `record` as a single NDJSON line, fsync'd before returning.
    /// Replaying the same request ID with an identical record is idempotent;
    /// reusing an ID for different decision data is rejected.
    pub fn record(&self, record: &AuditRecord) -> Result<()> {
        let line = serde_json::to_string(record).context("failed to serialize audit record")?;

        let mut state = self.state.lock().expect("audit trail mutex poisoned");
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open audit trail at {}", self.path.display()))?;

        if !state.initialized {
            for existing_line in BufReader::new(&file).lines() {
                let existing_line = existing_line.context("failed to inspect audit trail")?;
                if existing_line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<AuditRecord>(&existing_line) {
                    Ok(existing) => state.remember(existing),
                    Err(error) => tracing::warn!(
                        %error,
                        "ignoring malformed historical HITL audit record"
                    ),
                }
            }
            state.initialized = true;
        }

        if let Some(existing) = state
            .recent_records
            .iter()
            .find(|existing| existing.id == record.id)
        {
            if existing != record {
                anyhow::bail!(
                    "audit record {} conflicts with an existing decision",
                    record.id
                );
            }
            file.sync_all().context("failed to fsync audit trail")?;
            return Ok(());
        }

        writeln!(file, "{line}").context("failed to write audit record")?;
        state.remember(record.clone());
        file.sync_all().context("failed to fsync audit trail")?;
        Ok(())
    }

    /// Read back every record currently in the trail, in append order.
    ///
    /// Intended for tests and operator inspection — the bridge itself is
    /// write-only.
    pub fn read_all(&self) -> Result<Vec<AuditRecord>> {
        let _guard = self.state.lock().expect("audit trail mutex poisoned");
        let contents = match std::fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to read audit trail at {}", self.path.display())
                })
            }
        };

        contents
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).context("failed to parse audit record"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_runtime::RiskLevel;
    use tempfile::tempdir;

    fn sample_record(id: Uuid, decision: AuditDecision) -> AuditRecord {
        AuditRecord {
            id,
            decision,
            decided_by: "U12345".to_string(),
            decided_at_ms: 1_700_000_000_000,
            outcome_projection: Some(OutcomeProjection {
                projected_amount: Some(900.0),
                risk_level: RiskLevel::High,
            }),
        }
    }

    #[test]
    fn record_then_read_all_round_trips_exactly() {
        let dir = tempdir().expect("tempdir");
        let trail = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
        let record = sample_record(Uuid::new_v4(), AuditDecision::Approved);

        trail.record(&record).expect("record should succeed");
        let read_back = trail.read_all().expect("read_all should succeed");

        assert_eq!(read_back, vec![record]);
    }

    #[test]
    fn each_record_appends_one_ndjson_line() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("audit.ndjson");
        let trail = BridgeAuditTrail::new(&path);

        trail
            .record(&sample_record(Uuid::new_v4(), AuditDecision::Approved))
            .expect("first record");
        trail
            .record(&sample_record(Uuid::new_v4(), AuditDecision::Rejected))
            .expect("second record");

        let contents = std::fs::read_to_string(&path).expect("read file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
            assert!(parsed.get("id").is_some());
            assert!(parsed.get("decided_by").is_some());
            assert!(parsed.get("outcome_projection").is_some());
        }
    }

    #[test]
    fn recording_the_same_decision_twice_is_idempotent() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("audit.ndjson");
        let trail = BridgeAuditTrail::new(&path);
        let record = sample_record(Uuid::new_v4(), AuditDecision::Approved);

        trail.record(&record).expect("first record");
        trail.record(&record).expect("idempotent retry");

        assert_eq!(trail.read_all().expect("read audit trail"), vec![record]);
    }

    #[test]
    fn malformed_history_does_not_block_new_audit_records() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("audit.ndjson");
        std::fs::write(&path, "{truncated\n").expect("write malformed history");
        let trail = BridgeAuditTrail::new(&path);
        let record = sample_record(Uuid::new_v4(), AuditDecision::Approved);

        trail
            .record(&record)
            .expect("malformed history must not block a new decision");

        let contents = std::fs::read_to_string(path).expect("read audit trail");
        assert_eq!(contents.lines().count(), 2);
        assert_eq!(
            serde_json::from_str::<AuditRecord>(contents.lines().nth(1).expect("new record"))
                .expect("valid appended record"),
            record
        );
    }

    #[test]
    fn in_memory_audit_index_is_bounded() {
        let dir = tempdir().expect("tempdir");
        let trail = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));

        for _ in 0..=MAX_RECENT_AUDIT_RECORDS {
            trail
                .record(&sample_record(Uuid::new_v4(), AuditDecision::Approved))
                .expect("record");
        }

        assert_eq!(
            trail
                .state
                .lock()
                .expect("audit trail mutex")
                .recent_records
                .len(),
            MAX_RECENT_AUDIT_RECORDS
        );
    }

    #[test]
    fn read_all_on_missing_file_returns_empty() {
        let dir = tempdir().expect("tempdir");
        let trail = BridgeAuditTrail::new(dir.path().join("does-not-exist.ndjson"));

        assert_eq!(
            trail.read_all().expect("read_all should succeed"),
            Vec::new()
        );
    }

    #[test]
    fn growth_is_append_only_and_preserves_order() {
        let dir = tempdir().expect("tempdir");
        let trail = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));

        let first = sample_record(Uuid::new_v4(), AuditDecision::Approved);
        let second = sample_record(Uuid::new_v4(), AuditDecision::Rejected);
        trail.record(&first).expect("first record");
        trail.record(&second).expect("second record");

        let read_back = trail.read_all().expect("read_all should succeed");
        assert_eq!(read_back, vec![first, second]);
    }
}
