//! Pull requests and CI of a repository through the `gh` CLI.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use swarmux_application::{CiRun, GithubStatus, PullRequest};

use super::cli::{self, CliError};

const LIMIT: Duration = Duration::from_secs(20);
const PR_LIMIT: &str = "20";

/// Why GitHub data is missing, with what the user can do about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GithubError {
    pub message: String,
    pub hint: Option<String>,
}

/// Open pull requests of the repository at `cwd` and the latest run on `branch`.
pub fn status(gh: &Path, cwd: &Path, branch: Option<&str>) -> Result<GithubStatus, GithubError> {
    let run = |args: &[&str]| cli::run(gh, args, Some(cwd), LIMIT).map_err(explain);
    let repo: RepoJson = parse(&run(&["repo", "view", "--json", "nameWithOwner"])?)?;
    let prs: Vec<PrJson> = parse(&run(&[
        "pr",
        "list",
        "--state",
        "open",
        "--limit",
        PR_LIMIT,
        "--json",
        "number,title,state,reviewDecision,isDraft,headRefName",
    ])?)?;
    let ci = match branch {
        Some(b) => {
            let runs: Vec<RunJson> = parse(&run(&[
                "run",
                "list",
                "--branch",
                b,
                "--limit",
                "1",
                "--json",
                "status,conclusion,workflowName,headBranch,url",
            ])?)?;
            runs.into_iter().next().map(RunJson::into_run)
        }
        None => None,
    };
    Ok(GithubStatus {
        repo: Some(repo.name_with_owner),
        pull_requests: prs.into_iter().map(PrJson::into_pr).collect(),
        ci,
    })
}

/// A CLI failure as a message plus a hint.
pub fn explain(e: CliError) -> GithubError {
    let message = e.to_string();
    let lower = message.to_ascii_lowercase();
    let hint = match &e {
        CliError::NotFound => {
            Some("install the GitHub CLI (gh) to see pull requests and CI".into())
        }
        _ if lower.contains("auth login") || lower.contains("not logged") => {
            Some("run `gh auth login`".into())
        }
        _ if lower.contains("not a git repository") || lower.contains("no git remote") => {
            Some("the folder is not a GitHub repository".into())
        }
        _ => None,
    };
    GithubError { message, hint }
}

fn parse<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, GithubError> {
    serde_json::from_str(text).map_err(|e| GithubError {
        message: format!("unexpected gh output: {e}"),
        hint: None,
    })
}

#[derive(Deserialize)]
struct RepoJson {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize)]
struct PrJson {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    #[serde(rename = "reviewDecision", default)]
    review_decision: Option<String>,
    #[serde(rename = "isDraft", default)]
    is_draft: bool,
    #[serde(rename = "headRefName", default)]
    head_ref_name: String,
}

impl PrJson {
    fn into_pr(self) -> PullRequest {
        PullRequest {
            number: self.number,
            title: self.title,
            state: self.state,
            review_decision: self.review_decision.filter(|d| !d.is_empty()),
            draft: self.is_draft,
            branch: self.head_ref_name,
        }
    }
}

#[derive(Deserialize)]
struct RunJson {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(rename = "workflowName", default)]
    workflow_name: String,
    #[serde(rename = "headBranch", default)]
    head_branch: String,
    #[serde(default)]
    url: Option<String>,
}

impl RunJson {
    fn into_run(self) -> CiRun {
        CiRun {
            workflow: self.workflow_name,
            branch: self.head_branch,
            status: self.status,
            conclusion: self.conclusion.filter(|c| !c.is_empty()),
            url: self.url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_requests_parse() {
        let text = r#"[{"headRefName":"feat/x","isDraft":false,"number":12,"reviewDecision":"APPROVED","state":"OPEN","title":"Add x"},
                       {"headRefName":"wip","isDraft":true,"number":13,"reviewDecision":"","state":"OPEN","title":"WIP"}]"#;
        let prs: Vec<PrJson> = parse(text).unwrap();
        let prs: Vec<PullRequest> = prs.into_iter().map(PrJson::into_pr).collect();
        assert_eq!(prs[0].review_decision.as_deref(), Some("APPROVED"));
        assert_eq!((prs[1].draft, prs[1].review_decision.clone()), (true, None));
        assert_eq!(prs[0].branch, "feat/x");
    }

    #[test]
    fn runs_parse() {
        let text = r#"[{"conclusion":"","headBranch":"main","status":"in_progress","url":"https://github.com/o/r/actions/runs/1","workflowName":"CI"}]"#;
        let runs: Vec<RunJson> = parse(text).unwrap();
        let run = runs.into_iter().next().unwrap().into_run();
        assert_eq!((run.status.as_str(), run.conclusion), ("in_progress", None));
        assert_eq!(run.workflow, "CI");
        assert!(parse::<Vec<RunJson>>("not json").is_err());
    }

    #[test]
    fn errors_come_with_hints() {
        assert!(
            explain(CliError::NotFound)
                .hint
                .unwrap()
                .contains("install")
        );
        let e = explain(CliError::Failed(
            "To get started with GitHub CLI, please run:  gh auth login".into(),
        ));
        assert_eq!(e.hint.as_deref(), Some("run `gh auth login`"));
        let e = explain(CliError::Failed("fatal: not a git repository".into()));
        assert!(e.hint.unwrap().contains("not a GitHub repository"));
        assert_eq!(explain(CliError::TimedOut).hint, None);
    }

    #[test]
    fn missing_gh_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let e = status(&dir.path().join("no-gh"), dir.path(), None).unwrap_err();
        assert!(e.hint.is_some());
    }
}
