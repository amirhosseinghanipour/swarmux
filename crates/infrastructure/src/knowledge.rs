//! Knowledge on disk: items and events as JSON for the database, a TOML
//! file per workspace or repository that owners can keep in git, and the
//! reader that hashes anchors.

use std::path::Path;

use serde::{Deserialize, Serialize};
use swarmux_application::{AnchorReader, KnowledgeCodec};
use swarmux_domain::knowledge::{
    Action, Anchor, Change, CheckRule, Counters, Enforcer, GuardRule, Home, Item, ItemId, Kind,
    Provenance, Scope, Source, Status, Trust,
};

/// Largest anchored file read, in bytes: bigger ones hash as missing.
const ANCHOR_MAX: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct ScopeDto {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    repos: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    classes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct AnchorDto {
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    symbol: Option<String>,
    #[serde(default)]
    hash: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum EnforcerDto {
    Guard {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<String>,
        action: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
    },
    Gate {
        command: String,
    },
    Lint {
        command: String,
    },
    Prompt,
}

/// An item as stored and as written to a file. Counters and times are
/// left out of files (`counters` false), so the file only changes when
/// the knowledge does.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct ItemDto {
    id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    home: String,
    kind: String,
    status: String,
    text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    why: String,
    #[serde(default, skip_serializing_if = "is_false")]
    core: bool,
    #[serde(default = "owner")]
    source: String,
    #[serde(default = "verified")]
    trust: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prior: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    valid_for: String,
    #[serde(default)]
    scope: ScopeDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    anchors: Vec<AnchorDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enforcer: Option<EnforcerDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stats: Option<StatsDto>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct StatsDto {
    #[serde(default)]
    events: Vec<u64>,
    #[serde(default)]
    hits: u64,
    #[serde(default)]
    shadow_hits: u64,
    #[serde(default)]
    last_used: Option<i64>,
    #[serde(default)]
    last_check: Option<bool>,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: i64,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn owner() -> String {
    "owner".into()
}

fn verified() -> String {
    "verified".into()
}

fn enforcer_to(e: &Enforcer) -> EnforcerDto {
    match e {
        Enforcer::Guard(g) => EnforcerDto::Guard {
            tools: g.tools.clone(),
            paths: g.paths.clone(),
            pattern: g.pattern.clone(),
            action: g.action.as_str().into(),
            message: g.message.clone(),
        },
        Enforcer::Gate(c) => EnforcerDto::Gate {
            command: c.command.clone(),
        },
        Enforcer::Lint(c) => EnforcerDto::Lint {
            command: c.command.clone(),
        },
        Enforcer::Prompt => EnforcerDto::Prompt,
    }
}

fn enforcer_from(e: EnforcerDto) -> Result<Enforcer, String> {
    Ok(match e {
        EnforcerDto::Guard {
            tools,
            paths,
            pattern,
            action,
            message,
        } => Enforcer::Guard(GuardRule {
            tools,
            paths,
            pattern,
            action: Action::parse(&action).ok_or_else(|| format!("unknown action {action:?}"))?,
            message,
        }),
        EnforcerDto::Gate { command } => Enforcer::Gate(CheckRule { command }),
        EnforcerDto::Lint { command } => Enforcer::Lint(CheckRule { command }),
        EnforcerDto::Prompt => Enforcer::Prompt,
    })
}

fn to_dto(i: &Item, stats: bool) -> ItemDto {
    ItemDto {
        id: i.id.to_string(),
        home: if stats { i.home.key() } else { String::new() },
        kind: i.kind.as_str().into(),
        status: i.status.as_str().into(),
        text: i.text.clone(),
        why: i.why.clone(),
        core: i.core,
        source: i.provenance.source.as_str().into(),
        trust: i.trust.as_str().into(),
        prior: i.prior.map(|s| s.as_str().to_string()),
        valid_for: i.valid_for.clone(),
        scope: ScopeDto {
            repos: i.scope.repos.clone(),
            paths: i.scope.paths.clone(),
            classes: i.scope.classes.clone(),
        },
        anchors: i
            .anchors
            .iter()
            .map(|a| AnchorDto {
                path: a.path.clone(),
                symbol: a.symbol.clone(),
                hash: a.hash.clone(),
            })
            .collect(),
        enforcer: i.enforcer.as_ref().map(enforcer_to),
        stats: stats.then(|| StatsDto {
            events: i.provenance.events.clone(),
            hits: i.counters.hits,
            shadow_hits: i.counters.shadow_hits,
            last_used: i.counters.last_used,
            last_check: i.last_check,
            created_at: i.created_at,
            updated_at: i.updated_at,
        }),
    }
}

fn from_dto(d: ItemDto, home: Option<&Home>) -> Result<Item, String> {
    let id = ItemId::parse(&d.id).ok_or_else(|| format!("bad id {:?}", d.id))?;
    let home = match home {
        Some(h) => h.clone(),
        None => Home::parse(&d.home).ok_or_else(|| format!("{id}: bad home {:?}", d.home))?,
    };
    let kind = Kind::parse(&d.kind).ok_or_else(|| format!("{id}: unknown kind {:?}", d.kind))?;
    if id.kind() != Some(kind) {
        return Err(format!("{id} is not a {} id", kind.as_str()));
    }
    let status =
        Status::parse(&d.status).ok_or_else(|| format!("{id}: unknown status {:?}", d.status))?;
    let stats = d.stats.unwrap_or_default();
    Ok(Item {
        id,
        home,
        kind,
        text: d.text,
        why: d.why,
        scope: Scope {
            repos: d.scope.repos,
            paths: d.scope.paths,
            classes: d.scope.classes,
        },
        anchors: d
            .anchors
            .into_iter()
            .map(|a| Anchor {
                path: a.path,
                symbol: a.symbol,
                hash: a.hash,
            })
            .collect(),
        valid_for: d.valid_for,
        status,
        prior: d.prior.as_deref().and_then(Status::parse),
        provenance: Provenance {
            source: Source::parse(&d.source).unwrap_or(Source::Owner),
            events: stats.events,
        },
        trust: Trust::parse(&d.trust).unwrap_or(Trust::Verified),
        core: d.core,
        enforcer: d.enforcer.map(enforcer_from).transpose()?,
        counters: Counters {
            hits: stats.hits,
            shadow_hits: stats.shadow_hits,
            last_used: stats.last_used,
        },
        last_check: stats.last_check,
        created_at: stats.created_at,
        updated_at: stats.updated_at,
    })
}

/// An item as the database keeps it.
pub fn item_json(i: &Item) -> String {
    serde_json::to_string(&to_dto(i, true)).unwrap_or_default()
}

pub fn item_from_json(text: &str) -> Result<Item, String> {
    let dto: ItemDto = serde_json::from_str(text).map_err(|e| e.to_string())?;
    from_dto(dto, None)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "lowercase")]
enum ChangeDto {
    Put {
        item: Box<ItemDto>,
    },
    Status {
        to: String,
        note: String,
    },
    Confirmed {
        anchors: Vec<AnchorDto>,
        valid_for: String,
    },
    Used {
        hits: u64,
        shadow: bool,
    },
    Checked {
        passed: bool,
    },
    Trusted {
        trust: String,
    },
}

/// A change as the database keeps it.
pub fn change_json(c: &Change) -> String {
    let dto = match c {
        Change::Put(i) => ChangeDto::Put {
            item: Box::new(to_dto(i, true)),
        },
        Change::Status { to, note } => ChangeDto::Status {
            to: to.as_str().into(),
            note: note.clone(),
        },
        Change::Confirmed { anchors, valid_for } => ChangeDto::Confirmed {
            anchors: anchors
                .iter()
                .map(|a| AnchorDto {
                    path: a.path.clone(),
                    symbol: a.symbol.clone(),
                    hash: a.hash.clone(),
                })
                .collect(),
            valid_for: valid_for.clone(),
        },
        Change::Used { hits, shadow } => ChangeDto::Used {
            hits: *hits,
            shadow: *shadow,
        },
        Change::Checked { passed } => ChangeDto::Checked { passed: *passed },
        Change::Trusted(t) => ChangeDto::Trusted {
            trust: t.as_str().into(),
        },
    };
    serde_json::to_string(&dto).unwrap_or_default()
}

pub fn change_from_json(text: &str) -> Result<Change, String> {
    let dto: ChangeDto = serde_json::from_str(text).map_err(|e| e.to_string())?;
    Ok(match dto {
        ChangeDto::Put { item } => Change::Put(Box::new(from_dto(*item, None)?)),
        ChangeDto::Status { to, note } => Change::Status {
            to: Status::parse(&to).ok_or_else(|| format!("unknown status {to:?}"))?,
            note,
        },
        ChangeDto::Confirmed { anchors, valid_for } => Change::Confirmed {
            anchors: anchors
                .into_iter()
                .map(|a| Anchor {
                    path: a.path,
                    symbol: a.symbol,
                    hash: a.hash,
                })
                .collect(),
            valid_for,
        },
        ChangeDto::Used { hits, shadow } => Change::Used { hits, shadow },
        ChangeDto::Checked { passed } => Change::Checked { passed },
        ChangeDto::Trusted { trust } => {
            Change::Trusted(Trust::parse(&trust).ok_or_else(|| format!("unknown trust {trust:?}"))?)
        }
    })
}

#[derive(Serialize, Deserialize)]
struct FileDto {
    /// What the file is; the home is the one it is imported into.
    #[serde(default)]
    about: String,
    #[serde(default)]
    items: Vec<ItemDto>,
}

/// The knowledge file: TOML, one `[[items]]` table per item.
#[derive(Clone, Copy, Debug, Default)]
pub struct TomlKnowledge;

impl KnowledgeCodec for TomlKnowledge {
    fn encode(&self, items: &[Item]) -> String {
        let file = FileDto {
            about: "swarmux knowledge: rules, facts and decisions. Edit, then `swarmux knowledge import`.".into(),
            items: items.iter().map(|i| to_dto(i, false)).collect(),
        };
        toml::to_string_pretty(&file).unwrap_or_default()
    }

    fn decode(&self, text: &str, home: &Home) -> Result<Vec<Item>, String> {
        let file: FileDto = toml::from_str(text).map_err(|e| e.to_string())?;
        file.items
            .into_iter()
            .map(|d| from_dto(d, Some(home)))
            .collect()
    }
}

/// Reads anchored files from disk.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsAnchorReader;

impl AnchorReader for FsAnchorReader {
    fn read(&self, root: &Path, rel: &str) -> Option<String> {
        if rel.split('/').any(|part| part == "..") {
            return None;
        }
        let path = root.join(rel);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > ANCHOR_MAX {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swarmux_domain::WorkspaceId;
    use swarmux_domain::knowledge::{Draft, Event};

    fn sample() -> Item {
        let home = Home::Workspace(WorkspaceId::random());
        let mut d = Draft::new(home, Kind::Rule, "Anonymous users are full users.");
        d.why = "the backend relies on it".into();
        d.scope.paths = vec!["backend/supabase/migrations/**".into()];
        d.anchors = vec![Anchor::parse("backend/auth.sql#is_anonymous")];
        d.enforcer = Some(Enforcer::Guard(GuardRule {
            tools: vec!["Edit".into()],
            paths: vec!["backend/**".into()],
            pattern: Some("is_anonymous".into()),
            action: Action::Ask,
            message: "show the owner".into(),
        }));
        let anchors = d.anchors.clone();
        let mut i = d.into_item(ItemId::parse("R4").unwrap(), anchors, 7);
        i.anchors[0].hash = "abc".into();
        i.counters.hits = 3;
        i.provenance.events = vec![1, 9];
        i
    }

    #[test]
    fn items_and_changes_round_trip_through_json() {
        let i = sample();
        assert_eq!(item_from_json(&item_json(&i)).unwrap(), i);
        for c in [
            Change::Put(Box::new(i.clone())),
            Change::Status {
                to: Status::Stale,
                note: "n".into(),
            },
            Change::Confirmed {
                anchors: i.anchors.clone(),
                valid_for: "v".into(),
            },
            Change::Used {
                hits: 2,
                shadow: true,
            },
            Change::Checked { passed: false },
            Change::Trusted(Trust::Untrusted),
        ] {
            assert_eq!(change_from_json(&change_json(&c)).unwrap(), c);
        }
        let _ = Event {
            seq: 0,
            at: 0,
            item: i.id.clone(),
            actor: String::new(),
            change: Change::Checked { passed: true },
            ledger: None,
        };
    }

    #[test]
    fn the_file_is_readable_toml_without_counters() {
        let i = sample();
        let text = TomlKnowledge.encode(std::slice::from_ref(&i));
        assert!(text.contains("[[items]]"), "{text}");
        assert!(
            text.contains("text = \"Anonymous users are full users.\""),
            "{text}"
        );
        assert!(!text.contains("hits"), "{text}");
        let back = TomlKnowledge.decode(&text, &i.home).unwrap();
        assert_eq!(back[0].text, i.text);
        assert_eq!(back[0].enforcer, i.enforcer);
        assert_eq!(back[0].anchors, i.anchors);
        assert_eq!(back[0].counters.hits, 0);
        assert!(TomlKnowledge.decode("items = 3", &i.home).is_err());
        let wrong = text.replace("id = \"R4\"", "id = \"F4\"");
        assert!(
            TomlKnowledge.decode(&wrong, &i.home).is_err(),
            "id and kind disagree"
        );
    }

    #[test]
    fn anchors_are_read_inside_the_root_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a/b.sql"), "x").unwrap();
        assert_eq!(
            FsAnchorReader.read(dir.path(), "a/b.sql").as_deref(),
            Some("x")
        );
        assert_eq!(FsAnchorReader.read(dir.path(), "a/missing"), None);
        assert_eq!(FsAnchorReader.read(dir.path(), "../x"), None);
        assert_eq!(FsAnchorReader.read(dir.path(), "a"), None, "a folder");
    }
}
