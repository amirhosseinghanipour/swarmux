use super::*;
use crate::config::parse_settings;
use crate::config_registry::{KeyDef, Strict};
use swarmux_domain::Unmatched;
use swarmux_domain::layers::{Effect, MergeKind, Tier, TierSet};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

fn user_only(text: &str) -> Resolved {
    let home = Home::new();
    let user = home.write("config.toml", text);
    resolve(
        &Registry::core(),
        &Inputs {
            user: Some(user),
            ..Inputs::default()
        },
    )
}

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn entry<'a>(r: &'a Resolved, path: &str) -> &'a Entry {
    r.get(path).unwrap_or_else(|| panic!("no key {path}"))
}

fn said(r: &Resolved, needle: &str) -> bool {
    r.findings.iter().any(|f| f.to_string().contains(needle))
}

/// A realistic config of an existing install: every section there is.
const EXISTING: &str = r#"
max_concurrent = 5
min_free_ram_mb = 2048
default_model = "opus"
theme = "nord"
icons = "ascii"
gaps = 2
fps = 20
motion = "reduced"
splash = false

[guard]
allow = ["Bash(npm test)", "Read(**)"]
deny = ["Bash(rm -rf *)"]
unmatched = "pass"
approval_timeout_secs = 300
loop_repeats = 4
on_unreachable = "ask"
always_scope = "workspace"
always_expiry_days = 60
protected_paths = ["secrets/**"]
hook_on_failure_block = true

[review]
enabled = true
model = "haiku"
when = "always"

[telemetry]
github = false
usage_secs = 30
transcripts_path = "/data/claude"

[worktree]
copy = [".env"]
port_base = 30000
stale_hours = 24

[governor]
hold_at = 0.95
downgrade_at = 0.8
max_job_usd = 5.0

[notify]
toast = false
terminal = "bell"
quiet = "22:00-08:00"

[watchdog]
stall_secs = 300
max_restarts = 5
escalate_to = ["opus"]

[missions]
plan_first = true
landing = "manual"

[console]
metrics_hours = 48
send_back_notes = ["Fix the failing test first."]

[board]
enabled = false

[followups]
max_cards = 9
brief_times = ["09:00"]

[themes.mine]
base = "nord"

[keys.global]
quit = "ctrl+q"
"#;

// ---------------------------------------------------------------------
// An existing install keeps working
// ---------------------------------------------------------------------

#[test]
fn an_existing_config_gives_the_same_settings_as_before_and_no_findings() {
    let layered = user_only(EXISTING);
    let direct = parse_settings(EXISTING, Path::new("config.toml")).unwrap();
    assert!(
        layered.findings.is_empty(),
        "nothing is wrong with it: {:#?}",
        layered.findings
    );
    let mut got = layered.settings.clone();
    got.warnings.clear();
    got.provenance = Default::default();
    let mut want = direct;
    want.warnings.clear();
    assert_eq!(got, want);
    assert_eq!(layered.settings.max_concurrent, 5);
    assert_eq!(layered.settings.guard.unmatched, Unmatched::Pass);
}

#[test]
fn no_file_at_all_is_the_defaults() {
    let r = resolve(&Registry::core(), &Inputs::default());
    assert_eq!(
        r.settings.max_concurrent,
        Settings::default().max_concurrent
    );
    assert!(r.findings.is_empty());
    assert!(r.settings.warnings.is_empty());
}

#[test]
fn every_default_in_the_registry_is_what_the_settings_default_to() {
    let reg = Registry::core();
    let want = Settings::default();
    for def in reg.keys() {
        if def.default.is_empty() || def.path == "schema_version" || def.path == "jev" {
            continue;
        }
        let value = literal_value(&def.default)
            .unwrap_or_else(|| panic!("{}: {} is not a TOML literal", def.path, def.default));
        let mut table = Table::new();
        set_path(&mut table, &def.path, value);
        let mut got = settings_from_table(&table, "", Path::new("c.toml"))
            .unwrap_or_else(|e| panic!("{} = {}: {}", def.path, def.default, e.message));
        got.warnings.clear();
        assert_eq!(
            got, want,
            "{} = {} is not what the default is",
            def.path, def.default
        );
    }
}

#[test]
fn every_registered_key_is_read_by_the_loader() {
    // A key that is registered but that the reader does not know would be
    // dropped at the first merge: setting each to its default must be clean.
    let reg = Registry::core();
    let mut text = String::new();
    for def in reg.keys() {
        if def.default.is_empty() || def.path.contains('.') || def.path == "schema_version" {
            continue;
        }
        text.push_str(&format!("{} = {}\n", def.path, def.default));
    }
    for section in reg.sections() {
        let mut body = String::new();
        for def in reg.section_keys(&section.name) {
            if !def.default.is_empty() {
                body.push_str(&format!("{} = {}\n", def.name(), def.default));
            }
        }
        if !body.is_empty() {
            text.push_str(&format!("[{}]\n{body}", section.name));
        }
    }
    let r = user_only(&text);
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
}

// ---------------------------------------------------------------------
// Per-line tolerance
// ---------------------------------------------------------------------

#[test]
fn a_bad_value_costs_only_its_own_line() {
    let r = user_only(
        "max_concurrent = 7\n[guard]\napproval_timeout_secs = 0\nloop_repeats = 5\n[review]\nmodel = \"two words\"\nenabled = true\n",
    );
    assert_eq!(r.settings.max_concurrent, 7);
    assert_eq!(r.settings.guard.loop_repeats, 5);
    assert!(r.settings.review.enabled, "the rest of [review] stands");
    assert_eq!(
        r.settings.guard.approval_timeout_secs,
        Settings::default().guard.approval_timeout_secs,
        "the bad value falls back to the default"
    );
    assert_eq!(r.settings.review.model, Settings::default().review.model);
    assert!(said(&r, "approval_timeout_secs"), "{:#?}", r.findings);
    assert!(said(&r, "model"), "{:#?}", r.findings);
    let f = r
        .findings
        .iter()
        .find(|f| f.key.as_deref() == Some("guard.approval_timeout_secs"))
        .unwrap();
    assert_eq!(f.level, Level::Error);
    assert_eq!(f.line, Some(3), "it names the line");
    assert!(
        r.settings
            .warnings
            .iter()
            .any(|w| w.contains("approval_timeout_secs")),
        "and the TUI shows it"
    );
}

#[test]
fn an_unknown_key_warns_and_fails_only_under_strict() {
    let r = user_only("max_concurrent = 2\nfuture_setting = 1\n[guard]\nhelpful = true\n");
    assert_eq!(r.settings.max_concurrent, 2);
    assert!(said(&r, "future_setting"));
    assert!(said(&r, "guard.helpful"));
    assert!(r.findings.iter().all(|f| f.level == Level::Warn));
    assert!(r.failing(false).is_empty(), "not an error");
    assert_eq!(r.failing(true).len(), 2, "but --strict fails on both");
}

#[test]
fn an_unknown_section_is_one_finding() {
    let r = user_only("[nonsense]\nx = 1\n");
    assert!(said(&r, "nonsense"), "{:#?}", r.findings);
}

#[test]
fn a_restrictive_key_with_a_bad_value_reads_as_restrictive() {
    let r = user_only("[guard]\non_unreachable = \"sometimes\"\n");
    assert_eq!(
        r.settings.guard.on_unreachable.as_str(),
        "deny",
        "a hook that cannot reach swarmux denies when the value is unreadable"
    );
    assert!(said(&r, "restrictive"), "{:#?}", r.findings);
}

#[test]
fn a_list_with_one_bad_entry_keeps_the_good_ones() {
    let r = user_only("[guard]\ndeny = [\"Bash(rm -rf *)\", \"Bash(\", \"Write(/etc/**)\"]\n");
    assert_eq!(r.settings.guard.deny.len(), 2);
    assert!(said(&r, "Bash("), "{:#?}", r.findings);
}

#[test]
fn a_line_that_is_not_toml_is_dropped_and_the_rest_is_read() {
    let r = user_only("max_concurrent = 4\nthis is not toml\nmin_free_ram_mb = 100\n");
    assert_eq!(r.settings.max_concurrent, 4);
    assert_eq!(r.settings.min_free_ram_mb, 100);
    let f = r.findings.iter().find(|f| f.kind == Kind::Syntax).unwrap();
    assert_eq!(f.line, Some(2));
    assert_eq!(f.level, Level::Error);
}

#[test]
fn a_broken_header_takes_its_block_with_it_instead_of_misfiling_its_keys() {
    let r = user_only("[guard\nunmatched = \"pass\"\n[review]\nenabled = true\n");
    assert_eq!(
        r.settings.guard.unmatched,
        Unmatched::Ask,
        "the key of the broken block is not read as a top-level key"
    );
    assert!(r.settings.review.enabled);
}

#[test]
fn a_file_that_is_garbage_never_stops_the_app() {
    let r = user_only("\u{0}\u{1} = = = [[[ ]]] \"unterminated\n{{{\n");
    assert_eq!(
        r.settings.max_concurrent,
        Settings::default().max_concurrent
    );
    assert!(r.findings.iter().any(|f| f.level == Level::Error));
}

#[test]
fn a_section_left_from_jev_is_said_to_be_removed_and_ignored() {
    let r = user_only("[jev]\nenabled = true\napi_key = \"sk-xyz\"\n");
    let f = r.findings.iter().find(|f| f.kind == Kind::Removed).unwrap();
    assert!(f.message.contains("JEV was removed"), "{}", f.message);
    assert!(!f.message.contains("sk-xyz"), "never shows a value");
    assert!(r.merged.get("jev").is_none(), "the section is not kept");
}

#[test]
fn a_newer_schema_version_is_said_and_the_known_keys_are_still_read() {
    let r = user_only("schema_version = 99\nmax_concurrent = 6\n");
    assert_eq!(r.settings.max_concurrent, 6);
    assert!(r.findings.iter().any(|f| f.kind == Kind::Schema));
}

// ---------------------------------------------------------------------
// Tiers and merge kinds
// ---------------------------------------------------------------------

#[test]
fn explain_shows_the_chain_of_a_lock_key() {
    let home = Home::new();
    let user = home.write("config.toml", "[guard]\nunmatched = \"pass\"\n");
    let ws = home.write("ws.toml", "[guard]\nunmatched = \"ask\"\n");
    let r = resolve(
        &Registry::core(),
        &Inputs {
            user: Some(user),
            workspace: Some(ws),
            ..Inputs::default()
        },
    );
    let e = entry(&r, "guard.unmatched");
    assert_eq!(e.value, Some(Value::from("ask")), "the strictest wins");
    assert_eq!(e.tier, Tier::Workspace);
    assert_eq!(e.merge, MergeKind::Lock);
    assert_eq!(e.chain.len(), 3, "the default, the user, the workspace");
    assert_eq!(e.chain[0].tier, Tier::Core);
    assert_eq!(e.chain[1].tier, Tier::User);
    assert_eq!(e.chain[1].line, Some(2), "with the line it was written on");
    assert_eq!(e.overridden.len(), 2, "the default and the user");
    assert!(
        e.overridden
            .iter()
            .any(|c| c.value == Value::from("pass") && c.tier == Tier::User)
    );
    assert_eq!(r.settings.guard.unmatched, Unmatched::Ask);
}

#[test]
fn the_user_can_loosen_a_lock_key_the_default_made_strict() {
    let r = user_only("[guard]\nunmatched = \"pass\"\n");
    assert_eq!(r.settings.guard.unmatched, Unmatched::Pass);
}

#[test]
fn scalars_take_the_nearest_tier() {
    let home = Home::new();
    let user = home.write("config.toml", "max_concurrent = 4\ntheme = \"nord\"\n");
    let ws = home.write("ws.toml", "max_concurrent = 2\n");
    let r = resolve(
        &Registry::core(),
        &Inputs {
            user: Some(user),
            workspace: Some(ws),
            env: env(&[("SWARMUX__MAX_CONCURRENT", "9")]),
            ..Inputs::default()
        },
    );
    let e = entry(&r, "max_concurrent");
    assert_eq!(e.value, Some(Value::from(9)));
    assert_eq!(e.tier, Tier::Run);
    assert_eq!(e.source, "environment");
    assert_eq!(
        e.overridden.len(),
        3,
        "the default, the user and the workspace"
    );
    assert_eq!(entry(&r, "theme").tier, Tier::User);
    assert_eq!(r.settings.ui.theme, "nord");
}

#[test]
fn lists_are_the_union_of_every_tier_without_repeats() {
    let home = Home::new();
    let user = home.write(
        "config.toml",
        "[guard]\ndeny = [\"Bash(rm -rf *)\", \"Bash(sudo *)\"]\n",
    );
    let ws = home.write(
        "ws.toml",
        "[guard]\ndeny = [\"Bash(sudo *)\", \"Write(/etc/**)\"]\n",
    );
    let r = resolve(
        &Registry::core(),
        &Inputs {
            user: Some(user),
            workspace: Some(ws),
            ..Inputs::default()
        },
    );
    assert_eq!(
        entry(&r, "guard.deny").value,
        Some(Value::Array(vec![
            "Bash(rm -rf *)".into(),
            "Bash(sudo *)".into(),
            "Write(/etc/**)".into(),
        ]))
    );
    assert_eq!(r.settings.guard.deny.len(), 3);
}

#[test]
fn an_allowlist_is_taken_whole_from_the_highest_tier() {
    let home = Home::new();
    let user = home.write(
        "config.toml",
        "[watchdog]\nescalate_to = [\"opus\", \"fable\"]\n",
    );
    let ws = home.write("ws.toml", "[watchdog]\nescalate_to = [\"sonnet\"]\n");
    let r = resolve(
        &Registry::core(),
        &Inputs {
            user: Some(user),
            workspace: Some(ws),
            ..Inputs::default()
        },
    );
    assert_eq!(
        entry(&r, "watchdog.escalate_to").value,
        Some(Value::Array(vec!["sonnet".into()]))
    );
}

#[test]
fn a_managed_lock_overrides_what_the_user_set() {
    let home = Home::new();
    let managed = home.write(
        "managed.toml",
        "locked = [\"guard.unmatched\", \"guard.on_unreachable\"]\n[guard]\nunmatched = \"ask\"\non_unreachable = \"deny\"\n",
    );
    let user = home.write(
        "config.toml",
        "[guard]\nunmatched = \"pass\"\non_unreachable = \"pass\"\n",
    );
    let r = resolve(
        &Registry::core(),
        &Inputs {
            managed: Some(managed),
            user: Some(user),
            // Even what is typed on the command line does not beat a lock.
            flags: vec![("guard.unmatched".into(), "pass".into())],
            ..Inputs::default()
        },
    );
    assert_eq!(r.settings.guard.unmatched, Unmatched::Ask);
    assert_eq!(r.settings.guard.on_unreachable.as_str(), "deny");
    let e = entry(&r, "guard.unmatched");
    assert_eq!(e.tier, Tier::Managed);
    assert!(e.locked_by.as_deref().unwrap().ends_with("managed.toml"));
    assert_eq!(
        e.overridden.len(),
        3,
        "the default, the user's and the flag are overridden"
    );
    assert_eq!(r.locked.len(), 2);
}

#[test]
fn what_managed_did_not_lock_is_a_default_the_user_may_change() {
    let home = Home::new();
    let managed = home.write("managed.toml", "max_concurrent = 2\ntheme = \"nord\"\n");
    let user = home.write("config.toml", "max_concurrent = 6\n");
    let r = resolve(
        &Registry::core(),
        &Inputs {
            managed: Some(managed),
            user: Some(user),
            ..Inputs::default()
        },
    );
    assert_eq!(
        r.settings.max_concurrent, 6,
        "the user's wins over the org default"
    );
    assert_eq!(
        r.settings.ui.theme, "nord",
        "the org default stands where the user is silent"
    );
    assert_eq!(entry(&r, "theme").tier, Tier::Org);
}

#[test]
fn a_lock_naming_a_key_it_does_not_set_locks_nothing_and_says_so() {
    let home = Home::new();
    let managed = home.write(
        "managed.toml",
        "locked = [\"guard.unmatched\", \"nope.key\"]\n",
    );
    let r = resolve(
        &Registry::core(),
        &Inputs {
            managed: Some(managed),
            ..Inputs::default()
        },
    );
    assert!(r.locked.is_empty());
    assert!(said(&r, "guard.unmatched"));
    assert!(said(&r, "nope.key"));
}

#[test]
fn a_managed_allowlist_that_cannot_be_read_is_enforced_as_empty() {
    let home = Home::new();
    let managed = home.write(
        "managed.toml",
        "locked = [\"watchdog.rungs\"]\n[watchdog]\nrungs = [\"nudge\", 7]\n",
    );
    let r = resolve(
        &Registry::core(),
        &Inputs {
            managed: Some(managed),
            ..Inputs::default()
        },
    );
    assert_eq!(
        entry(&r, "watchdog.rungs").value,
        Some(Value::Array(Vec::new()))
    );
    assert!(said(&r, "enforced as empty"), "{:#?}", r.findings);
}

#[test]
fn the_user_can_type_a_flag_that_loosens_a_lock_key_for_one_run_but_the_environment_cannot() {
    let home = Home::new();
    let user = home.write("config.toml", "[guard]\nunmatched = \"ask\"\n");
    let base = Inputs {
        user: Some(user),
        ..Inputs::default()
    };
    let flag = resolve(
        &Registry::core(),
        &Inputs {
            flags: vec![("guard.unmatched".into(), "pass".into())],
            ..base.clone()
        },
    );
    assert_eq!(flag.settings.guard.unmatched, Unmatched::Pass);
    let from_env = resolve(
        &Registry::core(),
        &Inputs {
            env: env(&[("SWARMUX_GUARD__UNMATCHED", "pass")]),
            ..base
        },
    );
    assert_eq!(
        from_env.settings.guard.unmatched,
        Unmatched::Ask,
        "a variable can only narrow a lock key"
    );
}

// ---------------------------------------------------------------------
// The project tier and trust
// ---------------------------------------------------------------------

fn with_acme() -> Registry {
    let mut reg = Registry::core();
    reg.register_section("acme", "A pack's keys.");
    reg.register(
        KeyDef::new(
            "acme.hook_url",
            Ty::Str,
            "",
            "A URL swarmux calls when a job ends.",
        )
        .grants()
        .project()
        .external(),
    );
    reg.register(
        KeyDef::new("acme.max_diff", Ty::Int, "400", "Lines a change may have.")
            .lock(Strict::Lowest)
            .restricts()
            .project()
            .external(),
    );
    reg
}

fn repo_with(text: &str) -> (Home, PathBuf) {
    let home = Home::new();
    home.write("repo/.swarmux/config.toml", text);
    let root = home.dir.path().join("repo");
    (home, root)
}

#[test]
fn a_repository_that_is_not_trusted_cannot_grant_but_can_restrict() {
    let (_home, root) =
        repo_with("[acme]\nhook_url = \"https://evil.example/x\"\nmax_diff = 100\n");
    let r = resolve(
        &with_acme(),
        &Inputs {
            project: Some(Project {
                root,
                trusted: false,
            }),
            ..Inputs::default()
        },
    );
    let url = entry(&r, "acme.hook_url");
    assert!(url.is_default, "the repository's grant does not count");
    assert_eq!(url.ignored.len(), 1);
    assert!(url.ignored[0].why.contains("not trusted"));
    assert!(
        r.findings
            .iter()
            .any(|f| f.kind == Kind::NeedsTrust && f.level == Level::Info)
    );
    let diff = entry(&r, "acme.max_diff");
    assert_eq!(
        diff.value,
        Some(Value::from(100)),
        "a restriction counts at once"
    );
    assert_eq!(diff.tier, Tier::Project);
}

#[test]
fn a_trusted_repository_grants() {
    let (_home, root) = repo_with("[acme]\nhook_url = \"https://ci.example/x\"\n");
    let r = resolve(
        &with_acme(),
        &Inputs {
            project: Some(Project {
                root,
                trusted: true,
            }),
            ..Inputs::default()
        },
    );
    let url = entry(&r, "acme.hook_url");
    assert_eq!(url.value, Some(Value::from("https://ci.example/x")));
    assert_eq!(url.tier, Tier::Project);
}

#[test]
fn a_key_the_project_tier_may_not_set_is_ignored_with_a_reason() {
    // `guard.allow` grants and no repository file may carry it.
    let (_home, root) =
        repo_with("[guard]\nallow = [\"Bash(curl *)\"]\nprotected_paths = [\"vendor/**\"]\n");
    let r = resolve(
        &Registry::core(),
        &Inputs {
            project: Some(Project {
                root,
                trusted: true,
            }),
            ..Inputs::default()
        },
    );
    assert!(r.settings.guard.allow.is_empty());
    assert!(r.findings.iter().any(|f| f.kind == Kind::NotAllowed));
    assert_eq!(r.settings.guard.protected_paths, ["vendor/**"]);
}

#[test]
fn a_repository_can_only_make_a_lock_key_stricter() {
    let (_home, root) = repo_with("[acme]\nmax_diff = 900\n");
    let home = Home::new();
    let user = home.write("config.toml", "[acme]\nmax_diff = 300\n");
    let r = resolve(
        &with_acme(),
        &Inputs {
            user: Some(user),
            project: Some(Project {
                root,
                trusted: true,
            }),
            ..Inputs::default()
        },
    );
    assert_eq!(
        entry(&r, "acme.max_diff").value,
        Some(Value::from(300)),
        "the repository's looser number does not win"
    );
}

#[test]
fn a_pack_cannot_set_a_granting_key() {
    let r = resolve(
        &with_acme(),
        &Inputs {
            packs: vec![(
                "acme/pack".into(),
                "[acme]\nhook_url = \"https://x\"\n".into(),
            )],
            ..Inputs::default()
        },
    );
    assert!(entry(&r, "acme.hook_url").is_default);
    assert!(r.findings.iter().any(|f| f.kind == Kind::NotAllowed));
}

#[test]
fn a_mission_can_narrow_but_the_layers_above_the_owner_cannot_widen() {
    let r = resolve(
        &Registry::core(),
        &Inputs {
            mission: Some((
                "mission".into(),
                "[guard]\nallow = [\"Bash(*)\"]\ndeny = [\"Bash(sudo *)\"]\n".into(),
            )),
            ..Inputs::default()
        },
    );
    assert!(r.settings.guard.allow.is_empty(), "a mission cannot grant");
    assert_eq!(r.settings.guard.deny.len(), 1, "but it can deny");
}

// ---------------------------------------------------------------------
// The environment
// ---------------------------------------------------------------------

#[test]
fn environment_names_map_to_keys() {
    assert_eq!(
        env_key("SWARMUX_GUARD__ON_UNREACHABLE").as_deref(),
        Some("guard.on_unreachable")
    );
    assert_eq!(env_key("SWARMUX__THEME").as_deref(), Some("theme"));
    assert_eq!(env_key("SWARMUX_CONFIG_DIR"), None, "not a setting");
    assert_eq!(env_key("SWARMUX_CLAUDE_BIN"), None);
    assert_eq!(env_key("PATH"), None);
}

#[test]
fn environment_variables_are_typed_by_the_key() {
    let r = resolve(
        &Registry::core(),
        &Inputs {
            env: env(&[
                ("SWARMUX__MAX_CONCURRENT", "8"),
                ("SWARMUX__LOW_POWER", "yes"),
                ("SWARMUX_GUARD__DENY", "Bash(sudo *), Write(/etc/**)"),
                ("SWARMUX_GOVERNOR__HOLD_AT", "0.9"),
            ]),
            ..Inputs::default()
        },
    );
    assert_eq!(r.settings.max_concurrent, 8);
    assert!(r.settings.ui.low_power);
    assert_eq!(r.settings.guard.deny.len(), 2);
    assert!((r.settings.governor.hold_at - 0.9).abs() < 1e-9);
    assert!(r.findings.is_empty(), "{:#?}", r.findings);
}

#[test]
fn a_granting_key_cannot_be_set_from_the_environment() {
    let r = resolve(
        &Registry::core(),
        &Inputs {
            env: env(&[
                ("SWARMUX_GUARD__ALLOW", "Bash(*)"),
                ("SWARMUX_NOTIFY__NTFY", "https://ntfy.sh/x"),
            ]),
            ..Inputs::default()
        },
    );
    assert!(r.settings.guard.allow.is_empty());
    assert!(r.settings.notify.ntfy.is_none());
    assert_eq!(r.findings.iter().filter(|f| f.kind == Kind::Env).count(), 2);
}

#[test]
fn a_bad_environment_value_and_an_unknown_name_are_said_not_fatal() {
    let r = resolve(
        &Registry::core(),
        &Inputs {
            env: env(&[
                ("SWARMUX__MAX_CONCURRENT", "lots"),
                ("SWARMUX_NOPE__X", "1"),
            ]),
            ..Inputs::default()
        },
    );
    assert_eq!(
        r.settings.max_concurrent,
        Settings::default().max_concurrent
    );
    assert!(said(&r, "SWARMUX__MAX_CONCURRENT"));
    assert!(said(&r, "SWARMUX_NOPE__X"));
}

// ---------------------------------------------------------------------
// Hash and extension
// ---------------------------------------------------------------------

#[test]
fn the_effective_config_has_a_stable_hash_that_changes_with_a_value() {
    let a = user_only("max_concurrent = 4\n");
    let b = user_only("max_concurrent = 4\n");
    let c = user_only("max_concurrent = 5\n");
    assert_eq!(a.sha256, b.sha256);
    assert_ne!(a.sha256, c.sha256);
    assert_eq!(a.sha256.len(), 64);
}

#[test]
fn a_registered_extension_key_goes_through_the_same_layers() {
    let reg = with_acme();
    let home = Home::new();
    let user = home.write("config.toml", "[acme]\nmax_diff = 250\n");
    let r = resolve(
        &reg,
        &Inputs {
            user: Some(user),
            env: env(&[("SWARMUX_ACME__MAX_DIFF", "100")]),
            ..Inputs::default()
        },
    );
    // Not a key the settings read: it is in the merged table, with its
    // provenance, and the build does not choke on it.
    let e = entry(&r, "acme.max_diff");
    assert_eq!(e.merge, MergeKind::Lock);
    assert_eq!(e.effect, Effect::Restricts);
    assert!(TierSet::of(&[Tier::User]).contains(Tier::User));
    assert_eq!(
        e.value,
        Some(Value::from(100)),
        "the stricter of user and env"
    );
}

#[test]
fn a_repository_fences_its_own_paths_through_the_gate_before_any_trust() {
    use swarmux_application::GateSource;
    let (_home, root) = repo_with(
        "[guard]\nprotected_paths = [\"vendor/**\", \"../outside/**\"]\nallow = [\"Bash(curl *)\"]\n",
    );
    let found = project_restrictions(&root);
    assert_eq!(found.protected, ["vendor/**"]);
    assert!(
        found.warnings.iter().any(|w| w.contains("..")),
        "{:?}",
        found.warnings
    );
    // The gate carries them as protected globs, with nobody's trust.
    let gate = crate::FsGateSource::new().load(&root);
    assert!(gate.spec.protected.contains(&"vendor/**".to_string()));
    assert!(gate.spec.allow_test_changes.is_empty());
}
