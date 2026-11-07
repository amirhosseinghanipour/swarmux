//! Landing a workspace mission: once its gate passed, each repository's
//! branch is merged into the mission's branch in that repository, in the
//! order the mission names them (producers before consumers). It lands
//! whole or not at all: when one repository can't take its merge, the
//! ones merged before it are put back.

use crate::Millis;
use crate::json::Json;
use crate::ledger::Payload;

/// Where one repository's part stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LandState {
    /// Not tried yet.
    #[default]
    Waiting,
    /// Merged into the mission's branch.
    Landed,
    /// It could not be merged; see the note.
    Failed,
    /// It was merged, then put back because a later repository failed.
    Undone,
    /// Not tried: an earlier repository failed.
    Skipped,
}

impl LandState {
    pub fn as_str(self) -> &'static str {
        match self {
            LandState::Waiting => "waiting",
            LandState::Landed => "landed",
            LandState::Failed => "failed",
            LandState::Undone => "undone",
            LandState::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Option<LandState> {
        [
            LandState::Waiting,
            LandState::Landed,
            LandState::Failed,
            LandState::Undone,
            LandState::Skipped,
        ]
        .into_iter()
        .find(|l| l.as_str() == s)
    }
}

/// One repository of a landing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepoLanding {
    /// The repository's name in its workspace.
    pub repo: String,
    /// The job's branch in that repository.
    pub branch: String,
    pub state: LandState,
    /// The merge commit, once landed.
    pub commit: Option<String>,
    /// Why it failed or was put back.
    pub note: String,
}

/// A workspace mission's landing, repository by repository, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Landing {
    /// The mission's branch in every repository.
    pub into: String,
    pub repos: Vec<RepoLanding>,
    pub at: Millis,
}

impl Landing {
    /// A landing of `repos` (name and job branch), in that order.
    pub fn new(into: &str, repos: &[(String, String)], at: Millis) -> Landing {
        Landing {
            into: into.to_string(),
            repos: repos
                .iter()
                .map(|(repo, branch)| RepoLanding {
                    repo: repo.clone(),
                    branch: branch.clone(),
                    ..RepoLanding::default()
                })
                .collect(),
            at,
        }
    }

    /// Every repository landed.
    pub fn complete(&self) -> bool {
        !self.repos.is_empty() && self.repos.iter().all(|r| r.state == LandState::Landed)
    }

    /// The repository that stopped it, if one did.
    pub fn failed(&self) -> Option<&RepoLanding> {
        self.repos.iter().find(|r| r.state == LandState::Failed)
    }

    /// Marks `repo` failed: the ones landed before it are to be put back
    /// (the caller undoes them and calls [`Landing::undone`]) and the rest
    /// are skipped.
    pub fn fail(&mut self, repo: &str, note: &str) {
        let mut after = false;
        for r in &mut self.repos {
            if after {
                r.state = LandState::Skipped;
            } else if r.repo == repo {
                r.state = LandState::Failed;
                r.note = note.to_string();
                after = true;
            }
        }
    }

    /// `repo`, landed earlier, was put back.
    pub fn undone(&mut self, repo: &str) {
        if let Some(r) = self.repos.iter_mut().find(|r| r.repo == repo) {
            r.state = LandState::Undone;
            r.commit = None;
            r.note = "put back: a later repository failed".into();
        }
    }

    /// One line for the card and the inbox.
    pub fn summary(&self) -> String {
        if let Some(f) = self.failed() {
            let undone = self
                .repos
                .iter()
                .filter(|r| r.state == LandState::Undone)
                .count();
            let back = if undone > 0 {
                format!("; {undone} landed before it put back, nothing landed")
            } else {
                "; nothing landed".to_string()
            };
            return format!("{} could not land: {}{back}", f.repo, f.note);
        }
        let parts: Vec<String> = self
            .repos
            .iter()
            .map(|r| format!("{} {}", r.repo, r.state.as_str()))
            .collect();
        parts.join(" · ")
    }

    pub fn to_json(&self) -> String {
        Payload::new()
            .str("into", &self.into)
            .int("at", self.at)
            .objects(
                "repos",
                self.repos
                    .iter()
                    .map(|r| {
                        Payload::new()
                            .str("repo", &r.repo)
                            .str("branch", &r.branch)
                            .str("state", r.state.as_str())
                            .opt_str("commit", r.commit.as_deref())
                            .str("note", &r.note)
                    })
                    .collect(),
            )
            .build()
    }

    pub fn from_json(text: &str) -> Option<Landing> {
        let v = Json::parse(text).ok()?;
        let s = |j: &Json, k: &str| {
            j.get(k)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Some(Landing {
            into: s(&v, "into"),
            at: v
                .get("at")
                .and_then(Json::as_f64)
                .map_or(0, |n| n as Millis),
            repos: v
                .get("repos")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .map(|r| RepoLanding {
                    repo: s(r, "repo"),
                    branch: s(r, "branch"),
                    state: LandState::parse(&s(r, "state")).unwrap_or_default(),
                    commit: r.get("commit").and_then(Json::as_str).map(str::to_string),
                    note: s(r, "note"),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing() -> Landing {
        Landing::new(
            "swarmux/m/integration",
            &[
                ("backend".into(), "swarmux/s/1".into()),
                ("app".into(), "swarmux/s/1".into()),
                ("admin".into(), "swarmux/s/1".into()),
            ],
            5,
        )
    }

    #[test]
    fn a_failure_skips_the_rest_and_puts_back_the_landed() {
        let mut l = landing();
        l.repos[0].state = LandState::Landed;
        l.repos[0].commit = Some("abc".into());
        l.fail("app", "conflict in lib/api.dart");
        l.undone("backend");
        let states: Vec<LandState> = l.repos.iter().map(|r| r.state).collect();
        assert_eq!(
            states,
            [LandState::Undone, LandState::Failed, LandState::Skipped]
        );
        assert!(!l.complete());
        assert_eq!(l.failed().unwrap().repo, "app");
        assert_eq!(
            l.summary(),
            "app could not land: conflict in lib/api.dart; 1 landed before it put back, nothing landed"
        );
    }

    #[test]
    fn complete_when_all_landed_and_round_trips() {
        let mut l = landing();
        assert!(!l.complete());
        for r in &mut l.repos {
            r.state = LandState::Landed;
            r.commit = Some("c".into());
        }
        assert!(l.complete());
        assert_eq!(l.summary(), "backend landed · app landed · admin landed");
        assert_eq!(Landing::from_json(&l.to_json()), Some(l));
        assert!(!Landing::default().complete());
    }
}
