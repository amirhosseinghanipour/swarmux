//! The keys of the configuration, as data.
//!
//! Every key swarmux reads is registered here with its type, its default,
//! how its values from several tiers become one (its merge kind), whether it
//! grants or restricts, and which tiers may set it. The layering, the lint,
//! `swarmux config`, the environment overrides and the JSON Schema all read
//! this table, so none of them can disagree with the others. A practice pack
//! or an extension registers its own keys the same way: build a [`KeyDef`]
//! and call [`Registry::register`].

use std::cmp::Ordering;
use std::collections::BTreeMap;

use swarmux_domain::layers::{Effect, MergeKind, Tier, TierSet};
use toml::Value;

/// The kind of value a key holds.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Bool,
    Int,
    Float,
    Str,
    /// One of these words.
    Enum(Vec<String>),
    StrList,
    /// A table (a free-form one: `[keys]`, `[deck]`).
    Table,
    /// Anything the key's reader accepts (a number of days or the word
    /// "never", a list of tables).
    Any,
}

/// Which of two values is the stricter, for keys that merge as a lock.
#[derive(Clone, Debug, PartialEq)]
pub enum Strict {
    /// Not a lock.
    None,
    /// These words, strictest first.
    Order(Vec<String>),
    /// The smaller number is stricter; a word (such as "never") is the
    /// weakest.
    Lowest,
    /// `true` is stricter.
    True,
}

impl Strict {
    /// A rank where a smaller one is stricter; `None` when the value says
    /// nothing comparable.
    fn rank(&self, v: &Value) -> Option<f64> {
        match self {
            Strict::None => None,
            Strict::Order(words) => v
                .as_str()
                .and_then(|s| words.iter().position(|w| w == s.trim()))
                .map(|i| i as f64),
            Strict::Lowest => match v {
                Value::Integer(n) => Some(*n as f64),
                Value::Float(f) => Some(*f),
                Value::String(_) => Some(f64::INFINITY),
                _ => None,
            },
            Strict::True => v.as_bool().map(|b| if b { 0.0 } else { 1.0 }),
        }
    }

    /// Which of `a` and `b` is the stricter; `Equal` when neither is, or
    /// when they cannot be compared.
    pub fn compare(&self, a: &Value, b: &Value) -> Ordering {
        match (self.rank(a), self.rank(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => Ordering::Equal,
        }
    }
}

/// One key.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyDef {
    /// `section.key`, or just `key` for the top level.
    pub path: String,
    pub ty: Ty,
    pub merge: MergeKind,
    pub effect: Effect,
    pub strict: Strict,
    pub tiers: TierSet,
    /// May `SWARMUX_<SECTION>__<KEY>` set it.
    pub env: bool,
    /// What the key reads as when its value is not valid, as a TOML literal,
    /// for a restrictive key whose default is not the strictest. `None`: the
    /// default.
    pub invalid: Option<String>,
    /// The default as a TOML literal; empty when it is "not set".
    pub default: String,
    /// The lowest and highest number it takes, for the schema.
    pub range: Option<(f64, f64)>,
    /// Read by an extension, not by swarmux's own settings: it goes through
    /// the same layers and checks (by type and range) but is not part of
    /// [`swarmux_application::Settings`].
    pub external: bool,
    pub doc: String,
}

const OWNER_TIERS: TierSet = TierSet::of(&[Tier::Org, Tier::User, Tier::Workspace, Tier::Run]);
const NARROWING_TIERS: TierSet = TierSet::of(&[
    Tier::Org,
    Tier::User,
    Tier::Workspace,
    Tier::Pack,
    Tier::Mission,
    Tier::Run,
]);

impl KeyDef {
    pub fn new(path: &str, ty: Ty, default: &str, doc: &str) -> KeyDef {
        KeyDef {
            path: path.to_string(),
            ty,
            merge: MergeKind::Scalar,
            effect: Effect::Neutral,
            strict: Strict::None,
            tiers: OWNER_TIERS,
            env: true,
            invalid: None,
            default: default.to_string(),
            range: None,
            external: false,
            doc: doc.to_string(),
        }
    }

    pub fn merge(mut self, merge: MergeKind) -> KeyDef {
        self.merge = merge;
        self
    }

    /// Merges as a lock: the strictest of the tiers wins.
    pub fn lock(mut self, strict: Strict) -> KeyDef {
        self.merge = MergeKind::Lock;
        self.strict = strict;
        self
    }

    /// The key allows more. Honoured from a repository only with trust,
    /// never from a pack (until its manifest grants), a mission or the
    /// environment.
    pub fn grants(mut self) -> KeyDef {
        self.effect = Effect::Grants;
        self.env = false;
        self.tiers = OWNER_TIERS;
        self
    }

    /// The key allows less: any tier may narrow it, at once.
    pub fn restricts(mut self) -> KeyDef {
        self.effect = Effect::Restricts;
        self.tiers = NARROWING_TIERS;
        self
    }

    /// A repository's `.swarmux/config.toml` may set it.
    pub fn project(mut self) -> KeyDef {
        self.tiers = self.tiers.with(Tier::Project);
        self
    }

    pub fn tiers(mut self, tiers: TierSet) -> KeyDef {
        self.tiers = tiers;
        self
    }

    pub fn no_env(mut self) -> KeyDef {
        self.env = false;
        self
    }

    /// What an invalid value reads as, for a key where the default is not
    /// the strictest answer.
    pub fn invalid_as(mut self, literal: &str) -> KeyDef {
        self.invalid = Some(literal.to_string());
        self
    }

    /// The key belongs to an extension (a pack), not to swarmux's settings.
    pub fn external(mut self) -> KeyDef {
        self.external = true;
        self
    }

    pub fn range(mut self, lo: f64, hi: f64) -> KeyDef {
        self.range = Some((lo, hi));
        self
    }

    /// Whether `value` is the kind of value the key holds (and in range).
    pub fn check(&self, value: &Value) -> Result<(), String> {
        let number = match value {
            Value::Integer(n) => Some(*n as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        };
        let in_range = || match (self.range, number) {
            (Some((lo, hi)), Some(n)) if n < lo || n > hi => {
                Err(format!("{} is outside {lo} to {hi}", self.path))
            }
            _ => Ok(()),
        };
        match &self.ty {
            Ty::Bool if value.is_bool() => Ok(()),
            Ty::Int if value.is_integer() => in_range(),
            Ty::Float if number.is_some() => in_range(),
            Ty::Str if value.is_str() => Ok(()),
            Ty::Enum(words) => match value.as_str() {
                Some(s) if words.iter().any(|w| w == s.trim()) => Ok(()),
                _ => Err(format!("{} is one of {}", self.path, words.join(", "))),
            },
            Ty::StrList
                if value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_str)) =>
            {
                Ok(())
            }
            Ty::Table if value.is_table() => Ok(()),
            Ty::Any => Ok(()),
            _ => Err(format!(
                "{} has the wrong kind of value: {value}",
                self.path
            )),
        }
    }

    /// The section part of the path (`""` for a top-level key).
    pub fn section(&self) -> &str {
        self.path.rsplit_once('.').map_or("", |(s, _)| s)
    }

    /// The key part of the path.
    pub fn name(&self) -> &str {
        self.path.rsplit_once('.').map_or(&self.path, |(_, k)| k)
    }

    /// Whether the key is honoured from a repository without a trust record.
    pub fn restrictive(&self) -> bool {
        self.effect != Effect::Grants
    }
}

/// A section of the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub doc: String,
    /// Any key below it is allowed, each read as this definition
    /// (`[pricing]` has a table per model, `[themes]` one per theme).
    pub open: Option<KeyDef>,
}

/// Every key and section there is.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    keys: BTreeMap<String, KeyDef>,
    sections: BTreeMap<String, Section>,
}

impl Registry {
    pub fn register(&mut self, def: KeyDef) {
        self.keys.insert(def.path.clone(), def);
    }

    pub fn register_section(&mut self, name: &str, doc: &str) {
        self.sections
            .entry(name.to_string())
            .or_insert_with(|| Section {
                name: name.to_string(),
                doc: doc.to_string(),
                open: None,
            });
    }

    /// A section whose keys are not listed: each is read as `like`.
    pub fn register_open_section(&mut self, name: &str, doc: &str, like: KeyDef) {
        self.sections.insert(
            name.to_string(),
            Section {
                name: name.to_string(),
                doc: doc.to_string(),
                open: Some(like),
            },
        );
    }

    pub fn is_section(&self, name: &str) -> bool {
        self.sections.contains_key(name)
    }

    pub fn sections(&self) -> impl Iterator<Item = &Section> {
        self.sections.values()
    }

    /// The definition of `path`, an open section's included.
    pub fn key(&self, path: &str) -> Option<KeyDef> {
        if let Some(def) = self.keys.get(path) {
            return Some(def.clone());
        }
        let (section, name) = path.split_once('.')?;
        let open = self.sections.get(section)?.open.as_ref()?;
        let mut def = open.clone();
        def.path = format!("{section}.{name}");
        Some(def)
    }

    /// The registered keys, in path order (open sections' keys are only
    /// those that are set).
    pub fn keys(&self) -> impl Iterator<Item = &KeyDef> {
        self.keys.values()
    }

    /// The keys of one section.
    pub fn section_keys<'a>(&'a self, section: &'a str) -> impl Iterator<Item = &'a KeyDef> {
        self.keys.values().filter(move |k| k.section() == section)
    }

    /// The keys swarmux itself reads.
    pub fn core() -> Registry {
        let mut r = Registry::default();
        let list = |path: &str, default: &str, doc: &str| {
            KeyDef::new(path, Ty::StrList, default, doc).merge(MergeKind::List)
        };
        let whole = |path: &str, ty: Ty, default: &str, doc: &str| {
            KeyDef::new(path, ty, default, doc).merge(MergeKind::Allowlist)
        };
        let int = |path: &str, default: &str, lo: f64, hi: f64, doc: &str| {
            KeyDef::new(path, Ty::Int, default, doc).range(lo, hi)
        };
        let flag = |path: &str, default: &str, doc: &str| KeyDef::new(path, Ty::Bool, default, doc);
        let word = |path: &str, default: &str, doc: &str| KeyDef::new(path, Ty::Str, default, doc);
        let one_of = |path: &str, words: &[&str], default: &str, doc: &str| {
            KeyDef::new(
                path,
                Ty::Enum(words.iter().map(|w| w.to_string()).collect()),
                default,
                doc,
            )
        };
        let share = |path: &str, default: &str, doc: &str| {
            KeyDef::new(path, Ty::Float, default, doc).range(0.0, 1.0)
        };
        let order = |words: &[&str]| Strict::Order(words.iter().map(|w| w.to_string()).collect());

        r.register(
            int(
                "schema_version",
                "1",
                1.0,
                1.0,
                "The version of this file's format.",
            )
            .no_env(),
        );

        // Top level.
        r.register(int(
            "max_concurrent",
            "3",
            1.0,
            f64::from(i32::MAX),
            "Jobs that may run at the same time.",
        ));
        r.register(int(
            "min_free_ram_mb",
            "1536",
            0.0,
            f64::from(i32::MAX),
            "Queued jobs wait while free memory is below this.",
        ));
        r.register(word(
            "default_model",
            "\"sonnet\"",
            "Model for new sessions.",
        ));
        r.register(word(
            "theme",
            "\"reference\"",
            "A built-in theme, one of your [themes], `wal`, `base16:<file>` or `matugen:<file>`.",
        ));
        r.register(word(
            "layout",
            "\"command\"",
            "The dashboard layout: a built-in one or one of your [[layouts]].",
        ));
        r.register(one_of(
            "icons",
            &["ascii", "unicode", "nerd", "auto"],
            "\"unicode\"",
            "The icon set.",
        ));
        r.register(word(
            "font",
            "\"\"",
            "The terminal font to assume for icons, when it matters.",
        ));
        r.register(one_of(
            "bg",
            &["theme", "none", "solid", "transparent"],
            "\"theme\"",
            "\"none\" draws no background, so the terminal's shows through.",
        ));
        r.register(int("gaps", "1", 0.0, 4.0, "Cells between panels."));
        r.register(int("fps", "10", 1.0, 30.0, "Frames per second."));
        r.register(flag(
            "low_power",
            "false",
            "Fewer frames and no animation, for a laptop on battery.",
        ));
        r.register(one_of(
            "motion",
            &["auto", "full", "reduced", "off"],
            "\"auto\"",
            "How much the interface moves.",
        ));
        r.register(flag("splash", "true", "The splash screen at start."));
        r.register(whole(
            "layouts",
            Ty::Any,
            "",
            "Your own dashboard layouts, `[[layouts]]` tables with rows of panels.",
        ));
        r.register(whole(
            "keys",
            Ty::Table,
            "",
            "Your key bindings (see `swarmux keys list`).",
        ));
        r.register(whole(
            "deck",
            Ty::Table,
            "",
            "The Deck's panels and zones (see `swarmux deck show`).",
        ));
        r.register(KeyDef::new(
            "jev",
            Ty::Any,
            "",
            "Removed: JEV no longer exists. The section is ignored with a warning.",
        ));
        r.register_open_section(
            "themes",
            "Your own themes: `[themes.<name>]` with `base` and colours by role.",
            KeyDef::new("themes.*", Ty::Table, "", "One theme."),
        );
        r.register_open_section(
            "pricing",
            "Model prices that replace the built-in table: `[pricing.<model>]`.",
            KeyDef::new(
                "pricing.*",
                Ty::Table,
                "",
                "One model's prices per million tokens.",
            ),
        );

        // [guard]
        r.register_section("guard", "Which tool calls are allowed, denied or asked.");
        r.register(
            list(
                "guard.allow",
                "[]",
                "Allow rules (`Tool(pattern)`). Evaluated after the denies.",
            )
            .grants(),
        );
        r.register(
            list(
                "guard.deny",
                "[]",
                "Deny rules: a call that matches is refused, whatever else allows it.",
            )
            .restricts(),
        );
        r.register(
            one_of(
                "guard.unmatched",
                &["ask", "pass"],
                "\"ask\"",
                "What happens to a call no rule matches: you are asked, or Claude Code's own rules decide.",
            )
            .lock(order(&["ask", "pass"]))
            .restricts(),
        );
        r.register(int(
            "guard.approval_timeout_secs",
            "120",
            1.0,
            3600.0,
            "How long a held call waits for your answer.",
        ));
        r.register(int(
            "guard.loop_repeats",
            "2",
            0.0,
            f64::from(i32::MAX),
            "The same failing call this many times is a loop; 0 turns the loop breaker off.",
        ));
        r.register(
            one_of(
                "guard.on_unreachable",
                &["auto", "deny", "ask", "pass"],
                "\"auto\"",
                "What a job's hook does when swarmux cannot be reached: auto denies for unattended jobs and passes for the rest.",
            )
            .lock(order(&["deny", "ask", "auto", "pass"]))
            .restricts()
            .invalid_as("\"deny\""),
        );
        r.register(
            one_of(
                "guard.always_scope",
                &["repo", "workspace", "global"],
                "\"repo\"",
                "Where an \"always allow\" rule applies.",
            )
            .lock(order(&["repo", "workspace", "global"]))
            .restricts(),
        );
        r.register(
            KeyDef::new(
                "guard.always_expiry_days",
                Ty::Any,
                "30",
                "Days an \"always allow\" rule lasts, 1 to 3650, or \"never\".",
            )
            .lock(Strict::Lowest)
            .restricts(),
        );
        r.register(
            list(
                "guard.protected_paths",
                "[]",
                "Globs of files agents may not write, besides the built-in ones.",
            )
            .restricts()
            .project(),
        );
        r.register(
            flag(
                "guard.hook_on_failure_block",
                "false",
                "Add onFailure=block to the PreToolUse hook (a Claude Code that knows it).",
            )
            .lock(Strict::True)
            .restricts(),
        );

        // [review]
        r.register_section("review", "A second model reviews finished jobs.");
        r.register(flag("review.enabled", "false", "Review finished jobs."));
        r.register(word("review.model", "\"opus\"", "The reviewer's model."));
        r.register(one_of(
            "review.when",
            &["risk", "always"],
            "\"risk\"",
            "Review only risky changes, or every job.",
        ));

        // [telemetry]
        r.register_section("telemetry", "Which background sources run, and how often.");
        for (key, doc) in [
            ("usage", "Usage windows and spend."),
            ("system", "Memory and CPU."),
            ("git", "Branch and diff figures."),
            ("github", "Pull requests through `gh` (talks to GitHub)."),
            ("agents", "Agents Claude Code runs."),
        ] {
            let mut def = flag(&format!("telemetry.{key}"), "true", doc);
            if key == "github" {
                def = def.restricts();
            }
            r.register(def);
        }
        for (key, default) in [
            ("usage_secs", "15"),
            ("system_secs", "1"),
            ("git_secs", "5"),
            ("github_secs", "60"),
            ("agents_secs", "15"),
        ] {
            r.register(int(
                &format!("telemetry.{key}"),
                default,
                1.0,
                86400.0,
                "Seconds between reads.",
            ));
        }
        r.register(word(
            "telemetry.transcripts_path",
            "",
            "Where Claude Code keeps its transcripts, when it is not the usual place.",
        ));

        // [worktree]
        r.register_section("worktree", "Where job worktrees go and what they get.");
        r.register(word(
            "worktree.root",
            "",
            "Where worktrees are made; empty is ~/.swarmux/worktrees.",
        ));
        r.register(list(
            "worktree.copy",
            "[\".env\", \".env.local\", \".claude/settings.local.json\"]",
            "Files copied from the main checkout into each worktree.",
        ));
        r.register(int(
            "worktree.port_base",
            "20000",
            1024.0,
            65000.0,
            "First port handed to jobs.",
        ));
        r.register(int(
            "worktree.port_block",
            "20",
            1.0,
            1000.0,
            "Ports per job.",
        ));
        r.register(int(
            "worktree.port_slots",
            "500",
            1.0,
            1000.0,
            "How many blocks.",
        ));
        r.register(int(
            "worktree.stale_hours",
            "12",
            1.0,
            f64::from(i32::MAX),
            "Hours before an untouched worktree counts as stale.",
        ));

        // [governor]
        r.register_section(
            "governor",
            "When queued work waits, runs cheaper or runs fewer at once.",
        );
        r.register(flag(
            "governor.enabled",
            "true",
            "Hold, downgrade and limit by the usage windows.",
        ));
        r.register(share(
            "governor.hold_at",
            "0.97",
            "New jobs wait while a window is this full.",
        ));
        r.register(share(
            "governor.downgrade_at",
            "0.90",
            "Jobs use the cheaper model above this.",
        ));
        r.register(word(
            "governor.downgrade_to",
            "\"sonnet\"",
            "The cheaper model.",
        ));
        r.register(int(
            "governor.horizon_minutes",
            "30",
            1.0,
            1440.0,
            "How far ahead the forecast looks.",
        ));
        r.register(int(
            "governor.battery_concurrency",
            "2",
            1.0,
            f64::from(i32::MAX),
            "Jobs at once on battery.",
        ));
        r.register(
            KeyDef::new(
                "governor.max_job_usd",
                Ty::Float,
                "",
                "A job that has cost this much is stopped; unset is no limit.",
            )
            .lock(Strict::Lowest)
            .restricts(),
        );
        r.register(flag(
            "governor.budget_line",
            "true",
            "Show the budget line on the wall.",
        ));
        r.register(share(
            "governor.load_hold_at",
            "0.7",
            "New jobs wait while the machine is this busy.",
        ));

        // [notify]
        r.register_section("notify", "Where swarmux tells you something needs you.");
        r.register(flag("notify.enabled", "true", "Notifications at all."));
        r.register(flag(
            "notify.toast",
            "",
            "Desktop toasts (on by default on Windows).",
        ));
        r.register(one_of(
            "notify.terminal",
            &["off", "osc777", "osc9", "bell"],
            "",
            "A terminal notification escape (off on Windows, osc777 elsewhere).",
        ));
        r.register(word(
            "notify.quiet",
            "\"\"",
            "A 24-hour window with no notifications, like \"22:00-08:00\".",
        ));
        r.register(
            word(
                "notify.ntfy",
                "\"\"",
                "An ntfy topic URL: urgent items are sent there (leaves your machine).",
            )
            .grants(),
        );

        // [watchdog]
        r.register_section("watchdog", "Stuck-agent detection and recovery.");
        r.register(flag("watchdog.enabled", "true", "Watch running jobs."));
        r.register(whole(
            "watchdog.rungs",
            Ty::StrList,
            "[\"nudge\", \"fresh\", \"diagnose\", \"fork\", \"escalate\", \"replan\", \"inbox\"]",
            "The recovery ladder, in order.",
        ));
        for (key, default, doc) in [
            (
                "stall_secs",
                "120",
                "Silence before a job is called stalled.",
            ),
            (
                "stall_tool_secs",
                "600",
                "Silence during a tool call before it is called stalled.",
            ),
            ("stall_turns", "15", "Turns without progress."),
            (
                "oscillation_repeats",
                "3",
                "A back-and-forth this many times is a loop.",
            ),
            (
                "compactions",
                "2",
                "Context compactions before a job is called thrashing.",
            ),
            ("nudge_cooldown_secs", "90", "Seconds between nudges."),
            ("restart_cooldown_secs", "180", "Seconds between restarts."),
            ("max_restarts", "3", "Restarts before escalating."),
        ] {
            r.register(int(
                &format!("watchdog.{key}"),
                default,
                0.0,
                f64::from(i32::MAX),
                doc,
            ));
        }
        r.register(share(
            "watchdog.context_high",
            "0.85",
            "Context use above this is high.",
        ));
        r.register(KeyDef::new(
            "watchdog.burn_usd_per_min",
            Ty::Float,
            "1.0",
            "Dollars per minute that is too fast; 0 is off.",
        ));
        r.register(whole(
            "watchdog.escalate_to",
            Ty::StrList,
            "[\"sonnet\", \"opus\"]",
            "The models a job is moved up through before the ladder goes on.",
        ));

        // [missions]
        r.register_section("missions", "Plan-first missions.");
        r.register(flag(
            "missions.plan_first",
            "false",
            "A mission waits for your approval of its plan.",
        ));
        r.register(word(
            "missions.plan_model",
            "\"sonnet\"",
            "The planner's model.",
        ));
        r.register(share(
            "missions.drift_threshold",
            "0.3",
            "A change this far from the plan is drift.",
        ));
        r.register(flag(
            "missions.parallel",
            "true",
            "Tasks of a mission run at the same time when they can.",
        ));
        r.register(
            one_of(
                "missions.landing",
                &["auto", "manual"],
                "\"auto\"",
                "auto lands a mission when the gate passes; manual waits for you.",
            )
            .lock(order(&["manual", "auto"]))
            .restricts(),
        );

        // [console]
        r.register_section("console", "The Work and Deck screens.");
        r.register(int(
            "console.metrics_hours",
            "24",
            1.0,
            720.0,
            "How many hours back the verification figures look.",
        ));
        r.register(whole(
            "console.send_back_notes",
            Ty::StrList,
            "",
            "The notes offered when you send work back.",
        ));

        // [board]
        r.register_section(
            "board",
            "The Board: shared state between the agents of a mission.",
        );
        r.register(flag("board.enabled", "true", "Mission jobs get the board."));

        // [followups]
        r.register_section("followups", "Follow-through: what agents leave behind.");
        r.register(flag("followups.enabled", "true", "Collect follow-ups."));
        r.register(flag(
            "followups.extract",
            "true",
            "Find follow-ups in what agents say (calls a model).",
        ));
        r.register(word(
            "followups.extract_model",
            "\"haiku\"",
            "The model that finds them.",
        ));
        r.register(whole(
            "followups.brief_times",
            Ty::StrList,
            "",
            "Times of day for the owner brief, like \"09:00\".",
        ));
        r.register(int(
            "followups.max_cards",
            "5",
            1.0,
            100.0,
            "Cards in the brief.",
        ));
        r.register(int(
            "followups.defer_hours",
            "24",
            1.0,
            8760.0,
            "How long \"later\" is.",
        ));
        r.register(int(
            "followups.spot_check_days",
            "7",
            1.0,
            365.0,
            "Days between spot checks.",
        ));
        r.register(
            list(
                "followups.protected",
                "[]",
                "Globs a standing order may never touch.",
            )
            .restricts(),
        );
        r.register(whole(
            "followups.order",
            Ty::Any,
            "",
            "Standing orders: `[[followups.order]]` tables that act on follow-ups by themselves.",
        )
        .grants());
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_knows_its_section_and_name() {
        let r = Registry::core();
        let k = r.key("guard.unmatched").unwrap();
        assert_eq!((k.section(), k.name()), ("guard", "unmatched"));
        let top = r.key("theme").unwrap();
        assert_eq!((top.section(), top.name()), ("", "theme"));
    }

    #[test]
    fn an_open_section_reads_any_key_like_its_definition() {
        let r = Registry::core();
        let k = r.key("pricing.my-model").unwrap();
        assert_eq!(k.path, "pricing.my-model");
        assert_eq!(k.ty, Ty::Table);
        assert!(r.key("guard.nope").is_none());
    }

    #[test]
    fn granting_keys_are_neither_environment_nor_pack_settable() {
        let r = Registry::core();
        for path in ["guard.allow", "notify.ntfy", "followups.order"] {
            let k = r.key(path).unwrap();
            assert_eq!(k.effect, Effect::Grants, "{path}");
            assert!(!k.env, "{path}");
            assert!(!k.tiers.contains(Tier::Pack), "{path}");
            assert!(!k.tiers.contains(Tier::Mission), "{path}");
            assert!(!k.tiers.contains(Tier::Project), "{path}");
        }
    }

    #[test]
    fn strictness_ranks_words_numbers_and_flags() {
        let order = Strict::Order(vec!["deny".into(), "ask".into(), "pass".into()]);
        let (deny, pass) = (Value::from("deny"), Value::from("pass"));
        assert_eq!(order.compare(&deny, &pass), Ordering::Less);
        assert_eq!(
            Strict::Lowest.compare(&Value::from(7), &Value::from("never")),
            Ordering::Less
        );
        assert_eq!(
            Strict::True.compare(&Value::from(true), &Value::from(false)),
            Ordering::Less
        );
        // Words it does not know compare as equal: they do not win.
        assert_eq!(order.compare(&Value::from("x"), &pass), Ordering::Equal);
    }

    #[test]
    fn a_pack_can_register_its_own_keys_the_same_way() {
        let mut r = Registry::core();
        r.register_section("acme", "A practice pack's settings.");
        r.register(
            KeyDef::new(
                "acme.max_diff_lines",
                Ty::Int,
                "400",
                "Lines a change may have.",
            )
            .lock(Strict::Lowest)
            .restricts(),
        );
        let k = r.key("acme.max_diff_lines").unwrap();
        assert_eq!(k.merge, MergeKind::Lock);
        assert!(r.is_section("acme"));
    }
}
