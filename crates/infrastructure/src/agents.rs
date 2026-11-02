//! `AgentCatalog` adapter: reads subagent definitions from
//! `.claude/agents/*.md` in the session folder, then the user's own.

use std::path::{Path, PathBuf};

use swarmux_application::ports::{AgentCatalog, AgentProfile};

/// Definitions bigger than this are not read.
const FILE_MAX: u64 = 256 * 1024;

pub struct FsAgentCatalog {
    /// The user's home folder (for `~/.claude/agents`), when known.
    home: Option<PathBuf>,
}

impl FsAgentCatalog {
    pub fn new(home: Option<PathBuf>) -> FsAgentCatalog {
        FsAgentCatalog { home }
    }

    fn dirs(&self, cwd: &Path) -> Vec<PathBuf> {
        let mut dirs = vec![cwd.join(".claude").join("agents")];
        dirs.extend(self.home.as_ref().map(|h| h.join(".claude").join("agents")));
        dirs
    }
}

impl AgentCatalog for FsAgentCatalog {
    fn profile(&self, cwd: &Path, agent_type: &str) -> AgentProfile {
        let name = agent_type.trim();
        if name.is_empty() || name.contains(['/', '\\', '.']) {
            return AgentProfile::default();
        }
        for dir in self.dirs(cwd) {
            if let Some(front) = read_front(&dir.join(format!("{name}.md")))
                && field(&front, "name").is_none_or(|n| n == name)
            {
                return profile_of(&front);
            }
            // The file name may differ from the agent's `name:`.
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md")
                    && let Some(front) = read_front(&path)
                    && field(&front, "name").as_deref() == Some(name)
                {
                    return profile_of(&front);
                }
            }
        }
        AgentProfile::default()
    }
}

fn profile_of(front: &str) -> AgentProfile {
    AgentProfile {
        model: field(front, "model").filter(|m| !m.is_empty()),
        description: field(front, "description").filter(|d| !d.is_empty()),
    }
}

/// The YAML front matter of a Markdown file (between the first two `---`).
fn read_front(path: &Path) -> Option<String> {
    if std::fs::metadata(path).ok()?.len() > FILE_MAX {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None;
    }
    let front: Vec<&str> = lines.take_while(|l| l.trim() != "---").collect();
    Some(front.join("\n"))
}

/// A top-level `key: value` of the front matter, unquoted.
fn field(front: &str, key: &str) -> Option<String> {
    front.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if l.starts_with(char::is_whitespace) || k.trim() != key {
            return None;
        }
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(v);
        Some(v.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, file: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
    }

    #[test]
    fn project_agents_win_over_user_agents() {
        let project = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let p = project.path().join(".claude/agents");
        let h = home.path().join(".claude/agents");
        write(
            &p,
            "worker.md",
            "---\nname: worker\nmodel: sonnet\ndescription: \"Runs tests\"\n---\nbody\n",
        );
        write(&h, "worker.md", "---\nname: worker\nmodel: opus\n---\n");
        write(
            &h,
            "scout-agent.md",
            "\u{feff}---\nname: scout\nmodel: 'haiku'\n  model: nested\n---\n",
        );
        let catalog = FsAgentCatalog::new(Some(home.path().into()));
        assert_eq!(
            catalog.profile(project.path(), "worker"),
            AgentProfile {
                model: Some("sonnet".into()),
                description: Some("Runs tests".into())
            }
        );
        assert_eq!(
            catalog.profile(project.path(), "scout").model.as_deref(),
            Some("haiku")
        );
    }

    #[test]
    fn unknown_or_odd_types_give_nothing() {
        let project = tempfile::tempdir().unwrap();
        let p = project.path().join(".claude/agents");
        write(&p, "plain.md", "no front matter\nmodel: opus\n");
        let catalog = FsAgentCatalog::new(None);
        for kind in ["plain", "general-purpose", "../x", "", "a.b"] {
            assert_eq!(
                catalog.profile(project.path(), kind),
                AgentProfile::default(),
                "{kind}"
            );
        }
    }
}
