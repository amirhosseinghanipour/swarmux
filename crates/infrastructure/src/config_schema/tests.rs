use std::path::PathBuf;

use super::*;
use crate::gate::parse_gates;
use crate::mission::{parse_contract, render_contract};
use swarmux_application::KnowledgeCodec;
use swarmux_domain::gate::RepoFacts;
use swarmux_domain::knowledge::Home;

/// Just enough of JSON Schema to check the documents here: type, enum,
/// const, properties, additionalProperties, items, required, minimum,
/// maximum, minItems, maxItems and oneOf.
fn validate(schema: &Value, v: &Value, at: &str, out: &mut Vec<String>) {
    let ty_ok = |t: &str| match t {
        "object" => v.is_object(),
        "array" => v.is_array(),
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "integer" => v.is_i64() || v.is_u64(),
        "number" => v.is_number(),
        _ => true,
    };
    if let Some(t) = schema.get("type").and_then(Value::as_str)
        && !ty_ok(t)
    {
        out.push(format!("{at}: not a {t}: {v}"));
        return;
    }
    if schema.get("pattern") == Some(&json!("^#[0-9a-fA-F]{6}$"))
        && let Some(s) = v.as_str()
        && !(s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()))
    {
        out.push(format!("{at}: {s:?} is not a #rrggbb colour"));
    }
    if let Some(c) = schema.get("const")
        && c != v
    {
        out.push(format!("{at}: must be {c}, not {v}"));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array)
        && !e.contains(v)
    {
        out.push(format!("{at}: {v} is not one of {e:?}"));
    }
    if let (Some(n), Some(min)) = (v.as_f64(), schema.get("minimum").and_then(Value::as_f64))
        && n < min
    {
        out.push(format!("{at}: {n} is below {min}"));
    }
    if let (Some(n), Some(max)) = (v.as_f64(), schema.get("maximum").and_then(Value::as_f64))
        && n > max
    {
        out.push(format!("{at}: {n} is above {max}"));
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array) {
        let hits = one_of
            .iter()
            .filter(|s| {
                let mut e = Vec::new();
                validate(s, v, at, &mut e);
                e.is_empty()
            })
            .count();
        if hits != 1 {
            out.push(format!("{at}: matches {hits} of the oneOf, not 1: {v}"));
        }
    }
    if let Some(obj) = v.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        for req in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = req.as_str()
                && !obj.contains_key(name)
            {
                out.push(format!("{at}: {name} is required"));
            }
        }
        for (k, val) in obj {
            let here = format!("{at}/{k}");
            match props.and_then(|p| p.get(k)) {
                Some(s) => validate(s, val, &here, out),
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => out.push(format!("{here}: not a known key")),
                    Some(s @ Value::Object(_)) => validate(s, val, &here, out),
                    _ => {}
                },
            }
        }
    }
    if let Some(arr) = v.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (arr.len() as u64) < min
        {
            out.push(format!("{at}: fewer than {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (arr.len() as u64) > max
        {
            out.push(format!("{at}: more than {max} items"));
        }
        if let Some(items) = schema.get("items") {
            for (i, x) in arr.iter().enumerate() {
                validate(items, x, &format!("{at}/{i}"), out);
            }
        }
    }
}

fn errors(schema: &Value, toml_text: &str) -> Vec<String> {
    let table: toml::Table = toml::from_str(toml_text).unwrap();
    let doc = toml_to_json(&toml::Value::Table(table));
    let mut out = Vec::new();
    validate(schema, &doc, "", &mut out);
    out
}

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("schema")
}

#[test]
fn the_checked_in_schemas_are_current() {
    let dir = schema_dir();
    let update = std::env::var_os("SWARMUX_UPDATE_SCHEMA").is_some();
    let mut stale = Vec::new();
    for (file, text) in all(&Registry::core()) {
        let path = dir.join(file);
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(have) if have == text => {}
            Ok(_) => stale.push(format!("{file} differs")),
            Err(_) => stale.push(format!("{file} is missing")),
        }
    }
    assert!(
        stale.is_empty(),
        "schema/ is stale ({stale:?}): run `swarmux config schema --out schema` (or SWARMUX_UPDATE_SCHEMA=1 cargo test -p swarmux-infrastructure --lib the_checked_in) and commit it"
    );
}

#[test]
fn the_schemas_are_deterministic_and_named_after_their_files() {
    let a = all(&Registry::core());
    let b = all(&Registry::core());
    assert_eq!(a, b);
    for (file, text) in &a {
        let doc: Value = serde_json::from_str(text).unwrap();
        assert_eq!(doc["$id"], json!(format!("{BASE}/{file}")));
        assert_eq!(doc["$schema"], json!(DRAFT));
        assert!(text.ends_with('\n'));
    }
}

#[test]
fn every_registered_key_is_in_the_config_schema_with_its_default_valid() {
    let reg = Registry::core();
    let schema = config(&reg);
    for def in reg.keys().filter(|d| d.path != "schema_version") {
        let node = if def.section().is_empty() {
            schema["properties"][def.path.as_str()].clone()
        } else {
            schema["properties"][def.section()]["properties"][def.name()].clone()
        };
        assert!(!node.is_null(), "{} is not in the schema", def.path);
        assert_eq!(node["x-swarmux-merge"], json!(def.merge.as_str()));
        if let Some(v) = crate::config_layers::literal_value(&def.default) {
            let mut out = Vec::new();
            validate(&node, &toml_to_json(&v), &def.path, &mut out);
            assert!(out.is_empty(), "{}: {out:?}", def.path);
        }
    }
}

const FULL_CONFIG: &str = r#"
schema_version = 1
max_concurrent = 5
theme = "nord"
icons = "ascii"
low_power = true

[guard]
allow = ["Bash(npm test)"]
deny = ["Bash(rm -rf *)"]
unmatched = "ask"
approval_timeout_secs = 300
on_unreachable = "deny"
always_scope = "repo"
always_expiry_days = 30

[governor]
hold_at = 0.95
max_job_usd = 5.0

[notify]
terminal = "bell"

[themes.mine]
base = "nord"

[pricing.my-model]
input = 1.0
output = 2.0
"#;

#[test]
fn a_real_config_validates_and_the_reader_accepts_it() {
    let schema = config(&Registry::core());
    assert_eq!(errors(&schema, FULL_CONFIG), Vec::<String>::new());
    crate::config::parse_settings(FULL_CONFIG, std::path::Path::new("c.toml")).unwrap();
}

#[test]
fn a_wrong_config_is_flagged_by_the_schema() {
    let schema = config(&Registry::core());
    let e = errors(&schema, "[guard]\nunmatched = \"maybe\"\nnope = 1\n");
    assert!(e.iter().any(|m| m.contains("unmatched")), "{e:?}");
    assert!(e.iter().any(|m| m.contains("nope")), "{e:?}");
    let e = errors(&schema, "max_concurrent = 0\nmotion = \"wild\"\n");
    assert_eq!(e.len(), 2, "{e:?}");
}

#[test]
fn the_managed_schema_adds_locked() {
    let reg = Registry::core();
    let s = managed(&reg);
    assert!(
        errors(
            &s,
            "locked = [\"guard.unmatched\"]\n[guard]\nunmatched = \"ask\"\n"
        )
        .is_empty()
    );
    assert!(!errors(&s, "locked = [\"nope\"]\n").is_empty());
}

const FULL_GATES: &str = r#"
schema_version = 1

[gate]
max_blocks = 2
evidence = true
subagent_checks = false
detect = false
protected = ["oracles/**"]
level = "full"

[[check]]
name = "unit"
run = ["cargo", "test"]
timeout_secs = 900
required = true
level = "fast"
on_error = "fail"
env = ["MY_VAR"]

[verifier]
clean = true
test_edits = "block"
base_suite = true
allow_test_changes = ["tests/golden/**"]
substrate = ["tools/run.sh"]
allow_substrate = ["Makefile"]
revert_probe = false
worktree_checks = "escalate"
dependencies = "flag"
env = ["OTHER_VAR"]
flaky_after = 3
flaky_window = 10
"#;

#[test]
fn a_gates_file_with_every_key_validates_and_the_reader_accepts_it() {
    assert_eq!(errors(&gates(), FULL_GATES), Vec::<String>::new());
    let spec = parse_gates(FULL_GATES, &RepoFacts::default()).unwrap();
    assert_eq!(spec.checks.len(), 1);
}

#[test]
fn what_the_gates_reader_refuses_the_schema_flags_too() {
    for bad in [
        "[gate]\nmax_block = 2\n",
        "[verifier]\nhidden = true\n",
        "[[check]]\nrun = []\n",
        "[[check]]\nname = \"x\"\n",
        "[[check]]\nrun = [\"x\"]\non_error = \"pass\"\n",
    ] {
        assert!(
            parse_gates(bad, &RepoFacts::default()).is_err() || bad.contains("on_error"),
            "{bad}"
        );
        assert!(
            !errors(&gates(), bad).is_empty(),
            "the schema accepts {bad}"
        );
    }
}

#[test]
fn a_rendered_mission_contract_validates_and_reads_back() {
    let text = "schema_version = 1\ntitle = \"Fast search\"\ngoal = \"answer in 50 ms\"\n\n[acceptance]\nchecks = [\"cargo test\"]\nhuman_signoff = true\n\n[constraints]\npaths_rw = [\"src/**\"]\nprotected = [\"tests/**\"]\n\n[budget]\nusd = 2.0\nwall_minutes = 90\non_exhaust = \"warn\"\n\n[[clarifications]]\nquestion = \"Which corpus?\"\nanswer = \"bench\"\n";
    assert_eq!(errors(&mission(), text), Vec::<String>::new());
    let contract = parse_contract(text).unwrap();
    let again = render_contract(&contract);
    assert!(again.starts_with("schema_version = 1\n"), "{again}");
    assert_eq!(errors(&mission(), &again), Vec::<String>::new(), "{again}");
    assert!(!errors(&mission(), "title = \"no goal\"\n").is_empty());
}

#[test]
fn the_knowledge_file_has_a_version_and_validates() {
    let codec = crate::TomlKnowledge;
    let text = codec.encode(&[]);
    assert!(text.contains("schema_version = 1"), "{text}");
    assert_eq!(errors(&knowledge(), &text), Vec::<String>::new());
    let sample = "schema_version = 1\nabout = \"x\"\n\n[[items]]\nid = \"a\"\nkind = \"rule\"\nstatus = \"active\"\ntext = \"never edit generated files\"\n";
    assert_eq!(errors(&knowledge(), sample), Vec::<String>::new());
    let home = Home::Workspace(swarmux_domain::WorkspaceId::random());
    assert!(
        codec
            .decode("schema_version = 1\n", &home)
            .unwrap()
            .is_empty()
    );
    assert!(
        codec
            .decode("schema_version = 7\nitems = []\n", &home)
            .is_ok(),
        "a newer version is read, not refused"
    );
}

#[test]
fn a_keys_export_and_a_theme_export_validate() {
    let keys = "schema_version = 1\n\n[keys.global]\nquit = \"q\"\n\"nav.go_work\" = [\"g w\", \"w\"]\n\n[keys.queue]\n\"queue.retry\" = []\n";
    assert_eq!(errors(&super::keys(), keys), Vec::<String>::new());
    assert!(!errors(&super::keys(), "[keys.global]\nquit = 5\n").is_empty());
    let theme = "schema_version = 1\ntheme = \"nord\"\n\n[themes.nord]\nbg = \"#2e3440\"\ngauge = [\"#a3be8c\", \"#ebcb8b\", \"#bf616a\"]\n";
    assert_eq!(errors(&super::theme(), theme), Vec::<String>::new());
    assert!(!errors(&super::theme(), "[themes.x]\nbg = \"blue\"\n").is_empty());
}
