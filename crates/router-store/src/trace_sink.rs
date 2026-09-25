//! The trace sink (spec §6, DESIGN §12.6): one append-only JSONL line per
//! request under `<trace.dir>`, rolled **hourly** to
//! `YYYY-MM-DDTHH.jsonl` (UTC) — the only rollover v0.1 accepts
//! (§12.5: any other value is a load error).
//!
//! A write failure **never blocks the request** (spec §8): `write`
//! returns `Ok(None)` and the caller records
//! `errors[].kind = trace_write_failed`. The line offset of the appended
//! record is the `"<file>:<line>"` pointer the accounting rows carry as
//! `trace_ref` (DESIGN §12.10.5 note R2: the trace line precedes
//! `cost.computed` / `quota.charged`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use router_core::trace::DecisionRecord;

/// One appended line's durable pointer: the file name (under `trace.dir`)
/// and the 1-based line number, the `"file:line"` form of `trace_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceRef {
    pub file: String,
    pub line: u64,
}

impl TraceRef {
    /// The `"file:line"` wire form stored in `events.trace_ref`.
    pub fn as_pointer(&self) -> String {
        format!("{}:{}", self.file, self.line)
    }
}

/// The hourly sink. One open file at a time; the hour is read per write
/// (the hour is the file-name dimension, not a content input — AGENTS
/// constraint 2 governs content, which stays a pure function of the
/// request's own facts).
pub struct TraceSink {
    dir: PathBuf,
    inner: Mutex<Inner>,
}

struct Inner {
    hour: Option<String>,
    file: Option<File>,
    /// Lines written to the current file so far (1-based next line).
    lines: u64,
}

impl TraceSink {
    /// Creates the sink (and the directory). Failure is a startup refusal:
    /// a router that cannot write its own analysis truth must not serve
    /// (the §8 non-blocking clause covers per-request failures, not a
    /// missing directory at startup — same line as CONF-23a).
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            inner: Mutex::new(Inner {
                hour: None,
                file: None,
                lines: 0,
            }),
        })
    }

    /// Appends one record. `Ok(Some(ref))` on success; `Ok(None)` on a
    /// write failure (the request continues, spec §8 — the caller records
    /// `trace_write_failed`); `Err` only for a poisoned lock, which is a
    /// process defect, not a degradation path.
    pub fn write(&self, rec: &DecisionRecord) -> Result<Option<TraceRef>, String> {
        let mut inner = self.inner.lock().map_err(|e| e.to_string())?;
        let hour = current_hour_stamp();
        if inner.hour.as_deref() != Some(hour.as_str()) {
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.dir.join(format!("{hour}.jsonl")));
            match file {
                Ok(f) => {
                    inner.hour = Some(hour.clone());
                    inner.file = Some(f);
                    inner.lines = 0;
                }
                Err(_) => return Ok(None),
            }
        }
        let Some(file) = inner.file.as_mut() else {
            return Ok(None);
        };
        let mut line = serde_json::to_string(rec).map_err(|e| e.to_string())?;
        line.push('\n');
        if file.write_all(line.as_bytes()).is_err() {
            return Ok(None);
        }
        if file.flush().is_err() {
            return Ok(None);
        }
        inner.lines += 1;
        Ok(Some(TraceRef {
            file: format!("{hour}.jsonl"),
            line: inner.lines,
        }))
    }
}

/// `YYYY-MM-DDTHH` in UTC from the wall clock (civil-from-days, the same
/// algorithm router-core's peak module uses — no chrono dependency).
fn current_hour_stamp() -> String {
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    let secs = now_us.div_euclid(1_000_000);
    let hour = secs.div_euclid(3600);
    let (y, m, d, h) = civil_hour(hour);
    format!("{y:04}-{m:02}-{d:02}T{h:02}")
}

/// Hour index since the epoch → (year, month, day, hour-of-day), UTC.
fn civil_hour(hour: i64) -> (i64, u32, u32, u32) {
    let days = hour.div_euclid(24);
    let h = hour.rem_euclid(24) as u32;
    // Howard Hinnant's civil_from_days, inlined (router-core::peak keeps
    // its own copy private; the store keeps this one local for the same
    // dependency-discipline reason).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, h)
}

impl router_core::trace::TraceWriter for TraceSink {
    /// The core seam's adapter: the sink's `TraceRef` becomes the
    /// `"file:line"` pointer string the accounting rows store.
    fn write(&self, rec: &DecisionRecord) -> Result<Option<String>, String> {
        match TraceSink::write(self, rec)? {
            Some(r) => Ok(Some(r.as_pointer())),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use router_core::trace::{
        CostRec, DecisionRec, DecisionRecord, IdentityRec, PrefixRec, ProtocolRec, ResultRec,
        StateRec, TRACE_SCHEMA_VERSION,
    };
    use router_core::{Currency, Nano, Usage};

    fn rec(request_id: &str, event_id: i64) -> DecisionRecord {
        DecisionRecord {
            schema_version: TRACE_SCHEMA_VERSION,
            ts: "2026-09-19T07:41:02.123Z".into(),
            config_digest: "0123456789abcdef".into(),
            identity: IdentityRec {
                request_id: request_id.into(),
                event_id,
                client: "other",
                session: Some("sess-1".into()),
                thread_id: None,
                turn_index: 1,
            },
            protocol: ProtocolRec {
                protocol_in: "chat".into(),
                protocol_out: Some("chat".into()),
                translated: false,
                lossy: Vec::new(),
            },
            decision: DecisionRec {
                provider: "zai".into(),
                model: "glm-5.3".into(),
                requested_model: Some("zai/glm-5.3".into()),
                selection_source: "explicit".into(),
                plugin_chain: Vec::new(),
                decision_ms: 0,
            },
            state: StateRec {
                stateful_inbound: false,
                sticky_hit: false,
                cache_control_breaks: 0,
            },
            prefix: PrefixRec {
                blocks: Vec::new(),
                continuity: None,
            },
            transform_mode: router_core::transform::TransformMode::Passthrough,
            transforms: Vec::new(),
            usage: Usage::default(),
            usage_missing: false,
            cost: CostRec {
                input_miss: Nano(0),
                input_hit: Nano(0),
                cache_write: Nano(0),
                output: Nano(0),
                peak_applied_pct: 100,
                total: Nano(0),
                currency: Currency::Usd,
                quota_after: None,
            },
            result: ResultRec {
                status: 200,
                upstream_status: Some(200),
                failover_from: None,
                plan_switch: None,
                overhead_ms: 0,
                upstream_ms: None,
            },
            errors: Vec::new(),
        }
    }

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trace-sink-{}-{}-{tag}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_lines_and_advances_pointers() {
        let dir = tempdir("lines");
        let sink = TraceSink::open(&dir).unwrap();
        let a = sink.write(&rec("req-1", 1)).unwrap().unwrap();
        let b = sink.write(&rec("req-2", 2)).unwrap().unwrap();
        assert_eq!(a.line, 1);
        assert_eq!(b.line, 2);
        assert_eq!(a.file, b.file, "same hour, same file");
        assert!(a.file.ends_with(".jsonl"));
        assert_eq!(a.as_pointer(), format!("{}:1", a.file));
        // The file on disk holds exactly two lines.
        let written = std::fs::read_to_string(dir.join(&a.file)).unwrap();
        assert_eq!(written.lines().count(), 2);
        assert!(written.ends_with('\n'));
        // Each line is one JSON object carrying the join key.
        for (i, line) in written.lines().enumerate() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["identity"]["event_id"], (i + 1) as i64);
        }
    }

    #[test]
    fn write_failure_returns_none_never_panics() {
        // The directory is replaced by a file after open: every write fails
        // with Ok(None) — the request path must survive a dead trace sink.
        let dir = tempdir("dead");
        let sink = TraceSink::open(&dir).unwrap();
        let target = dir.join(format!("{}.jsonl", current_hour_stamp()));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::write(&target.parent().unwrap(), b"not a dir").ok();
        let out = sink.write(&rec("req-1", 1));
        assert!(
            out.is_ok(),
            "a trace write failure must not surface as an error"
        );
        assert_eq!(out.unwrap(), None);
    }

    #[test]
    fn hour_stamp_is_utc_shaped() {
        let s = current_hour_stamp();
        assert_eq!(s.len(), 13);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[10..11], "T");
        // A known epoch hour: 2026-09-19T07 → hour index 496599... verify
        // via the algorithm's own round trip instead of a magic constant
        // (constraint 6: assert relations, not snapshots).
        let (y, m, d, h) = civil_hour(0);
        assert_eq!((y, m, d, h), (1970, 1, 1, 0));
        let (y, m, d, h) = civil_hour(24);
        assert_eq!((y, m, d, h), (1970, 1, 2, 0));
    }
}
