//! The assembly (R41-3, ADR-036 D1/D8, spec §4.3): maps the declared
//! `plugins:` list to plugin instances and drives the runtime's `Loader`
//! with them. The launcher (`router-cli`) calls [`assemble`] **once**,
//! prints the returned notes verbatim, and reads what the assembled
//! context provides through the typed slots below — no plugin-kind
//! branching lives in the CLI, and nothing on the serving path touches
//! the loader: resolution happens here, at boot (ADR-036 D8 — a
//! per-request `ServiceKey` lookup, `Arc` clone or lock would be a
//! defect).
//!
//! The registry is exactly one kind this round — `builtin/transform_rules`
//! (§13.1's P6, the first migration). The three always-resident builtins
//! (`builtin/cost_ledger`, `builtin/quota_guard`, `builtin/sticky`) are
//! **not** here on purpose: they stay resident (the round's scope call).
//! A declared entry whose kind has no mount is a **named absence** on the
//! start-up log, never a silent drop.
//!
//! Live keys this round: `disabled` (not loaded at start-up, `/health`
//! reports it from the config — unchanged) and `inject` (an unmet
//! declaration leaves the fiber at `Loading{waiting_on}`, named on the
//! start-up log, instead of mounting). `isolate` and `intercept` stay
//! inert — accepted by the parser, acted on by nothing (the round's
//! scope call (i)).

use std::path::PathBuf;
use std::sync::Arc;

use router_core::config::PluginCfg;
use router_core::transform::TransformEngine;
use router_runtime::{
    Ctx, Effect, FiberState, Loader, Plugin, PluginError, PluginId, ServiceId, ServiceKey,
};

use crate::transform_rules::load_path;

/// The typed slot the transform chain is bound under (§13.1 P6, spec
/// §4.4). The launcher reads it once at start-up and hands the engine to
/// the `Forwarder`; the serving path never sees the slot.
pub const TRANSFORM_CHAIN: ServiceKey<TransformChain> = ServiceKey::new("transform_chain");

/// The value bound under [`TRANSFORM_CHAIN`]: the assembled transform
/// chain, pre-built at boot.
pub struct TransformChain(Arc<dyn TransformEngine>);

impl TransformChain {
    /// The launcher's handle on the chain. Cloning the `Arc` is a
    /// boot-time cost, once — never a serving-path one.
    pub fn engine(&self) -> Arc<dyn TransformEngine> {
        Arc::clone(&self.0)
    }
}

/// The one mountable plugin of this round: the transform chain, already
/// loaded by the factory, bound under [`TRANSFORM_CHAIN`] at activation.
/// `apply` does no I/O — the rule file was read by the factory, so a
/// fiber's activation is pure registration (and a failed `apply` has
/// nothing to roll back beyond what the loader already sweeps).
struct TransformRulesPlugin {
    id: PluginId,
    inject: &'static [ServiceId],
    engine: Arc<dyn TransformEngine>,
}

impl Plugin for TransformRulesPlugin {
    fn id(&self) -> &PluginId {
        &self.id
    }

    fn inject(&self) -> &'static [ServiceId] {
        self.inject
    }

    fn apply(&self, ctx: &mut Ctx) -> Result<Effect, PluginError> {
        ctx.provide(
            TRANSFORM_CHAIN,
            Arc::new(TransformChain(Arc::clone(&self.engine))),
        );
        Ok(Effect::noop())
    }
}

/// What the launcher gets back: the loaded runtime (kept alive for the
/// process — the bindings are the assembled pipeline) plus the start-up
/// log lines, in declaration order, each without the `router: ` prefix
/// (the launcher owns its log namespace).
pub struct Assembly {
    loader: Loader,
    notes: Vec<String>,
}

impl Assembly {
    /// The start-up log lines: named absences, rule-load failures, and
    /// named loading waits, in the order the `plugins:` list produced
    /// them (waits last — they are only knowable once resolution has run
    /// to its fixpoint).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// The transform chain the assembled context provides, or `None` when
    /// no entry mounted one (nothing declared it, every declaration
    /// failed, or the winner is still waiting on an `inject` slot) — the
    /// *asked-but-not-applied* case of spec §4.4/§6, unchanged.
    pub fn transform_engine(&self) -> Option<Arc<dyn TransformEngine>> {
        self.loader
            .get(&TRANSFORM_CHAIN)
            .map(|chain| chain.engine())
    }
}

/// The registry: the kinds the launcher can mount from the `plugins:`
/// list. Exactly one this round (§13.1 P6's migration; the three
/// always-resident builtins stay resident). Anything else declared is a
/// named absence — see [`assemble`].
const REGISTRY: &[&str] = &["builtin/transform_rules"];

/// Builds the pipeline from the declared `plugins:` list: one pass that
/// maps each entry to a plugin instance via the registry, one
/// `Loader::load` per mountable instance, one read of the resulting
/// fiber states for the named waits. `resolve` anchors a plugin's
/// relative paths at the config file's directory (the CLI's
/// `config_load::resolve` — path policy stays config machinery, §12.10.2).
///
/// Semantics carried over from the hand-rolled assembly, exactly:
///
/// - **first hit wins**: the first enabled `builtin/transform_rules`
///   entry whose rule file *loads* mounts the chain; later entries of
///   the kind are named as superseded, and an entry that fails falls
///   through to the next;
/// - **named absences**: a missing `config.rules_file`, an unreadable
///   rule file and a per-rule load failure produce the same log lines
///   the hand-rolled assembly printed;
/// - **`disabled`**: the entry is not loaded and says nothing (its
///   start-up meaning; `/health` shows it from the config);
/// - **`inject`**: translated into the fiber's coeffect declaration, so
///   a slot nothing provides leaves the fiber at `Loading{waiting_on}` —
///   named on the log, not a boot failure (spec §4.3).
pub fn assemble(plugins: &[PluginCfg], resolve: &dyn Fn(&str) -> PathBuf) -> Assembly {
    let mut loader = Loader::new();
    let mut notes = Vec::new();
    // Mounted fibers, in declaration order, for the named-wait pass.
    let mut mounted: Vec<&PluginCfg> = Vec::new();
    // The id of the entry that won the transform chain (first hit wins —
    // the three-level override is a later card's lookup ladder).
    let mut chain_winner: Option<&str> = None;

    for plug in plugins {
        if plug.disabled {
            // `disabled`'s start-up meaning (spec §4.4): not loaded, and
            // silent here — `/health` reports the entry as disabled from
            // the config.
            continue;
        }
        if !REGISTRY.contains(&plug.kind.as_str()) {
            // A declared entry the launcher cannot mount is a named
            // absence — the hand-rolled assembly's silent `continue` is
            // the behaviour this round removes.
            notes.push(format!(
                "plugins[{}] (kind {}): the launcher has no mount for this kind; \
                 not loaded (a declared entry is a named absence, not a silent drop)",
                plug.id, plug.kind
            ));
            continue;
        }
        // The one registry row: builtin/transform_rules.
        if let Some(winner) = chain_winner {
            notes.push(format!(
                "plugins[{}] (kind {}): not loaded — first hit wins; \
                 the transform chain is mounted from plugins[{winner}]",
                plug.id, plug.kind
            ));
            continue;
        }
        match build_transform_rules(plug, resolve) {
            Ok((plugin, mut lines)) => {
                notes.append(&mut lines);
                let id = plug.id.clone();
                if let Err(e) = loader.load(Box::new(plugin)) {
                    // Unreachable: config validation refuses duplicate
                    // plugin ids (config.rs's plugin loop). Named anyway
                    // — never a silent drop.
                    notes.push(format!("plugins[{id}] (kind {}): {e}", plug.kind));
                    continue;
                }
                chain_winner = Some(&plug.id);
                mounted.push(plug);
            }
            Err(line) => {
                // A failed entry is a named absence and the next entry
                // of the kind gets its turn (first *successful* hit
                // wins) — the hand-rolled assembly's `continue`.
                notes.push(line);
            }
        }
    }

    // The named loading waits (spec §4.3): a mounted plugin whose
    // `inject` nothing provides sits at `Loading{waiting_on}` — it does
    // not fail, and nothing else is affected. Knowable only now, after
    // resolution has run to its fixpoint.
    for plug in mounted {
        if let Some(FiberState::Loading { waiting_on }) =
            loader.state(&PluginId::new(plug.id.clone()))
        {
            let slots = waiting_on
                .iter()
                .map(|s| s.name())
                .collect::<Vec<_>>()
                .join(", ");
            notes.push(format!(
                "plugins[{}] (kind {}): inject declares service slot(s) {slots} that \
                 nothing provides — the plugin waits at Loading (a named wait, not a \
                 boot failure, spec §4.3); what it would provide is absent until then",
                plug.id, plug.kind
            ));
        }
    }

    Assembly { loader, notes }
}

/// The `builtin/transform_rules` factory row: resolve `config.rules_file`,
/// load the rule set, and wrap the engine as the plugin the loader
/// mounts. The `Err` arm is the named-absence line (the entry gets no
/// fiber and the next of its kind is tried); the `Ok` arm carries the
/// report lines the start-up log prints verbatim.
fn build_transform_rules(
    plug: &PluginCfg,
    resolve: &dyn Fn(&str) -> PathBuf,
) -> Result<(TransformRulesPlugin, Vec<String>), String> {
    let rules_file = plug
        .config
        .as_ref()
        .and_then(|c| c.get("rules_file"))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    let Some(rules_file) = rules_file else {
        return Err(format!(
            "plugins[{}] (kind builtin/transform_rules) has no config.rules_file: \
             not loaded (an engine that cannot find its rules is a named absence, \
             not a silent empty one)",
            plug.id
        ));
    };
    let path = resolve(&rules_file);
    match load_path(&path) {
        Ok((engine, report)) => {
            let mut lines = report.failure_lines();
            if report.loaded.is_empty() && report.failed.is_empty() {
                lines.push(format!(
                    "transform_rules: rule file {} has no rules; \
                     transform mode will ask-but-not-apply",
                    path.display()
                ));
            }
            let plugin = TransformRulesPlugin {
                id: PluginId::new(plug.id.clone()),
                inject: declared_inject(&plug.inject),
                engine: Arc::new(engine),
            };
            Ok((plugin, lines))
        }
        Err(e) => Err(format!(
            "transform_rules: rule file {}: {e}; not loaded \
             (a request that asks for transform mode runs with an empty ledger)",
            path.display()
        )),
    }
}

/// Translates the config's `inject:` names into the fiber's coeffect
/// declaration. Config validation (`KNOWN_SERVICE_SLOTS`,
/// config.rs:1744-1804) has already refused any name outside the
/// product-defined set, so every name here is a known slot name. The
/// leaked slice is process-lifetime by design: the assembly is built
/// once at start-up and never reloaded (the round's scope call (ii) —
/// no config-diff, no live reload).
fn declared_inject(names: &[String]) -> &'static [ServiceId] {
    let ids: Vec<ServiceId> = names
        .iter()
        .map(|n| ServiceId::new(Box::leak(n.clone().into_boxed_str())))
        .collect();
    Box::leak(ids.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "r41-3-assembly-{}-{tag}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A rule file whose one rule selects a tool no request names — it
    /// loads, its inline test passes (a stageless rule is the identity
    /// modulo the trailing newline, so the test round-trips "hello\n"),
    /// and it edits nothing on the wire.
    const QUIET_RULES: &str = r#"
schema_version = 1

[filters.never-matches]
match_tool = "^NoSuchToolAnywhere$"

[[tests.never-matches]]
name = "identity"
input = "hello\n"
expected = "hello\n"
"#;

    /// A rule file whose inline test fails: the rule does not load and
    /// the report names it (a visible failure line).
    const FAILING_RULES: &str = r#"
schema_version = 1

[filters.always-shout]
message = "SHOUT"

[[tests.always-shout]]
name = "deliberately wrong"
input = "hello"
expected = "this is not what the rule produces"
"#;

    fn entry(id: &str, kind: &str, config: Option<serde_json::Value>) -> PluginCfg {
        PluginCfg {
            id: id.to_string(),
            kind: kind.to_string(),
            config,
            url: None,
            inject: Vec::new(),
            isolate: false,
            intercept: None,
            disabled: false,
        }
    }

    fn rules_entry(id: &str, rules_file: &str) -> PluginCfg {
        entry(
            id,
            "builtin/transform_rules",
            Some(serde_json::json!({ "rules_file": rules_file })),
        )
    }

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).unwrap();
    }

    /// An empty list mounts nothing and says nothing — ADR-036 D1's
    /// first half at the assembly's own level.
    ///
    /// RED recipe: in `assemble`, push an unconditional note before the
    /// loop — `notes must be empty` fails.
    #[test]
    fn an_empty_list_mounts_nothing_and_says_nothing() {
        let dir = tempdir("empty");
        let resolve = |f: &str| dir.join(f);
        let assembly = assemble(&[], &resolve);
        assert!(assembly.notes().is_empty(), "notes must be empty");
        assert!(assembly.transform_engine().is_none());
    }

    /// A declared `builtin/transform_rules` entry is mounted: its fiber
    /// goes `Active`, and the assembled context provides the engine
    /// through the typed slot.
    ///
    /// RED recipe: in `TransformRulesPlugin::apply`, delete the
    /// `ctx.provide` — `transform_engine()` reads `None`.
    #[test]
    fn a_transform_rules_entry_mounts_and_provides_the_typed_slot() {
        let dir = tempdir("mounted");
        write(&dir, "rules.toml", QUIET_RULES);
        let resolve = |f: &str| dir.join(f);
        let assembly = assemble(&[rules_entry("tool-output-rules", "rules.toml")], &resolve);
        assert!(
            assembly.transform_engine().is_some(),
            "the typed slot must provide the mounted engine"
        );
        assert_eq!(
            assembly.loader.state(&PluginId::new("tool-output-rules")),
            Some(FiberState::Active)
        );
        assert!(assembly.notes().is_empty(), "notes: {:?}", assembly.notes());
    }

    /// An entry whose kind the launcher cannot mount is a named absence
    /// on the start-up log — the hand-rolled assembly's silent
    /// `continue` is gone. The entry gets no fiber.
    ///
    /// RED recipe: in `assemble`, replace the `!REGISTRY.contains` arm's
    /// `notes.push` with a bare `continue` — both assertions fail.
    #[test]
    fn an_unmountable_kind_is_a_named_absence_not_a_silent_drop() {
        let dir = tempdir("absence");
        let resolve = |f: &str| dir.join(f);
        let entry = entry(
            "cache-guard",
            "builtin/cache_guard",
            Some(serde_json::json!({ "strict_prefix": true })),
        );
        let assembly = assemble(std::slice::from_ref(&entry), &resolve);
        assert_eq!(assembly.notes().len(), 1);
        let note = &assembly.notes()[0];
        assert!(note.contains("plugins[cache-guard]"), "note: {note}");
        assert!(note.contains("builtin/cache_guard"), "note: {note}");
        assert!(note.contains("named absence"), "note: {note}");
        assert_eq!(assembly.loader.state(&PluginId::new("cache-guard")), None);
    }

    /// `disabled` keeps its start-up meaning: not loaded, and silent on
    /// the log (`/health` reports it from the config).
    #[test]
    fn a_disabled_entry_is_not_mounted_and_stays_silent() {
        let dir = tempdir("disabled");
        write(&dir, "rules.toml", QUIET_RULES);
        let resolve = |f: &str| dir.join(f);
        let mut entry = rules_entry("tool-output-rules", "rules.toml");
        entry.disabled = true;
        let assembly = assemble(std::slice::from_ref(&entry), &resolve);
        assert!(assembly.notes().is_empty());
        assert!(assembly.transform_engine().is_none());
        assert_eq!(
            assembly.loader.state(&PluginId::new("tool-output-rules")),
            None
        );
    }

    /// First hit wins: of two enabled entries of the kind, the first
    /// mounts and the second is named as superseded — its own rule-load
    /// lines never appear, proving its file was never even read.
    ///
    /// RED recipe: in `assemble`, delete the `chain_winner` guard — the
    /// second entry mounts, its failing rule's line appears, and the
    /// superseded note is absent.
    #[test]
    fn first_hit_wins_and_the_loser_is_named() {
        let dir = tempdir("first-hit");
        write(&dir, "first.toml", QUIET_RULES);
        write(&dir, "second.toml", FAILING_RULES);
        let resolve = |f: &str| dir.join(f);
        let assembly = assemble(
            &[
                rules_entry("rules-first", "first.toml"),
                rules_entry("rules-second", "second.toml"),
            ],
            &resolve,
        );
        let notes = assembly.notes();
        assert_eq!(notes.len(), 1, "notes: {notes:?}");
        assert!(
            notes[0].contains("plugins[rules-second]"),
            "note: {}",
            notes[0]
        );
        assert!(notes[0].contains("first hit wins"), "note: {}", notes[0]);
        assert!(notes[0].contains("rules-first"), "note: {}", notes[0]);
        assert!(
            assembly.transform_engine().is_some(),
            "the winner's engine is mounted"
        );
    }

    /// A failed first entry falls through: the next enabled entry of the
    /// kind mounts (first *successful* hit wins), and the failure is a
    /// named absence — the hand-rolled assembly's `continue`.
    #[test]
    fn a_failed_first_entry_falls_through_to_the_next() {
        let dir = tempdir("fall-through");
        write(&dir, "good.toml", QUIET_RULES);
        let resolve = |f: &str| dir.join(f);
        let assembly = assemble(
            &[
                rules_entry("rules-missing-file", "no-such-file.toml"),
                rules_entry("rules-good", "good.toml"),
            ],
            &resolve,
        );
        let notes = assembly.notes();
        assert_eq!(notes.len(), 1, "notes: {notes:?}");
        assert!(
            notes[0].contains("transform_rules: rule file")
                && notes[0].contains("no-such-file.toml")
                && notes[0].contains("not loaded"),
            "note: {}",
            notes[0]
        );
        assert!(assembly.transform_engine().is_some());
        assert_eq!(
            assembly.loader.state(&PluginId::new("rules-good")),
            Some(FiberState::Active)
        );
    }

    /// An entry with no `config.rules_file` is the same named absence
    /// the hand-rolled assembly printed, and mounts nothing.
    #[test]
    fn a_missing_rules_file_key_is_a_named_absence() {
        let dir = tempdir("no-rules-file");
        let resolve = |f: &str| dir.join(f);
        let entry = entry(
            "tool-output-rules",
            "builtin/transform_rules",
            Some(serde_json::json!({ "on_failure": "passthrough" })),
        );
        let assembly = assemble(std::slice::from_ref(&entry), &resolve);
        assert_eq!(
            assembly.notes(),
            &[
                "plugins[tool-output-rules] (kind builtin/transform_rules) has no \
               config.rules_file: not loaded (an engine that cannot find its rules \
               is a named absence, not a silent empty one)"
                    .to_string()
            ],
        );
        assert!(assembly.transform_engine().is_none());
    }

    /// `inject` is live: a declaration nothing provides leaves the fiber
    /// at `Loading{waiting_on}` — named on the log, not a boot failure —
    /// and the typed slot stays empty (the *asked-but-not-applied* case).
    ///
    /// RED recipe: in `build_transform_rules`, pass `&[]` instead of
    /// `declared_inject(&plug.inject)` — the fiber activates and every
    /// assertion below inverts.
    #[test]
    fn an_unsatisfied_inject_is_a_named_loading_wait() {
        let dir = tempdir("inject-wait");
        write(&dir, "rules.toml", QUIET_RULES);
        let resolve = |f: &str| dir.join(f);
        let mut entry = rules_entry("tool-output-rules", "rules.toml");
        entry.inject = vec!["cache_ledger".to_string()];
        let assembly = assemble(std::slice::from_ref(&entry), &resolve);
        assert_eq!(
            assembly.loader.state(&PluginId::new("tool-output-rules")),
            Some(FiberState::Loading {
                waiting_on: vec![ServiceId::new("cache_ledger")],
            })
        );
        assert!(
            assembly.transform_engine().is_none(),
            "a waiting fiber provides nothing"
        );
        assert_eq!(assembly.notes().len(), 1, "notes: {:?}", assembly.notes());
        let note = &assembly.notes()[0];
        assert!(note.contains("plugins[tool-output-rules]"), "note: {note}");
        assert!(note.contains("cache_ledger"), "note: {note}");
        assert!(note.contains("Loading"), "note: {note}");
    }

    /// A rule file with no rules still mounts (the engine exists; the
    /// mode will ask-but-not-apply) and says so — the hand-rolled
    /// assembly's exact semantics.
    #[test]
    fn an_empty_rule_file_mounts_and_says_ask_but_not_apply() {
        let dir = tempdir("empty-rules");
        // schema_version 1 + an empty [filters] table: nothing loaded,
        // nothing failed — the "has no rules" case exactly.
        write(&dir, "empty.toml", "schema_version = 1\n\n[filters]\n");
        let resolve = |f: &str| dir.join(f);
        let assembly = assemble(&[rules_entry("tool-output-rules", "empty.toml")], &resolve);
        assert!(
            assembly.transform_engine().is_some(),
            "an empty rule set is still a mounted engine"
        );
        assert_eq!(assembly.notes().len(), 1, "notes: {:?}", assembly.notes());
        assert!(
            assembly.notes()[0].contains("has no rules; transform mode will ask-but-not-apply"),
            "note: {}",
            assembly.notes()[0]
        );
    }
}
