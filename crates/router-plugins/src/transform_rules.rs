//! The `builtin/transform_rules` engine (ADR-019 §7 item 1, DESIGN §12.12
//! order ④): the landed rule-file format of `rules/tool_output.toml`,
//! implemented as data — text in, text out, per payload node.
//!
//! Acceptance test = the rule file's own 13 inline tests: the semantics
//! spec in the file's header is normative ("on mismatch, suspect the
//! implementation first and the spec text second"), and a rule whose
//! inline tests fail **does not load** (fail-safe: the payload passes
//! through verbatim; its siblings load as usual — the rule file's hard
//! constraint 3, spec §4.4).
//!
//! Content determinism (AGENTS hard constraint 2) holds by construction:
//! `apply` is a pure function of (rule set, node text); the only inputs
//! are the compiled rule table and the payload. No clock, no turn index,
//! no session, no RNG, no environment. Prefix monotonicity (I2) holds by
//! the per-node rule: every already-present node re-derives the same
//! bytes on every turn.
//!
//! Selection is by declaration, never by sniffing content (ADR-019 §3):
//! `match_tool` is a regex over the wire's own paired tool name,
//! `match_kind` intersects the declared tool→kind table
//! (`router_core::transform::TOOL_KINDS`, GAP-Q18's settlement). Both
//! given ⇒ both must match; neither given ⇒ wildcard.

use std::path::Path;

use regex::Regex;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use router_core::transform::{PayloadCtx, TransformEngine, TransformOutcome};

// ---------------------------------------------------------------------------
// The rule file, as data (the shape of `[filters.<id>]` / `[[tests.<id>]]`)
// ---------------------------------------------------------------------------

/// One `[filters.<id>]` table, exactly as the landed format allows it
/// (`rules/tool_output.toml`'s semantics spec). Unknown keys are a load
/// failure for the rule — the same "I changed it but it did not take
/// effect" stance as the config's `deny_unknown_fields`.
#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields, default)]
struct RuleSpec {
    description: Option<String>,
    /// The declared try order (R33-FIX, R33-F1): ascending, default 0,
    /// ties broken by id. Optional; rules without it keep the default.
    order: Option<i64>,
    match_tool: Option<String>,
    match_kind: Option<Vec<String>>,
    min_input_bytes: Option<usize>,
    strip_ansi: Option<bool>,
    json_compact: Option<bool>,
    replace: Option<Vec<ReplaceSpec>>,
    match_output: Option<Vec<String>>,
    message: Option<String>,
    unless: Option<Vec<String>>,
    strip_lines_matching: Option<Vec<String>>,
    keep_lines_matching: Option<Vec<String>>,
    truncate_lines_at: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    tee: Option<bool>,
    on_empty: Option<String>,
}

#[derive(Deserialize, Debug)]
struct ReplaceSpec {
    pattern: String,
    replacement: String,
}

/// One `[[tests.<id>]]` entry: name / input / expected all required.
#[derive(Deserialize, Debug)]
struct InlineTestSpec {
    #[allow(dead_code)]
    name: String,
    input: String,
    expected: String,
}

// ---------------------------------------------------------------------------
// The compiled rule
// ---------------------------------------------------------------------------

enum LineFilter {
    None,
    Strip(Vec<Regex>),
    Keep(Vec<Regex>),
}

/// A rule with every pattern compiled and every cross-field constraint
/// checked at load time — `apply` never fails, it only decides.
struct CompiledRule {
    id: String,
    /// The declared try order (`rules/tool_output.toml`'s semantics spec):
    /// ascending, default 0, ties broken by id — never the TOML map's
    /// order, which is alphabetical by construction (`toml`'s map is a
    /// BTreeMap) and is not a declared semantic.
    order: i64,
    match_tool: Option<Regex>,
    match_kind: Vec<String>,
    min_input_bytes: usize,
    strip_ansi: bool,
    json_compact: bool,
    replace: Vec<(Regex, String)>,
    match_output: Vec<Regex>,
    match_output_message: Option<String>,
    unless: Vec<Regex>,
    line_filter: LineFilter,
    truncate_at: Option<usize>,
    tail_lines: Option<usize>,
    max_lines: Option<usize>,
    tee: bool,
    on_empty: Option<String>,
}

fn compile_regexes(label: &str, pats: &[String]) -> Result<Vec<Regex>, String> {
    pats.iter()
        .map(|p| Regex::new(p).map_err(|e| format!("{label} pattern '{p}' does not compile: {e}")))
        .collect()
}

impl CompiledRule {
    fn compile(id: &str, spec: RuleSpec) -> Result<Self, String> {
        let line_filter = match (&spec.strip_lines_matching, &spec.keep_lines_matching) {
            (Some(_), Some(_)) => {
                return Err(
                    "strip_lines_matching and keep_lines_matching are mutually exclusive"
                        .to_string(),
                )
            }
            (Some(pats), None) => LineFilter::Strip(compile_regexes("strip_lines_matching", pats)?),
            (None, Some(pats)) => LineFilter::Keep(compile_regexes("keep_lines_matching", pats)?),
            (None, None) => LineFilter::None,
        };
        let match_output = compile_regexes("match_output", &spec.match_output.unwrap_or_default())?;
        if !match_output.is_empty() && spec.message.is_none() {
            return Err(
                "match_output requires 'message' (the literal it short-circuits to)".to_string(),
            );
        }
        let mut replace = Vec::new();
        for r in &spec.replace.unwrap_or_default() {
            let re = Regex::new(&r.pattern)
                .map_err(|e| format!("replace pattern '{}' does not compile: {e}", r.pattern))?;
            replace.push((re, r.replacement.clone()));
        }
        let match_tool = match &spec.match_tool {
            Some(p) => {
                Some(Regex::new(p).map_err(|e| format!("match_tool '{p}' does not compile: {e}"))?)
            }
            None => None,
        };
        Ok(Self {
            id: id.to_string(),
            order: spec.order.unwrap_or(0),
            match_tool,
            match_kind: spec.match_kind.unwrap_or_default(),
            min_input_bytes: spec.min_input_bytes.unwrap_or(0),
            strip_ansi: spec.strip_ansi.unwrap_or(false),
            json_compact: spec.json_compact.unwrap_or(false),
            replace,
            match_output,
            match_output_message: spec.message,
            unless: compile_regexes("unless", &spec.unless.unwrap_or_default())?,
            line_filter,
            truncate_at: spec.truncate_lines_at,
            tail_lines: spec.tail_lines,
            max_lines: spec.max_lines,
            tee: spec.tee.unwrap_or(false),
            on_empty: spec.on_empty,
        })
    }

    /// Selection by declaration (ADR-019 §3): the tool name from the wire's
    /// own pairing, the kinds from the declared table. Both given ⇒ both
    /// must match; neither given ⇒ wildcard.
    fn selects(&self, ctx: &PayloadCtx<'_>) -> bool {
        if let Some(re) = &self.match_tool {
            match ctx.tool {
                // A tool-anchored rule cannot fire on a nameless payload.
                Some(t) if re.is_match(t) => {}
                _ => return false,
            }
        }
        if !self.match_kind.is_empty() {
            let hit = self
                .match_kind
                .iter()
                .any(|k| ctx.kinds.contains(&k.as_str()));
            if !hit {
                return false;
            }
        }
        true
    }

    fn apply(&self, ctx: &PayloadCtx<'_>, text: &str) -> Option<TransformOutcome> {
        if !self.selects(ctx) {
            return None;
        }
        self.apply_text(text)
    }

    /// The stage pipeline, in the file's fixed order: strip_ansi →
    /// json_compact → replace → match_output → strip/keep_lines_matching →
    /// truncate_lines_at → tail_lines → max_lines → tee marker → on_empty.
    /// `None` = this rule does not apply to this payload (pass through).
    fn apply_text(&self, text: &str) -> Option<TransformOutcome> {
        // Precondition: an unmet byte budget leaves the payload — including
        // its original newlines — byte-for-byte alone.
        if text.len() < self.min_input_bytes {
            return None;
        }
        let original = text;
        let orig_line_count = split_lines(original).len();

        // Stage 1: strip_ansi.
        let mut cur = if self.strip_ansi {
            strip_ansi(original)
        } else {
            original.to_string()
        };

        // Stage 2: json_compact — order-preserving parse, compact to one
        // line, number literals verbatim (serde_json's arbitrary_precision
        // keeps the literal text). Parse failure ⇒ the rule does not apply
        // (fail-safe: no partial edit).
        if self.json_compact {
            match serde_json::from_str::<serde_json::Value>(&cur) {
                Ok(v) => {
                    // to_string of the preserve_order/arbitrary_precision
                    // Value is the compact form; unwrap: Value is
                    // infallibly serializable.
                    cur = serde_json::to_string(&v).expect("Value serializes");
                    cur.push('\n');
                }
                Err(_) => return None,
            }
        }

        // Stage 3: replace — per-line replace_all, array order.
        if !self.replace.is_empty() {
            let mut out = String::with_capacity(cur.len());
            let lines: Vec<&str> = split_lines(&cur);
            for (i, line) in lines.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                let mut l = (*line).to_string();
                for (re, rep) in &self.replace {
                    l = re.replace_all(&l, rep.as_str()).into_owned();
                }
                out.push_str(&l);
            }
            out.push('\n');
            cur = out;
        }

        // Stage 4: match_output — against the full text; a hit on `unless`
        // skips the rule; the first hit short-circuits to `message`.
        for u in &self.unless {
            if u.is_match(&cur) {
                return None;
            }
        }
        if !self.match_output.is_empty() && self.match_output.iter().any(|m| m.is_match(&cur)) {
            return Some(TransformOutcome {
                rule: self.id.clone(),
                new_text: self.match_output_message.clone().unwrap_or_default(),
                cache_impact: "neutral",
                tee_id: None,
            });
        }

        // Stage 5: strip / keep lines.
        let mut lines: Vec<String> = split_lines(&cur).iter().map(|l| (*l).to_string()).collect();
        match &self.line_filter {
            LineFilter::None => {}
            LineFilter::Strip(pats) => lines.retain(|l| !pats.iter().any(|p| p.is_match(l))),
            LineFilter::Keep(pats) => lines.retain(|l| pats.iter().any(|p| p.is_match(l))),
        }

        // Stage 6: truncate_lines_at — first N characters plus `...`
        // (`...` does not count toward N); characters, not bytes.
        if let Some(n) = self.truncate_at {
            for l in lines.iter_mut() {
                if l.chars().count() > n {
                    let head: String = l.chars().take(n).collect();
                    *l = format!("{head}...");
                }
            }
        }

        // Stage 7: tail_lines (last N) then max_lines (first N), both
        // after truncation, in the file's fixed order.
        if let Some(n) = self.tail_lines {
            let drop = lines.len().saturating_sub(n);
            lines.drain(..drop);
        }
        if let Some(n) = self.max_lines {
            lines.truncate(n);
        }

        // Stage 9 (the empty case): the on_empty literal, no newline, no
        // tee marker.
        if lines.is_empty() {
            return Some(TransformOutcome {
                rule: self.id.clone(),
                new_text: self.on_empty.clone().unwrap_or_default(),
                cache_impact: "neutral",
                tee_id: None,
            });
        }

        // Stage 8: the tee marker — only when lines were dropped. The hash
        // and the byte count are of the ORIGINAL payload (the bytes that
        // entered this rule, newlines included).
        let lines_dropped = orig_line_count.saturating_sub(lines.len());
        let mut out = lines.join("\n");
        out.push('\n');
        let mut tee_id = None;
        if self.tee && lines_dropped > 0 {
            let sha = sha16(original.as_bytes());
            out.push_str(&format!(
                "[router:tee sha256={sha} lines_dropped={lines_dropped} bytes_original={}]\n",
                original.len()
            ));
            tee_id = Some(sha);
        }
        Some(TransformOutcome {
            rule: self.id.clone(),
            new_text: out,
            cache_impact: "neutral",
            tee_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Line semantics (the file's own spec, one home)
// ---------------------------------------------------------------------------

/// The payload minus its **final** newline, split on `\n`
/// (`a\nb\n` → ["a","b"]; `a\n\n` → ["a",""]).
fn split_lines(t: &str) -> Vec<&str> {
    t.strip_suffix('\n').unwrap_or(t).split('\n').collect()
}

/// First 16 hex chars of the payload's sha256 — the tee marker's identity
/// and the trace's `tee_id`. The same convention as `body_sha16`, which
/// lives behind router-core's allowlist; this crate has its own sha2 row.
fn sha16(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut out = String::with_capacity(16);
    for b in &d[..8] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// CSI escape sequences (ESC [ ... final byte) — the `strip_ansi` stage.
fn strip_ansi(t: &str) -> String {
    // Compiled once per call site is fine: the engine path calls it only
    // for rules that declared the stage, per payload node.
    static CSI: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = CSI.get_or_init(|| Regex::new("\u{1b}\\[[0-9;?]*[ -/]*[@-~]").expect("literal"));
    re.replace_all(t, "").into_owned()
}

// ---------------------------------------------------------------------------
// Loading: parse → compile → inline tests; a rule that fails any of the
// three does not load, its siblings do (fail-safe, spec §4.4)
// ---------------------------------------------------------------------------

/// Why one rule id did not load (startup log / LoadReport consumer).
#[derive(Debug, Clone)]
pub struct FailedRule {
    pub id: String,
    pub reason: String,
}

/// The load outcome the wiring reports: which rule ids are live and which
/// failed (with the reason). A disabled rule is a countable, named state —
/// never a silent absence.
#[derive(Debug, Clone, Default)]
pub struct LoadReport {
    pub loaded: Vec<String>,
    pub failed: Vec<FailedRule>,
}

impl LoadReport {
    pub fn is_all_loaded(&self) -> bool {
        self.failed.is_empty()
    }

    /// One line per failure, for the startup log (`eprintln!`).
    pub fn failure_lines(&self) -> Vec<String> {
        self.failed
            .iter()
            .map(|f| format!("transform_rules: rule '{}' not loaded: {}", f.id, f.reason))
            .collect()
    }
}

/// The engine: the compiled rules in the declared try order (ascending
/// `order`, ties alphabetical by id). The first rule that both selects and
/// produces an edit wins for a node; a rule that selects but does not
/// apply (byte budget unmet, `json_compact` parse failure, `unless` hit)
/// falls through to the next.
pub struct TransformRulesEngine {
    rules: Vec<CompiledRule>,
}

impl TransformEngine for TransformRulesEngine {
    fn id(&self) -> &'static str {
        "builtin/transform_rules"
    }

    fn apply_node(&self, ctx: &PayloadCtx<'_>, text: &str) -> Option<TransformOutcome> {
        for rule in &self.rules {
            if let Some(o) = rule.apply(ctx, text) {
                return Some(o);
            }
        }
        None
    }
}

/// Parse and admit a rule file's text. A file-level parse failure (bad
/// TOML, wrong schema_version) fails every rule with the file named — an
/// empty engine must never be mistaken for a loaded one.
pub fn load_str(text: &str) -> (TransformRulesEngine, LoadReport) {
    let mut report = LoadReport::default();
    let doc: toml::Value = match toml::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            report.failed.push(FailedRule {
                id: "<rule file>".to_string(),
                reason: format!("TOML parse error: {e}"),
            });
            return (TransformRulesEngine { rules: Vec::new() }, report);
        }
    };
    let schema = doc.get("schema_version").and_then(toml::Value::as_integer);
    if schema != Some(1) {
        report.failed.push(FailedRule {
            id: "<rule file>".to_string(),
            reason: format!("unsupported schema_version {schema:?} (expected 1)"),
        });
        return (TransformRulesEngine { rules: Vec::new() }, report);
    }

    let filters = doc.get("filters").and_then(toml::Value::as_table);
    let tests = doc.get("tests").and_then(toml::Value::as_table);
    let mut rules = Vec::new();
    let filters = match filters {
        Some(f) => f,
        None => {
            report.failed.push(FailedRule {
                id: "<rule file>".to_string(),
                reason: "no [filters.<id>] section".to_string(),
            });
            return (TransformRulesEngine { rules }, report);
        }
    };
    for (id, spec_value) in filters {
        let spec = match RuleSpec::deserialize(spec_value.clone()) {
            Ok(s) => s,
            Err(e) => {
                report.failed.push(FailedRule {
                    id: id.clone(),
                    reason: format!("bad rule table: {e}"),
                });
                continue;
            }
        };
        let rule = match CompiledRule::compile(id, spec) {
            Ok(r) => r,
            Err(reason) => {
                report.failed.push(FailedRule {
                    id: id.clone(),
                    reason,
                });
                continue;
            }
        };
        // The inline tests are the rule's only spec (ADR-008): a rule whose
        // tests fail does not load. "No edit" (None) satisfies a test whose
        // expected equals the input — the pass-through the compose step
        // implements for a rule that does not apply.
        let mut failed_test: Option<String> = None;
        if let Some(entries) = tests
            .and_then(|t| t.get(id))
            .and_then(toml::Value::as_array)
        {
            for (i, entry) in entries.iter().enumerate() {
                let t = match InlineTestSpec::deserialize(entry.clone()) {
                    Ok(t) => t,
                    Err(e) => {
                        failed_test = Some(format!("test[{i}] is malformed: {e}"));
                        break;
                    }
                };
                let got = rule.apply_text(&t.input).map(|o| o.new_text);
                let pass = match got {
                    Some(g) => g == t.expected,
                    None => t.expected == t.input,
                };
                if !pass {
                    failed_test = Some(format!("inline test '{}' failed", t.name));
                    break;
                }
            }
        }
        match failed_test {
            Some(reason) => report.failed.push(FailedRule {
                id: id.clone(),
                reason,
            }),
            None => {
                report.loaded.push(id.clone());
                rules.push(rule);
            }
        }
    }
    // The declared try order (R33-FIX): ascending `order`, ties by id —
    // the loaded rule file's declaration, not the TOML map's order.
    rules.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.id.cmp(&b.id)));
    (TransformRulesEngine { rules }, report)
}

/// Load from a path (the CLI wiring's entry point).
pub fn load_path(path: &Path) -> std::io::Result<(TransformRulesEngine, LoadReport)> {
    let text = std::fs::read_to_string(path)?;
    Ok(load_str(&text))
}

// ---------------------------------------------------------------------------
// Tests — the acceptance test is the repo's own rule file and its 13
// inline tests (DESIGN §12.12 order ④: "whose 13 inline tests are its
// acceptance test")
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    // R33-FIX: every fixture below builds `PayloadCtx` from `kinds_for_tool`
    // — the live derivation (`router-proxy/src/forward.rs`) — never from a
    // hand-built kind list, so an unreachable selection fails a test.
    use router_core::transform::kinds_for_tool;

    const REPO_RULES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../rules/tool_output.toml");

    #[test]
    fn repo_rule_file_loads_and_every_inline_test_passes() {
        let text = std::fs::read_to_string(REPO_RULES).expect("rules/tool_output.toml on disk");
        let (engine, report) = load_str(&text);
        assert_eq!(
            report
                .loaded
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            [
                "bash-log-noise",
                "diff-budget",
                "grep-hits-budget",
                "tool-result-json"
            ]
            .into_iter()
            .map(String::from)
            .collect::<std::collections::BTreeSet<_>>(),
            "all four rules load"
        );
        assert!(report.failed.is_empty(), "no failures: {report:?}");
        // The engine answers by rule id, and the tee markers carry the
        // original payload's identity.
        let ctx = PayloadCtx {
            tool: Some("Grep"),
            kinds: kinds_for_tool("Grep"),
        };
        let input = "src/a.rs-10-  fn unrelated() {\nsrc/a.rs:12:let x = foo();\n";
        let o = engine.apply_node(&ctx, input).expect("grep rule fires");
        assert_eq!(o.rule, "grep-hits-budget");
        assert_eq!(
            o.new_text,
            "src/a.rs:12:let x = foo();\n[router:tee sha256=67de33fd0baa9756 lines_dropped=1 bytes_original=58]\n"
        );
        assert!(o.tee_id.is_some());
    }

    #[test]
    fn selection_is_by_declaration_not_content() {
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let (engine, _report) = load_str(&text);
        // The bash rule selects on tool + kind; the same noisy log text
        // under the Grep tool is not its business (grep keeps hit lines).
        let noisy = "   Compiling serde v1.0.210\n[ 62%] Building object\nerror: boom\n";
        let bash = engine.apply_node(
            &PayloadCtx {
                tool: Some("Bash"),
                kinds: kinds_for_tool("Bash"),
            },
            noisy,
        );
        assert_eq!(bash.expect("bash rule fires").rule, "bash-log-noise");
        let grep = engine.apply_node(
            &PayloadCtx {
                tool: Some("Grep"),
                kinds: kinds_for_tool("Grep"),
            },
            noisy,
        );
        assert_eq!(grep.expect("grep rule fires").rule, "grep-hits-budget");
        // A tool the match_tool regex does not cover: no rule fires, even
        // though the content is the same (selection never sniffs content).
        let other = engine.apply_node(
            &PayloadCtx {
                tool: Some("some_mcp_tool"),
                kinds: kinds_for_tool("some_mcp_tool"),
            },
            noisy,
        );
        assert!(other.is_none(), "content never selects a rule");
        // A nameless payload node: only a wildcard rule could fire, and the
        // file declares none for log-shaped content. `forward.rs` derives
        // the kinds of a nameless node exactly this way ("" matches no row).
        let nameless = engine.apply_node(
            &PayloadCtx {
                tool: None,
                kinds: kinds_for_tool(""),
            },
            noisy,
        );
        assert!(nameless.is_none());
    }

    #[test]
    fn a_rule_with_a_bad_regex_does_not_load_but_its_siblings_do() {
        let src = r#"
schema_version = 1
[filters.good]
match_tool = '^Bash$'
strip_lines_matching = ['^\s*$']
on_empty = '<empty>'
[[tests.good]]
name = "blank lines dropped"
input = "a\n\nb\n"
expected = "a\nb\n"
[filters.bad-regex]
match_tool = '(unclosed'
[[tests.bad-regex]]
name = "never reached"
input = "x"
expected = "x"
"#;
        let (engine, report) = load_str(src);
        assert_eq!(report.loaded, vec!["good"]);
        assert_eq!(report.failed.len(), 1);
        assert_eq!(report.failed[0].id, "bad-regex");
        assert!(report.failed[0].reason.contains("match_tool"));
        // The good rule still serves.
        let o = engine.apply_node(
            &PayloadCtx {
                tool: Some("Bash"),
                kinds: kinds_for_tool("Bash"),
            },
            "a\n\nb\n",
        );
        assert_eq!(o.expect("good rule live").new_text, "a\nb\n");
    }

    #[test]
    fn strip_and_keep_together_is_a_load_failure() {
        let src = r#"
schema_version = 1
[filters.greedy]
match_tool = '^Bash$'
strip_lines_matching = ['^x$']
keep_lines_matching = ['^y$']
[[tests.greedy]]
name = "never reached"
input = "x\ny\n"
expected = "y\n"
"#;
        let (_engine, report) = load_str(src);
        assert_eq!(report.loaded.len(), 0);
        assert!(report.failed[0].reason.contains("mutually exclusive"));
    }

    #[test]
    fn a_failing_inline_test_keeps_the_rule_out_and_the_rest_in() {
        let src = r#"
schema_version = 1
[filters.liar]
match_tool = '^Bash$'
strip_lines_matching = ['^noise $']
[[tests.liar]]
name = "claims the wrong output"
input = "noise \nkeep\n"
expected = "wrong\n"
[filters.honest]
match_tool = '^Bash$'
max_lines = 1
[[tests.honest]]
name = "first line only"
input = "one\ntwo\n"
expected = "one\n"
"#;
        let (engine, report) = load_str(src);
        assert_eq!(report.loaded, vec!["honest"]);
        assert_eq!(report.failed[0].id, "liar");
        assert!(report.failed[0].reason.contains("claims the wrong output"));
        let o = engine.apply_node(
            &PayloadCtx {
                tool: Some("Bash"),
                kinds: kinds_for_tool("Bash"),
            },
            "one\ntwo\n",
        );
        assert_eq!(o.expect("honest serves").new_text, "one\n");
    }

    #[test]
    fn bad_toml_fails_the_whole_file_by_name() {
        let (engine, report) = load_str("this is not toml {{{");
        assert!(report.failed.iter().any(|f| f.id == "<rule file>"));
        assert!(engine.rules.is_empty());
    }

    #[test]
    fn json_compact_keeps_number_literals_verbatim() {
        let src = r#"
schema_version = 1
[filters.j]
match_kind = ["log"]
min_input_bytes = 8
json_compact = true
[[tests.j]]
name = "compacted, order kept, 1.10 not 1.1"
input = '''{
  "a": 1.10,
  "b": 7
}
'''
expected = '''{"a":1.10,"b":7}
'''
"#;
        let (engine, report) = load_str(src);
        assert!(report.failed.is_empty(), "{report:?}");
        let o = engine
            .apply_node(
                &PayloadCtx {
                    tool: Some("Bash"),
                    kinds: kinds_for_tool("Bash"),
                },
                "{\n  \"a\": 1.10,\n  \"b\": 7\n}\n",
            )
            .expect("compacted");
        assert_eq!(o.new_text, "{\"a\":1.10,\"b\":7}\n");
    }

    #[test]
    fn min_input_bytes_pass_through_is_verbatim_including_newlines() {
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let (engine, _report) = load_str(&text);
        // tool-result-json's budget: shorter ⇒ THE RULE does not apply at
        // all (no edit), whatever the payload's newlines look like. The
        // rule selects on kind alone over the live derivation — a Bash
        // node's kinds are the shell family's. Asserted on the rule: at
        // engine level a short Bash payload is the shell rule's business
        // (its trailing-newline normalization is pre-existing live
        // behaviour, unchanged by the settlement).
        let rule = engine
            .rules
            .iter()
            .find(|r| r.id == "tool-result-json")
            .expect("tool-result-json loaded");
        let ctx = PayloadCtx {
            tool: Some("Bash"),
            kinds: kinds_for_tool("Bash"),
        };
        assert!(rule.apply(&ctx, "{\"a\":1}").is_none());
        // And an unmet budget leaves even a payload with no trailing
        // newline byte-for-byte alone (no newline normalization either).
        assert!(rule.apply(&ctx, "{\"a\": 1, \"b\": 2}").is_none());
        // Engine level: the budget guard keeps tool-result-json out of the
        // picture; the payload falls through to the shell rule.
        let o = engine
            .apply_node(&ctx, "{\"a\":1}")
            .expect("the shell rule fires on the short payload");
        assert_eq!(o.rule, "bash-log-noise");
    }

    #[test]
    fn engine_id_is_the_builtin_kind() {
        let (engine, _) = load_str("schema_version = 1\n[filters.x]\n");
        // No rules is a valid engine (an empty file loads nothing); the id
        // is what the config's `kind: builtin/transform_rules` names.
        assert_eq!(TransformEngine::id(&engine), "builtin/transform_rules");
    }

    // ------------------------------------------------------------------
    // R33-FIX (R33-F1): the reachability witness and the red controls.
    // The fixtures below build `PayloadCtx` from `kinds_for_tool` — the
    // live derivation (`router-proxy/src/forward.rs`) — never from a
    // hand-built kind list, so an unreachable selection fails a test.
    // ------------------------------------------------------------------

    /// E1 witness: the kind table, the loaded try order, and one
    /// pretty-printed JSON payload from `Bash` driven through
    /// `apply_node`. Prints the facts; asserts only what holds on both
    /// sides of the settlement (the raw output is the evidence).
    #[test]
    fn r33_fix_witness_kind_table_try_order_and_precedence() {
        use router_core::transform::TOOL_KINDS;
        // Fact 1: on the live path a payload's kinds come only from this
        // declared table — the engine never sniffs payload text.
        for (pat, kinds) in TOOL_KINDS {
            println!("tool_kind_row: {pat} -> {kinds:?}");
        }
        for tool in ["Bash", "Grep", "Diff", "some_mcp_tool"] {
            println!("kinds_for_tool({tool:?}) -> {:?}", kinds_for_tool(tool));
        }
        // Fact 2: the order the shipped file's rules are tried in.
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let (engine, report) = load_str(&text);
        assert!(report.failed.is_empty(), "{report:?}");
        println!("load_report.loaded (TOML map order): {:?}", report.loaded);
        for (i, rule) in engine.rules.iter().enumerate() {
            println!(
                "try_order[{i}]: {} match_kind={:?}",
                rule.id, rule.match_kind
            );
        }
        // The same payload through the engine: R33-1's B3 leg shape — a
        // pretty-printed JSON payload in a tool node paired to `Bash`.
        let payload = concat!(
            "{\n",
            "  \"total\": 3,\n",
            "  \"hits\": [\n",
            "    {\"path\": \"a.rs\", \"line\": 12},\n",
            "    {\"path\": \"b.rs\", \"line\": 40}\n",
            "  ]\n",
            "}\n"
        );
        let ctx = PayloadCtx {
            tool: Some("Bash"),
            kinds: kinds_for_tool("Bash"),
        };
        for rule in &engine.rules {
            let selects = rule.selects(&ctx);
            let applied = rule
                .apply(&ctx, payload)
                .map(|o| (o.rule.clone(), o.new_text.len()));
            println!(
                "per_rule: {} selects={selects} applied={applied:?}",
                rule.id
            );
        }
        let winner = engine
            .apply_node(&ctx, payload)
            .expect("a rule fires on the JSON payload from Bash");
        println!(
            "winner: {} bytes {} -> {}",
            winner.rule,
            payload.len(),
            winner.new_text.len()
        );
    }

    /// E3 red control (R33-FIX; amended by R35-2): the reachability
    /// invariant over the two artifacts, read at test time (AGENTS 6 — no
    /// snapshot list). Two limbs: (i) a rule that declares `match_kind`
    /// declares at least one entry `kinds_for_tool` can produce for some
    /// tool name — an unreachable kind declaration is a load-time lie
    /// (R33-F1); (ii) a rule that declares NO `match_kind` — the
    /// R35-1-F1 repair shape, selection on the wire's own tool name with
    /// the kind gate lifted — declares `match_tool`, because a wildcard
    /// selection is not this file's default. The invariant's subject is
    /// the DECLARATION; the observed client vocabulary is the sibling
    /// case `observed_client_tool_vocabulary_selects_a_shipped_rule`
    /// (R35-1's D7 R-c), which is the assertion that can fail for the
    /// reason R35-1-F1 names.
    #[test]
    fn every_shipped_rule_kind_is_reachable_on_the_live_path() {
        use router_core::transform::TOOL_KINDS;
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        let producible: std::collections::BTreeSet<&str> = TOOL_KINDS
            .iter()
            .flat_map(|(_, kinds)| kinds.iter().copied())
            .collect();
        let filters = doc
            .get("filters")
            .and_then(toml::Value::as_table)
            .expect("[filters] table");
        for (id, spec) in filters {
            let declared: Vec<&str> = spec
                .get("match_kind")
                .and_then(toml::Value::as_array)
                .map(|a| a.iter().filter_map(toml::Value::as_str).collect())
                .unwrap_or_default();
            if declared.is_empty() {
                assert!(
                    spec.get("match_tool").is_some(),
                    "rule '{id}' declares neither match_kind nor match_tool — \
                     a wildcard selection is not this file's default (R35-2)"
                );
                continue;
            }
            assert!(
                declared.iter().any(|k| producible.contains(k)),
                "rule '{id}' declares match_kind {declared:?}; none is producible \
                 by kinds_for_tool (producible: {producible:?}) — an unreachable \
                 selection on the live path (R33-F1)"
            );
        }
    }

    /// R35-2 red control (R35-1's D7 R-c, finding R35-1-F1): the shipped
    /// rule set must SELECT at least one payload node on the tool names
    /// this repository's own captured traffic carries. The vocabulary is
    /// the committed measurement (`autowork/harness/r35-1/corpus-shape.json`
    /// — the frozen corpus's 36 payload nodes, every one named
    /// `exec_command`), never a recalled list. The pre-existing invariant
    /// (`every_shipped_rule_kind_is_reachable_on_the_live_path`) asserts
    /// over the DECLARED table and cannot fail for this reason; this case
    /// asserts over the observed client vocabulary.
    #[test]
    fn observed_client_tool_vocabulary_selects_a_shipped_rule() {
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let (engine, report) = load_str(&text);
        assert!(report.failed.is_empty(), "{report:?}");
        let shape: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../autowork/harness/r35-1/corpus-shape.json"
            ))
            .expect("r35-1's committed corpus-shape.json on disk"),
        )
        .expect("corpus-shape.json is JSON");
        let names: Vec<&str> = shape["summary"]["tool_names_seen"]
            .as_array()
            .expect("tool_names_seen is an array")
            .iter()
            .map(|v| v.as_str().expect("tool names are strings"))
            .collect();
        assert!(!names.is_empty(), "the measured vocabulary is not empty");
        for tool in names {
            let ctx = PayloadCtx {
                tool: Some(tool),
                kinds: kinds_for_tool(tool),
            };
            assert!(
                engine.rules.iter().any(|r| r.selects(&ctx)),
                "0 shipped rules select a node whose tool is {tool:?} — \
                 every shipped rule is unreachable for the clients this \
                 repository configures (R35-1-F1)"
            );
        }
    }

    /// R33-FIX: the rule's own declared scenario on the live derivation —
    /// a pretty-printed JSON payload from `Bash` reaches
    /// `tool-result-json` (order-preserving compaction), not the shell
    /// rule's `max_lines` truncation.
    #[test]
    fn json_from_bash_fires_tool_result_json_on_live_kinds() {
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let (engine, report) = load_str(&text);
        assert!(report.failed.is_empty(), "{report:?}");
        let payload = concat!(
            "{\n",
            "  \"total\": 3,\n",
            "  \"hits\": [\n",
            "    {\"path\": \"a.rs\", \"line\": 12},\n",
            "    {\"path\": \"b.rs\", \"line\": 40}\n",
            "  ]\n",
            "}\n"
        );
        let ctx = PayloadCtx {
            tool: Some("Bash"),
            kinds: kinds_for_tool("Bash"),
        };
        let o = engine
            .apply_node(&ctx, payload)
            .expect("a rule fires on the JSON payload from Bash");
        assert_eq!(
            o.rule, "tool-result-json",
            "the JSON payload must reach the compaction rule (R33-F1)"
        );
        assert_eq!(
            o.new_text,
            "{\"total\":3,\"hits\":[{\"path\":\"a.rs\",\"line\":12},{\"path\":\"b.rs\",\"line\":40}]}\n"
        );
    }

    /// R33-FIX: the `order` key is the declared try order — ascending,
    /// default 0, ties alphabetical. A rule that selects but does not
    /// apply falls through to the next rule.
    #[test]
    fn declared_order_key_decides_try_order() {
        let src = r#"
schema_version = 1
[filters.a-first-alphabetically]
match_tool = '^Bash$'
max_lines = 1
order = 7
[[tests.a-first-alphabetically]]
name = "first line only"
input = "one\ntwo\n"
expected = "one\n"
[filters.z-last-alphabetically]
match_tool = '^Bash$'
max_lines = 2
order = 3
[[tests.z-last-alphabetically]]
name = "both lines"
input = "one\ntwo\n"
expected = "one\ntwo\n"
"#;
        let (engine, report) = load_str(src);
        assert!(report.failed.is_empty(), "{report:?}");
        let ids: Vec<&str> = engine.rules.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids,
            ["z-last-alphabetically", "a-first-alphabetically"],
            "ascending order, not the map's alphabetical order"
        );
        // z applies (max_lines 2 over a two-line payload), so it wins; had
        // it not applied, a would have served the node.
        let o = engine
            .apply_node(
                &PayloadCtx {
                    tool: Some("Bash"),
                    kinds: kinds_for_tool("Bash"),
                },
                "one\ntwo\n",
            )
            .expect("a rule fires");
        assert_eq!(o.rule, "z-last-alphabetically");
    }

    /// R33-FIX negative control: a non-JSON payload of the same tool keeps
    /// its pre-settlement outcome — `bash-log-noise` fires with the same
    /// edit; the settlement changes which rules may act, never what a
    /// payload's bytes are.
    #[test]
    fn non_json_bash_payload_still_fires_bash_log_noise() {
        let text = std::fs::read_to_string(REPO_RULES).unwrap();
        let (engine, _report) = load_str(&text);
        let noisy = "   Compiling serde v1.0.210\n[ 62%] Building object\nerror: boom\n";
        let ctx = PayloadCtx {
            tool: Some("Bash"),
            kinds: kinds_for_tool("Bash"),
        };
        let o = engine
            .apply_node(&ctx, noisy)
            .expect("the shell rule fires on the log payload");
        assert_eq!(o.rule, "bash-log-noise");
        assert_eq!(o.new_text, "error: boom\n");
    }
}
