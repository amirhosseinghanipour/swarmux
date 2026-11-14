//! The `[runtime]` section of `config.toml`: launch profiles, canaries,
//! trusted folders and the MCP servers a controlled run may use.
//!
//! Read key by key: a value that is wrong is replaced by its default and
//! said so in `Settings::warnings`, never a reason to refuse the file. A key
//! that is not known is reported, not silently ignored.

use std::path::PathBuf;

use swarmux_application::{
    CanaryMode, HookFailure, McpServerDef, OnError, RuntimeSettings, Settings, StatuslineMode,
};
use swarmux_domain::runtime::ProfileSetting;
use toml::Value;

/// The keys `[runtime]` knows.
const KEYS: [&str; 12] = [
    "profile",
    "controlled_sources",
    "reviewer_sources",
    "canary",
    "canary_cache_minutes",
    "canary_timeout_secs",
    "on_error",
    "hook_failure",
    "statusline",
    "repos",
    "mcp",
    "schema_version",
];

const SOURCES: [&str; 3] = ["user", "project", "local"];

/// Applies the `[runtime]` table `raw` to `settings`.
pub(crate) fn apply(raw: Value, text: &str, settings: &mut Settings) {
    let mut warnings: Vec<String> = Vec::new();
    let mut warn = |key: &str, message: String| {
        let line =
            super::config::key_line(text, key).map_or(String::new(), |l| format!(" (line {l})"));
        warnings.push(format!("config.toml{line}: {message}"));
    };
    let Value::Table(table) = raw else {
        warn(
            "runtime",
            "[runtime] is not a table; using the defaults".into(),
        );
        settings.warnings.extend(warnings);
        return;
    };
    let mut rt = RuntimeSettings::default();
    for key in table.keys() {
        if !KEYS.contains(&key.as_str()) {
            warn(
                key,
                format!(
                    "runtime.{key} is not a known key; it does nothing (known: {})",
                    KEYS.join(", ")
                ),
            );
        }
    }
    let word = |key: &str| table.get(key);
    if let Some(v) = word("profile") {
        match v.as_str().and_then(ProfileSetting::parse) {
            Some(p) => rt.profile = p,
            None => warn(
                "profile",
                format!(
                    "runtime.profile {v} is not \"auto\", \"inherit\" or \"controlled\"; using \"auto\""
                ),
            ),
        }
    }
    for (key, target) in [
        ("controlled_sources", &mut rt.controlled_sources),
        ("reviewer_sources", &mut rt.reviewer_sources),
    ] {
        if let Some(v) = word(key) {
            match sources(v) {
                Some(list) => *target = list,
                None => warn(
                    key,
                    format!(
                        "runtime.{key} {v} is not a list of \"user\", \"project\" and \"local\"; using {:?}",
                        target
                    ),
                ),
            }
        }
    }
    if let Some(v) = word("canary") {
        match v.as_str().and_then(CanaryMode::parse) {
            Some(m) => rt.canary = m,
            None => warn(
                "canary",
                format!(
                    "runtime.canary {v} is not \"each\", \"cached\" or \"off\"; using \"each\""
                ),
            ),
        }
    }
    for (key, range, target) in [
        (
            "canary_cache_minutes",
            1..=1440,
            &mut rt.canary_cache_minutes,
        ),
        ("canary_timeout_secs", 10..=600, &mut rt.canary_timeout_secs),
    ] {
        if let Some(v) = word(key) {
            match v.as_integer().filter(|n| range.contains(n)) {
                Some(n) => *target = n as u64,
                None => warn(
                    key,
                    format!(
                        "runtime.{key} {v} is not a whole number from {} to {}; using {target}",
                        range.start(),
                        range.end()
                    ),
                ),
            }
        }
    }
    if let Some(v) = word("on_error") {
        match v.as_str().and_then(OnError::parse) {
            Some(m) => rt.on_error = m,
            None => warn(
                "on_error",
                format!("runtime.on_error {v} is not \"refuse\" or \"warn\"; using \"refuse\""),
            ),
        }
    }
    let mut hook_failure_given = false;
    if let Some(v) = word("hook_failure") {
        match v.as_str().and_then(HookFailure::parse) {
            Some(m) => {
                rt.hook_failure = m;
                hook_failure_given = true;
            }
            None => warn(
                "hook_failure",
                format!(
                    "runtime.hook_failure {v} is not \"auto\", \"block\" or \"off\"; using \"auto\""
                ),
            ),
        }
    }
    if let Some(v) = word("statusline") {
        match v.as_str().and_then(StatuslineMode::parse) {
            Some(m) => rt.statusline = m,
            None => warn(
                "statusline",
                format!(
                    "runtime.statusline {v} is not \"auto\", \"on\" or \"off\"; using \"auto\""
                ),
            ),
        }
    }
    if let Some(v) = word("repos") {
        match v.as_table() {
            Some(repos) => {
                for (path, entry) in repos {
                    match entry.get("trusted").and_then(Value::as_bool) {
                        Some(true) => rt.trusted_repos.push(PathBuf::from(path)),
                        Some(false) => {}
                        None => warn(
                            "trusted",
                            format!(
                                "runtime.repos.\"{path}\" needs trusted = true or false; the folder is not trusted"
                            ),
                        ),
                    }
                }
            }
            None => warn(
                "repos",
                "runtime.repos is not a table of folders; no folder is trusted".into(),
            ),
        }
    }
    if let Some(v) = word("mcp") {
        match v.as_table() {
            Some(servers) => {
                for (name, def) in servers {
                    match server(name, def) {
                        Ok(def) => rt.mcp.push(def),
                        Err(why) => warn(
                            "mcp",
                            format!("runtime.mcp.{name} {why}; the server is not passed"),
                        ),
                    }
                }
            }
            None => warn(
                "mcp",
                "runtime.mcp is not a table of servers; none is passed".into(),
            ),
        }
    }
    // The old switch of the same thing keeps working.
    if !hook_failure_given && settings.guard.hook_on_failure_block {
        rt.hook_failure = HookFailure::Block;
    }
    settings.runtime = rt;
    settings.warnings.extend(warnings);
}

/// The sources in `v`, or `None` when it is not a list of known ones.
fn sources(v: &Value) -> Option<Vec<String>> {
    let list: Option<Vec<String>> = v
        .as_array()?
        .iter()
        .map(|x| {
            x.as_str()
                .map(str::trim)
                .filter(|s| SOURCES.contains(s))
                .map(str::to_string)
        })
        .collect();
    let mut list = list?;
    list.dedup();
    Some(list)
}

fn server(name: &str, def: &Value) -> Result<McpServerDef, String> {
    let table = def.as_table().ok_or("is not a table")?;
    if name.trim().is_empty() {
        return Err("has no name".into());
    }
    let text = |k: &str| table.get(k).and_then(Value::as_str).map(str::to_string);
    let command = text("command").unwrap_or_default();
    let url = text("url");
    if command.is_empty() == url.is_none() {
        return Err("needs either command or url, not both".into());
    }
    let args = match table.get("args") {
        None => Vec::new(),
        Some(a) => a
            .as_array()
            .and_then(|a| {
                a.iter()
                    .map(|x| x.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or("has args that are not a list of strings")?,
    };
    let env = match table.get("env") {
        None => Vec::new(),
        Some(e) => e
            .as_table()
            .and_then(|e| {
                e.iter()
                    .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect::<Option<Vec<_>>>()
            })
            .ok_or("has env that is not a table of strings")?,
    };
    let transport = match (&url, text("transport").as_deref()) {
        (None, _) => "stdio".to_string(),
        (Some(_), None | Some("http")) => "http".to_string(),
        (Some(_), Some("sse")) => "sse".to_string(),
        (Some(_), Some(_)) => return Err("has a transport that is not \"http\" or \"sse\"".into()),
    };
    Ok(McpServerDef {
        name: name.to_string(),
        command,
        args,
        env,
        url,
        transport,
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use swarmux_application::{CanaryMode, HookFailure, OnError, Settings, StatuslineMode};
    use swarmux_domain::runtime::ProfileSetting;

    use crate::config::parse_settings;

    fn parse(text: &str) -> Settings {
        parse_settings(text, Path::new("config.toml")).unwrap()
    }

    #[test]
    fn defaults_without_the_section_fail_closed() {
        let s = parse("");
        let r = &s.runtime;
        assert_eq!(r.profile, ProfileSetting::Auto);
        assert_eq!(r.canary, CanaryMode::Each);
        assert_eq!(r.on_error, OnError::Refuse);
        assert_eq!(r.hook_failure, HookFailure::Auto);
        assert_eq!(r.statusline, StatuslineMode::Auto);
        assert_eq!(r.controlled_sources, ["user"]);
        assert!(r.reviewer_sources.is_empty() && r.trusted_repos.is_empty() && r.mcp.is_empty());
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn good_values_are_read() {
        let s = parse(
            r#"
[runtime]
profile = "controlled"
controlled_sources = ["user", "local"]
reviewer_sources = ["user"]
canary = "cached"
canary_cache_minutes = 15
canary_timeout_secs = 45
on_error = "warn"
hook_failure = "off"
statusline = "on"

[runtime.repos."/work/app"]
trusted = true

[runtime.repos."/work/other"]
trusted = false

[runtime.mcp.docs]
command = "docs-server"
args = ["--stdio"]
env = { TOKEN = "t" }

[runtime.mcp.wiki]
url = "https://wiki.example/mcp"
"#,
        );
        let r = &s.runtime;
        assert_eq!(r.profile, ProfileSetting::Controlled);
        assert_eq!(r.controlled_sources, ["user", "local"]);
        assert_eq!(r.reviewer_sources, ["user"]);
        assert_eq!(r.canary, CanaryMode::Cached);
        assert_eq!((r.canary_cache_minutes, r.canary_timeout_secs), (15, 45));
        assert_eq!(r.on_error, OnError::Warn);
        assert_eq!(r.hook_failure, HookFailure::Off);
        assert_eq!(r.statusline, StatuslineMode::On);
        assert_eq!(r.trusted_repos, [std::path::PathBuf::from("/work/app")]);
        assert_eq!(r.mcp.len(), 2);
        assert_eq!(r.mcp[0].name, "docs");
        assert_eq!(r.mcp[0].args, ["--stdio"]);
        assert_eq!(r.mcp[0].env, [("TOKEN".to_string(), "t".to_string())]);
        assert_eq!(r.mcp[1].url.as_deref(), Some("https://wiki.example/mcp"));
        assert_eq!(r.mcp[1].transport, "http");
        assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    }

    #[test]
    fn bad_lines_fall_back_to_defaults_with_a_warning_each() {
        let s = parse(
            r#"
[runtime]
profile = "paranoid"
controlled_sources = ["user", "everything"]
canary = 1
canary_cache_minutes = 0
canary_timeout_secs = 5
on_error = "shrug"
hook_failure = true
statusline = "sometimes"
colour = "red"
"#,
        );
        let r = &s.runtime;
        assert_eq!(r.profile, ProfileSetting::Auto);
        assert_eq!(r.controlled_sources, ["user"]);
        assert_eq!(r.canary, CanaryMode::Each);
        assert_eq!((r.canary_cache_minutes, r.canary_timeout_secs), (60, 90));
        assert_eq!(r.on_error, OnError::Refuse);
        assert_eq!(s.warnings.len(), 9, "{:#?}", s.warnings);
        assert!(
            s.warnings
                .iter()
                .any(|w| w.contains("runtime.colour is not a known key"))
        );
        assert!(s.warnings.iter().all(|w| w.starts_with("config.toml")));
        assert!(
            s.warnings.iter().any(|w| w.contains("(line 3)")),
            "{:#?}",
            s.warnings
        );
    }

    #[test]
    fn a_server_that_cannot_be_passed_is_left_out_and_said() {
        let s = parse(
            r#"
[runtime.mcp.both]
command = "x"
url = "https://x"

[runtime.mcp.neither]
args = []

[runtime.mcp.odd]
url = "https://x"
transport = "carrier-pigeon"

[runtime.mcp.fine]
command = "x"
"#,
        );
        assert_eq!(s.runtime.mcp.len(), 1);
        assert_eq!(s.runtime.mcp[0].name, "fine");
        assert_eq!(s.warnings.len(), 3, "{:#?}", s.warnings);
    }

    #[test]
    fn the_old_guard_switch_still_turns_on_failure_on() {
        let s = parse("[guard]\nhook_on_failure_block = true\n");
        assert_eq!(s.runtime.hook_failure, HookFailure::Block);
        // The new key wins when both are given.
        let s = parse("[guard]\nhook_on_failure_block = true\n[runtime]\nhook_failure = \"off\"\n");
        assert_eq!(s.runtime.hook_failure, HookFailure::Off);
    }
}
