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

/// The engine: the compiled rules in file order. The first rule that both
/// selects and produces an edit wins for a node.
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
            kinds: &["text"],
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
                kinds: &["log", "text"],
            },
            noisy,
        );
        assert_eq!(bash.expect("bash rule fires").rule, "bash-log-noise");
        let grep = engine.apply_node(
            &PayloadCtx {
                tool: Some("Grep"),
                kinds: &["text"],
            },
            noisy,
        );
        assert_eq!(grep.expect("grep rule fires").rule, "grep-hits-budget");
        // A tool the match_tool regex does not cover: no rule fires, even
        // though the content is the same (selection never sniffs content).
        let other = engine.apply_node(
            &PayloadCtx {
                tool: Some("some_mcp_tool"),
                kinds: &[],
            },
            noisy,
        );
        assert!(other.is_none(), "content never selects a rule");
        // A nameless payload node: only a wildcard rule could fire, and the
        // file declares none for log-shaped content.
        let nameless = engine.apply_node(
            &PayloadCtx {
                tool: None,
                kinds: &[],
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
                kinds: &[],
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
                kinds: &[],
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
match_kind = ["json"]
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
                    tool: None,
                    kinds: &["json"],
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
        // tool-result-json's budget: shorter ⇒ the rule does not apply at
        // all (no edit), whatever the payload's newlines look like. The
        // rule selects on kind alone, so the node needs no tool name.
        let ctx = PayloadCtx {
            tool: None,
            kinds: &["json"],
        };
        assert!(engine.apply_node(&ctx, "{\"a\":1}").is_none());
        // And an unmet budget leaves even a payload with no trailing
        // newline byte-for-byte alone (no newline normalization either).
        assert!(engine.apply_node(&ctx, "{\"a\": 1, \"b\": 2}").is_none());
    }

    #[test]
    fn engine_id_is_the_builtin_kind() {
        let (engine, _) = load_str("schema_version = 1\n[filters.x]\n");
        // No rules is a valid engine (an empty file loads nothing); the id
        // is what the config's `kind: builtin/transform_rules` names.
        assert_eq!(TransformEngine::id(&engine), "builtin/transform_rules");
    }
}
