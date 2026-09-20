//! CONF-55 (§12.8; spec §3/§4.8/§6 + ADR-018 §3/§4): **the family tag
//! pairs two routes whose native ids differ — and it is a name, not an
//! address.**
//!
//! Numbering note: this round's cases start at CONF-52 (46/47 are on
//! the R6 branch; 52/53/54 are R8-2a's; recorded with this file in
//! DESIGN §12.8).
//!
//! The rig: the CONF-32 plan-first shape with the family split across
//! **different native ids** — `p-plan` (coding_plan) serves `k3`,
//! `p-api` (api, `region: cn`) serves `kimi-k3`, the metered entry
//! carries `family: fam`, and `plan_policy.family` says `fam`. Asserted:
//!
//! - (a) the pairing routes: turn 1 in-plan is served by the plan
//!   entry, turn 2 (after the 403) spills to the metered entry —
//!   the policy's tag matched two entries whose ids share nothing;
//! - (b) F3 unchanged (ADR-018 §4): the plan mock receives
//!   `"model":"k3"`, the api mock receives `"model":"kimi-k3"` —
//!   each route's own native id — while both records'
//!   `requested_model` is the client's string `"p-plan/k3"` verbatim;
//! - (c) **the tag is not an address**: a client writing the bare tag
//!   `fam` gets `404 unknown_model` (nothing reaches any upstream),
//!   and `p-api/fam` — the tag in the model position — is refused the
//!   same way;
//! - (d) **the tag never leaves the process**: no byte of any upstream
//!   request contains it;
//! - (e) `region` is a declared field with a display consequence and
//!   no routing one: `/health` reports `p-api` as `cn` and `p-plan`
//!   as `intl` (the default), while every request routes exactly as
//!   it would without the keys.

#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::net::TcpStream;

use router_conformance::testkit::{self, PlanRig};

/// The policy names the tag, not either id (the whole point of §4.8).
const POLICY: &str = "  family: fam\n  primary: p-plan/k3\n  overflow: p-api/kimi-k3\n  on_primary_exhausted: spill\n  recover: probe\n  cooldown: 1h";

/// The plan rig's config with the family split across different native
/// ids and the metered entry declared `region: cn` (a legal, inert
/// declaration — ADR-018 §3).
async fn tagged_rig(tag: &str) -> PlanRig {
    let (plan, api, dir, listen_addr) = testkit::plan_rig_parts(tag, "", POLICY).await;
    let cfg_path = dir.join("config.yaml");
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    // The rig's two `m1` entries are disambiguated by their price
    // tables (0.001 = the plan entry, 0.002 = the metered one).
    let patched = text
        .replace("  - name: p-api\n", "  - name: p-api\n    region: cn\n")
        .replace(
            "      - id: m1\n        context: 128k\n        price:\n          input_miss: 0.001",
            "      - id: k3\n        family: fam\n        context: 128k\n        price:\n          input_miss: 0.001",
        )
        .replace(
            "      - id: m1\n        context: 128k\n        price:\n          input_miss: 0.002",
            "      - id: kimi-k3\n        family: fam\n        context: 128k\n        price:\n          input_miss: 0.002",
        );
    std::fs::write(&cfg_path, patched).unwrap();
    let cfg = cfg_path.to_string_lossy().into_owned();
    let serve_task = tokio::task::spawn(async move { router_cli::serve(&cfg).await });
    testkit::wait_listening(&listen_addr);
    PlanRig {
        plan,
        api,
        listen_addr,
        dir,
        serve_task,
    }
}

fn post_chat(addr: &str, model: &str, session: Option<&str>) -> (u16, Vec<u8>) {
    let key = session
        .map(|s| format!(r#","prompt_cache_key":"{s}""#))
        .unwrap_or_default();
    let body = format!(
        r#"{{"model":"{model}","messages":[{{"role":"user","content":"turn"}}]{key},"stream":false}}"#
    );
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status: u16 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or("")
        .as_bytes()
        .to_vec();
    (status, body)
}

fn http_get_json(addr: &str, path: &str) -> serde_json::Value {
    let mut stream = TcpStream::connect(addr).expect("connect");
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap_or("").trim()).expect("health json")
}

fn trace_records(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir.join("state/traces")).expect("trace dir") {
        let entry = entry.unwrap();
        if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in std::fs::read_to_string(entry.path()).unwrap().lines() {
            out.push(serde_json::from_str(line).expect("record json"));
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conf_55_family_tag_pairs_ids_and_is_not_an_address() {
    let rig = tagged_rig("conf55-fam").await;
    rig.plan.queue(testkit::plan_ok("t1"));
    rig.plan.queue(testkit::plan_forbidden_403());
    rig.api.queue(testkit::plan_ok("spilled"));

    // (a)+(b): turn 1 in-plan on the plan entry's own id; turn 2 meets
    // the 403 and spills to the metered entry's own id — one tag, two
    // native ids, the client string never changes.
    let (s1, _b1) = post_chat(&rig.listen_addr, "p-plan/k3", Some("S1"));
    assert_eq!(s1, 200, "turn 1 serves from the plan entry");
    let (s2, _b2) = post_chat(&rig.listen_addr, "p-plan/k3", Some("S1"));
    assert_eq!(s2, 200, "turn 2 spills to the metered entry (same tag)");

    // (c): the bare tag resolves to nothing — and neither does the tag
    // in the model position of an explicit provider route.
    let (s3, b3) = post_chat(&rig.listen_addr, "fam", None);
    assert_eq!(s3, 404, "a bare tag is not an address");
    let err: serde_json::Value = serde_json::from_slice(&b3).unwrap_or(serde_json::Value::Null);
    assert_eq!(
        err["error"]["type"], "unknown_model",
        "the bare tag is refused as unknown_model, body: {b3:?}"
    );
    let (s4, b4) = post_chat(&rig.listen_addr, "p-api/fam", None);
    assert_eq!(s4, 404, "the tag is not an id on its own provider either");
    let err4: serde_json::Value = serde_json::from_slice(&b4).unwrap_or(serde_json::Value::Null);
    assert_eq!(err4["error"]["type"], "unknown_model");

    // (e): region is declared and displayed — cn where written, intl
    // where defaulted — and the tag appears on no reporting surface.
    let health = http_get_json(&rig.listen_addr, "/health");
    let providers = health["providers"].as_array().expect("provider list");
    let region_of = |n: &str| {
        providers
            .iter()
            .find(|p| p["name"] == n)
            .unwrap_or_else(|| panic!("provider {n} on /health"))["region"]
            .clone()
    };
    assert_eq!(region_of("p-api"), "cn", "the written region is displayed");
    assert_eq!(region_of("p-plan"), "intl", "absent region ⇒ intl");

    // (b)+(d): capture the mocks' bytes before the rig is stopped.
    let plan_reqs = rig.plan.requests();
    let api_reqs = rig.api.requests();
    assert_eq!(plan_reqs.len(), 2, "both turns reached the plan mock first");
    assert_eq!(api_reqs.len(), 1, "the spilled turn reached the api mock");
    for r in &plan_reqs {
        let body = String::from_utf8_lossy(&r.body);
        assert!(
            body.contains(r#""model":"k3""#),
            "the plan route's native id is k3, body: {body}"
        );
        assert!(!body.contains("fam"), "the tag never leaves the process");
    }
    for r in &api_reqs {
        let body = String::from_utf8_lossy(&r.body);
        assert!(
            body.contains(r#""model":"kimi-k3""#),
            "the metered route's native id is kimi-k3, body: {body}"
        );
        assert!(!body.contains("fam"), "the tag never leaves the process");
    }

    let dir = rig.stop();
    let recs = trace_records(&dir);
    assert_eq!(
        recs.len(),
        4,
        "two served + two 404 refusals = four records"
    );
    // (b): requested_model is the client's string verbatim, on every
    // record — including the refusals.
    let wanted = ["p-plan/k3", "fam", "p-api/fam"];
    let requested: Vec<&str> = recs
        .iter()
        .map(|r| {
            r["decision"]["requested_model"]
                .as_str()
                .expect("requested_model")
        })
        .collect();
    let in_plan = requested.iter().filter(|m| **m == "p-plan/k3").count();
    let bare = requested.iter().filter(|m| **m == "fam").count();
    let qualified = requested.iter().filter(|m| **m == "p-api/fam").count();
    assert_eq!(in_plan, 2, "both served turns record the client string");
    assert_eq!(bare, 1, "the bare-tag refusal records the bare tag");
    assert_eq!(qualified, 1, "the provider/tag refusal records it verbatim");
    let _ = wanted;
    // ...and the tag appears nowhere in the trace's decision fields.
    for r in &recs {
        let decision = serde_json::to_string(&r["decision"]).unwrap();
        let provider = r["decision"]["provider"].as_str().unwrap_or("");
        if provider.is_empty() {
            continue; // a pre-route refusal names no route
        }
        assert!(
            !decision.contains("\"fam\""),
            "the tag is not a trace value: {decision}"
        );
    }
    // The two served records name the two different providers of one
    // family — the pairing actually routed.
    let providers_of_served: Vec<&str> = recs
        .iter()
        .filter(|r| {
            r["decision"]["provider"]
                .as_str()
                .is_some_and(|p| !p.is_empty())
        })
        .map(|r| r["decision"]["provider"].as_str().unwrap())
        .collect();
    assert!(
        providers_of_served.contains(&"p-plan") && providers_of_served.contains(&"p-api"),
        "one family, two entries: {providers_of_served:?}"
    );
}
