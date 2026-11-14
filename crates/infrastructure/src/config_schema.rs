//! JSON Schemas of the files swarmux reads, for editors (taplo's `#:schema`
//! directive, VS Code's Even Better TOML) and for CI.
//!
//! The schema of `config.toml` is generated from the key registry, so the
//! keys, their types, defaults, ranges and merge kinds in it are the ones the
//! loader enforces. The schemas of the other files are described here next to
//! the reader of each, and a test in this module checks that every property
//! they name is accepted by the real reader. The schemas are checked into
//! `schema/` at the root of the repository; a test fails when they are stale.
//!
//! No schema crate is used: swarmux's dependency tree has none, and the
//! registry already holds what a derive macro would have to be told.

use serde_json::{Map, Value, json};
use swarmux_application::THEME_ROLES;

use crate::config_registry::{KeyDef, Registry, Ty};

/// Where the schemas are published (the repository's `schema/` folder).
pub const BASE: &str = "https://raw.githubusercontent.com/amirhosseinghanipour/swarmux/main/schema";

const DRAFT: &str = "https://json-schema.org/draft/2020-12/schema";

fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(n) => json!(n),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => json!(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// The schema of one registered key.
fn key_schema(def: &KeyDef) -> Value {
    let mut s = Map::new();
    match &def.ty {
        Ty::Bool => {
            s.insert("type".into(), json!("boolean"));
        }
        Ty::Int => {
            s.insert("type".into(), json!("integer"));
        }
        Ty::Float => {
            s.insert("type".into(), json!("number"));
        }
        Ty::Str => {
            s.insert("type".into(), json!("string"));
        }
        Ty::Enum(words) => {
            s.insert("type".into(), json!("string"));
            s.insert("enum".into(), json!(words));
        }
        Ty::StrList => {
            s.insert("type".into(), json!("array"));
            s.insert("items".into(), json!({"type": "string"}));
        }
        Ty::Table => {
            s.insert("type".into(), json!("object"));
        }
        Ty::Any => {}
    }
    if let Some((lo, hi)) = def.range {
        // Whole numbers print as integers.
        let num = |x: f64| {
            if x.fract() == 0.0 && x.abs() < 1e15 {
                json!(x as i64)
            } else {
                json!(x)
            }
        };
        s.insert("minimum".into(), num(lo));
        if hi < f64::from(i32::MAX) {
            s.insert("maximum".into(), num(hi));
        }
    }
    if !def.default.is_empty()
        && let Some(v) = crate::config_layers::literal_value(&def.default)
    {
        s.insert("default".into(), toml_to_json(&v));
    }
    s.insert("description".into(), json!(def.doc));
    s.insert("x-swarmux-merge".into(), json!(def.merge.as_str()));
    s.insert("x-swarmux-effect".into(), json!(def.effect.as_str()));
    s.insert(
        "x-swarmux-tiers".into(),
        json!(
            def.tiers
                .list()
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
        ),
    );
    s.insert("x-swarmux-env".into(), json!(def.env));
    Value::Object(s)
}

fn schema_version() -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "default": 1,
        "description": "The version of this file's format. Additive changes keep it; a breaking change raises it."
    })
}

fn document(file: &str, title: &str, description: &str, body: Map<String, Value>) -> Value {
    let mut s = Map::new();
    s.insert("$schema".into(), json!(DRAFT));
    s.insert("$id".into(), json!(format!("{BASE}/{file}")));
    s.insert("title".into(), json!(title));
    s.insert("description".into(), json!(description));
    s.extend(body);
    Value::Object(s)
}

/// The schema of `config.toml`, from the registry.
pub fn config(reg: &Registry) -> Value {
    let mut props = Map::new();
    props.insert("schema_version".into(), schema_version());
    for def in reg
        .keys()
        .filter(|k| k.section() == "" && k.path != "schema_version")
    {
        props.insert(def.path.clone(), key_schema(def));
    }
    for section in reg.sections() {
        if let Some(like) = &section.open {
            props.insert(
                section.name.clone(),
                json!({
                    "type": "object",
                    "description": section.doc,
                    "additionalProperties": {"type": "object", "description": like.doc},
                }),
            );
            continue;
        }
        let mut inner = Map::new();
        for def in reg.section_keys(&section.name) {
            inner.insert(def.name().to_string(), key_schema(def));
        }
        props.insert(
            section.name.clone(),
            json!({
                "type": "object",
                "description": section.doc,
                "properties": inner,
                "additionalProperties": false,
            }),
        );
    }
    let mut body = Map::new();
    body.insert("type".into(), json!("object"));
    body.insert("properties".into(), Value::Object(props));
    body.insert("additionalProperties".into(), json!(false));
    document(
        "config.schema.json",
        "swarmux config.toml",
        "Settings for swarmux. Layers: the organisation's file, this file, a workspace's file, a repository's .swarmux/config.toml, SWARMUX_<SECTION>__<KEY> variables and --set. Every key is optional.",
        body,
    )
}

/// The schema of the organisation's `managed.toml`: the same keys, and the
/// list of keys it locks.
pub fn managed(reg: &Registry) -> Value {
    let mut s = config(reg);
    let names: Vec<String> = reg.keys().map(|k| k.path.clone()).collect();
    if let Some(props) = s.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(
            "locked".into(),
            json!({
                "type": "array",
                "items": {"type": "string", "enum": names},
                "description": "Keys whose value here wins over every other tier, the command line included. A client-side control, not a security boundary.",
            }),
        );
    }
    s["$id"] = json!(format!("{BASE}/managed.schema.json"));
    s["title"] = json!("swarmux managed.toml");
    s["description"] = json!(
        "The organisation's file (/etc/swarmux/managed.toml, %ProgramData%\\swarmux\\managed.toml). Keys it does not lock are defaults the user may change; keys listed in `locked` cannot be changed."
    );
    s
}

fn strings(description: &str) -> Value {
    json!({"type": "array", "items": {"type": "string"}, "description": description})
}

fn level() -> Value {
    json!({"type": "string", "enum": ["fast", "full", "deep"]})
}

/// The schema of a repository's `.swarmux/gates.toml`.
pub fn gates() -> Value {
    let mut body = Map::new();
    body.insert("type".into(), json!("object"));
    body.insert("additionalProperties".into(), json!(false));
    body.insert(
        "properties".into(),
        json!({
            "schema_version": schema_version(),
            "gate": {
                "type": "object",
                "additionalProperties": false,
                "description": "How the done-gate behaves.",
                "properties": {
                    "max_blocks": {"type": "integer", "minimum": 0, "default": 3, "description": "Stops blocked before the owner is asked; 0 never blocks (ignored until the repository is trusted)."},
                    "evidence": {"type": "boolean", "default": true, "description": "Test runs the agent claims must appear in what it ran."},
                    "subagent_checks": {"type": "boolean", "default": false, "description": "Subagents' stops run the checks too."},
                    "detect": {"type": "boolean", "default": true, "description": "Guess checks from the repository's files when none are listed."},
                    "protected": strings("Globs of files agents may not write. Restricts, so it applies at once."),
                    "level": level(),
                },
            },
            "check": {
                "type": "array",
                "description": "Checks that must pass. A repository's checks run only after `swarmux trust grant`.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["run"],
                    "properties": {
                        "name": {"type": "string"},
                        "run": {"type": "array", "items": {"type": "string"}, "minItems": 1, "description": "The program and its arguments. Never a shell."},
                        "timeout_secs": {"type": "integer", "minimum": 1},
                        "required": {"type": "boolean", "default": true},
                        "level": level(),
                        "on_error": {"const": "fail", "description": "The only value: a check that cannot run never passes."},
                        "env": strings("Environment variables this check may see besides the built-in allowlist."),
                    },
                },
            },
            "verifier": {
                "type": "object",
                "additionalProperties": false,
                "description": "How the independent verifier treats this repository.",
                "properties": {
                    "clean": {"type": "boolean", "default": true},
                    "test_edits": {"type": "string", "enum": ["block", "allow"], "default": "block"},
                    "base_suite": {"type": "boolean", "default": true},
                    "allow_test_changes": strings("Globs of tests that may be removed or changed on purpose (loosens: needs trust)."),
                    "substrate": strings("Harness files besides the built-in list (restricts)."),
                    "allow_substrate": strings("Harness files this repository changes on purpose (loosens: needs trust)."),
                    "revert_probe": {"type": "boolean", "default": false},
                    "worktree_checks": {"type": "string", "enum": ["escalate", "allow"], "default": "escalate"},
                    "dependencies": {"type": "string", "enum": ["flag", "block"], "default": "flag"},
                    "env": strings("Environment variables every check may see (loosens: needs trust)."),
                    "flaky_after": {"type": "integer", "minimum": 0, "default": 3},
                    "flaky_window": {"type": "integer", "minimum": 0, "default": 10},
                },
            },
        }),
    );
    document(
        "gates.schema.json",
        "swarmux .swarmux/gates.toml",
        "A repository's done-gate. An unknown key or a value that cannot be used makes the gate escalate: it never passes on a file it cannot read.",
        body,
    )
}

/// The schema of a mission contract (`.swarmux/missions/<slug>.toml`).
pub fn mission() -> Value {
    let mut body = Map::new();
    body.insert("type".into(), json!("object"));
    body.insert("additionalProperties".into(), json!(false));
    body.insert("required".into(), json!(["goal"]));
    body.insert(
        "properties".into(),
        json!({
            "schema_version": schema_version(),
            "title": {"type": "string"},
            "goal": {"type": "string", "description": "What done means, in a sentence."},
            "repos": strings("The repositories of a workspace mission, producers first."),
            "due": {"type": "string", "format": "date-time", "description": "When it is needed by: RFC 3339 with an offset."},
            "acceptance": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "checks": strings("Commands that must pass, or `repo: command` lines."),
                    "human_signoff": {"type": "boolean", "default": false},
                    "by_repo": {"type": "object", "additionalProperties": strings("Checks of one repository.")},
                },
            },
            "constraints": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "paths_rw": strings("Globs the agent may write."),
                    "protected": strings("Globs the agent may never write."),
                },
            },
            "budget": {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "usd": {"type": "number", "exclusiveMinimum": 0},
                    "wall_minutes": {"type": "integer", "minimum": 1},
                    "on_exhaust": {"type": "string", "enum": ["escalate", "warn"], "default": "escalate"},
                },
            },
            "clarifications": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["question"],
                    "properties": {
                        "question": {"type": "string"},
                        "answer": {"type": "string"},
                    },
                },
            },
        }),
    );
    document(
        "mission.schema.json",
        "swarmux mission contract",
        "A mission's contract: the goal, what done means and the limits. An unknown key refuses the contract.",
        body,
    )
}

/// The schema of the knowledge file (`swarmux knowledge export`).
pub fn knowledge() -> Value {
    let mut body = Map::new();
    body.insert("type".into(), json!("object"));
    body.insert(
        "properties".into(),
        json!({
            "schema_version": schema_version(),
            "about": {"type": "string"},
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["id", "kind", "status", "text"],
                    "properties": {
                        "id": {"type": "string"},
                        "home": {"type": "string"},
                        "kind": {"type": "string"},
                        "status": {"type": "string"},
                        "text": {"type": "string"},
                        "why": {"type": "string"},
                        "core": {"type": "boolean"},
                        "source": {"type": "string"},
                        "trust": {"type": "string"},
                        "prior": {"type": "string"},
                        "valid_for": {"type": "string"},
                        "scope": {"type": "object"},
                        "anchors": {"type": "array", "items": {"type": "object"}},
                        "enforcer": {"type": "object"},
                        "stats": {"type": "object"},
                    },
                },
            },
        }),
    );
    document(
        "knowledge.schema.json",
        "swarmux knowledge file",
        "Rules, facts and decisions, as `swarmux knowledge export` writes them and `import` reads them. Keys of an item that this version does not know are kept out, not rejected.",
        body,
    )
}

/// The schema of `swarmux keys export`.
pub fn keys() -> Value {
    let mut body = Map::new();
    body.insert("type".into(), json!("object"));
    body.insert("additionalProperties".into(), json!(false));
    body.insert(
        "properties".into(),
        json!({
            "schema_version": schema_version(),
            "keys": {
                "type": "object",
                "description": "A table per context (`global`, a screen, or a part of one); each line is an action id and one key or a list of keys.",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": {
                        "oneOf": [
                            {"type": "string"},
                            {"type": "array", "items": {"type": "string"}},
                        ],
                    },
                },
            },
        }),
    );
    document(
        "keys.schema.json",
        "swarmux keys",
        "The [keys] section as `swarmux keys export` writes it.",
        body,
    )
}

/// The schema of `swarmux theme export` (the TOML format).
pub fn theme() -> Value {
    let colour = json!({"type": "string", "pattern": "^#[0-9a-fA-F]{6}$"});
    let mut roles = Map::new();
    for (name, arity) in THEME_ROLES
        .iter()
        .chain(std::iter::once(&swarmux_application::CHART_ROLE))
    {
        let schema = if *arity == 1 {
            colour.clone()
        } else {
            json!({"type": "array", "items": colour, "minItems": arity, "maxItems": arity})
        };
        roles.insert((*name).to_string(), schema);
    }
    roles.insert(
        "base".into(),
        json!({"type": "string", "description": "The theme this one starts from."}),
    );
    let mut body = Map::new();
    body.insert("type".into(), json!("object"));
    body.insert("additionalProperties".into(), json!(false));
    body.insert(
        "properties".into(),
        json!({
            "schema_version": schema_version(),
            "theme": {"type": "string", "description": "The theme in use."},
            "themes": {
                "type": "object",
                "additionalProperties": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": roles,
                },
            },
        }),
    );
    document(
        "theme.schema.json",
        "swarmux theme",
        "A theme as `swarmux theme export` writes it: colours by role.",
        body,
    )
}

fn render(v: &Value) -> String {
    let mut text = serde_json::to_string_pretty(v).unwrap_or_default();
    text.push('\n');
    text
}

/// Every schema, as (file name, text), in a fixed order.
pub fn all(reg: &Registry) -> Vec<(&'static str, String)> {
    vec![
        ("config.schema.json", render(&config(reg))),
        ("managed.schema.json", render(&managed(reg))),
        ("gates.schema.json", render(&gates())),
        ("mission.schema.json", render(&mission())),
        ("knowledge.schema.json", render(&knowledge())),
        ("keys.schema.json", render(&keys())),
        ("theme.schema.json", render(&theme())),
    ]
}

#[cfg(test)]
mod tests;
