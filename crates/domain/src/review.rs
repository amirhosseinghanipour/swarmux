//! The review step: a second, read-only model run that audits a finished job.

use crate::json::{Json, quote};
use crate::{DomainError, JobId};

/// Longest answer text searched for the verdict.
const ANSWER_MAX: usize = 32 * 1024;
const CONCERNS_MAX: usize = 20;
const CONCERN_CHARS: usize = 300;
/// Parts of the job shown to the reviewer, in bytes.
const PROMPT_CAP: usize = 8 * 1024;
const RESULT_CAP: usize = 8 * 1024;
const DIFF_CAP: usize = 48 * 1024;

/// System prompt of every review run.
pub const REVIEW_SYSTEM: &str = "You are a strict, silent code reviewer. You may read files but never \
change anything. Answer with one JSON object and nothing else.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewVerdict {
    Pass,
    Concerns,
}

impl ReviewVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            ReviewVerdict::Pass => "pass",
            ReviewVerdict::Concerns => "concerns",
        }
    }
}

/// What the reviewer answered.
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub verdict: ReviewVerdict,
    pub concerns: Vec<String>,
    pub diffs_checked: u32,
}

/// Where a job's review stands.
#[derive(Clone, Debug, PartialEq)]
pub enum ReviewState {
    /// Asked for; starts once the job finishes successfully.
    Requested,
    /// The review job is queued or running.
    Running { job: JobId },
    Done {
        review: Review,
        job: JobId,
        cost_usd: f64,
    },
    /// No usable verdict. Never affects the reviewed job's own result.
    Failed {
        error: String,
        job: Option<JobId>,
        cost_usd: f64,
    },
    /// Not worth a reviewer: the change's risk was routine. Says why.
    Skipped { why: String },
}

impl ReviewState {
    /// The review job, once there is one.
    pub fn job(&self) -> Option<JobId> {
        match self {
            ReviewState::Requested | ReviewState::Skipped { .. } => None,
            ReviewState::Running { job } | ReviewState::Done { job, .. } => Some(*job),
            ReviewState::Failed { job, .. } => *job,
        }
    }

    pub fn cost_usd(&self) -> Option<f64> {
        match self {
            ReviewState::Done { cost_usd, .. } | ReviewState::Failed { cost_usd, .. } => {
                Some(*cost_usd)
            }
            _ => None,
        }
    }

    /// One JSON object, as stored in `jobs.review`.
    pub fn encode(&self) -> String {
        let job = |j: Option<JobId>| j.map_or("null".to_string(), |j| j.0.to_string());
        match self {
            ReviewState::Requested => r#"{"state":"requested"}"#.to_string(),
            ReviewState::Running { job: j } => {
                format!(r#"{{"state":"running","job":{}}}"#, job(Some(*j)))
            }
            ReviewState::Done {
                review,
                job: j,
                cost_usd,
            } => {
                let concerns: Vec<String> = review.concerns.iter().map(|c| quote(c)).collect();
                format!(
                    r#"{{"state":"done","job":{},"cost_usd":{cost_usd},"verdict":"{}","concerns":[{}],"diffs_checked":{}}}"#,
                    job(Some(*j)),
                    review.verdict.as_str(),
                    concerns.join(","),
                    review.diffs_checked
                )
            }
            ReviewState::Failed {
                error,
                job: j,
                cost_usd,
            } => format!(
                r#"{{"state":"failed","job":{},"cost_usd":{cost_usd},"error":{}}}"#,
                job(*j),
                quote(error)
            ),
            ReviewState::Skipped { why } => {
                format!(r#"{{"state":"skipped","why":{}}}"#, quote(why))
            }
        }
    }

    /// Reads what [`ReviewState::encode`] wrote.
    pub fn decode(text: &str) -> Result<ReviewState, DomainError> {
        let bad = |why: &str| DomainError::InvalidReview(why.to_string());
        let v = Json::parse(text).map_err(|e| bad(&e))?;
        let job = v.get("job").and_then(Json::as_f64).map(|n| JobId(n as i64));
        let cost_usd = v.get("cost_usd").and_then(Json::as_f64).unwrap_or(0.0);
        match v.get("state").and_then(Json::as_str) {
            Some("requested") => Ok(ReviewState::Requested),
            Some("running") => Ok(ReviewState::Running {
                job: job.ok_or_else(|| bad("running review without a job"))?,
            }),
            Some("done") => Ok(ReviewState::Done {
                review: read_review(&v).map_err(|e| bad(&e))?,
                job: job.ok_or_else(|| bad("done review without a job"))?,
                cost_usd,
            }),
            Some("failed") => Ok(ReviewState::Failed {
                error: v
                    .get("error")
                    .and_then(Json::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                job,
                cost_usd,
            }),
            Some("skipped") => Ok(ReviewState::Skipped {
                why: v
                    .get("why")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string(),
            }),
            _ => Err(bad("unknown state")),
        }
    }
}

/// Finds the reviewer's JSON verdict in its answer: the first object with a
/// `verdict`, bare or inside a code fence or prose.
pub fn parse_review(answer: &str) -> Result<Review, String> {
    let text = match answer.char_indices().nth(ANSWER_MAX) {
        Some((i, _)) => &answer[..i],
        None => answer,
    };
    if text.trim().is_empty() {
        return Err("the reviewer gave no answer".into());
    }
    let mut saw_json = false;
    for start in text.match_indices('{').map(|(i, _)| i) {
        let Some(end) = object_end(&text[start..]) else {
            continue;
        };
        let Ok(v) = Json::parse(&text[start..start + end]) else {
            continue;
        };
        saw_json = true;
        if v.get("verdict").is_some() {
            return read_review(&v);
        }
    }
    Err(if saw_json {
        "the reviewer's JSON has no verdict".into()
    } else {
        "the reviewer's answer has no JSON".into()
    })
}

fn read_review(v: &Json) -> Result<Review, String> {
    let verdict = match v
        .get("verdict")
        .and_then(Json::as_str)
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("pass") => ReviewVerdict::Pass,
        Some("concerns") => ReviewVerdict::Concerns,
        Some(other) => return Err(format!("unknown verdict {other:?}")),
        None => return Err("verdict is not a string".into()),
    };
    let concerns = v
        .get("concerns")
        .and_then(Json::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(concern_text)
        .filter(|c| !c.is_empty())
        .take(CONCERNS_MAX)
        .collect();
    let diffs_checked = v
        .get("diffs_checked")
        .and_then(Json::as_f64)
        .map_or(0, |n| n.clamp(0.0, f64::from(u32::MAX)) as u32);
    Ok(Review {
        verdict,
        concerns,
        diffs_checked,
    })
}

/// A concern given as a string, or as an object with a text field.
fn concern_text(item: &Json) -> Option<String> {
    let s = match item {
        Json::Str(s) => s.as_str(),
        Json::Obj(_) => ["concern", "description", "text", "issue", "message"]
            .iter()
            .find_map(|k| item.get(k).and_then(Json::as_str))?,
        _ => return None,
    };
    let line = s.trim().lines().next().unwrap_or_default().trim();
    Some(line.chars().take(CONCERN_CHARS).collect())
}

/// Length of the balanced `{…}` at the start of `s`, skipping braces in strings.
fn object_end(s: &str) -> Option<usize> {
    let (mut depth, mut in_str, mut escaped) = (0usize, false, false);
    for (i, b) in s.bytes().enumerate() {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Files touched by a `git diff`.
pub fn diff_files(diff: &str) -> usize {
    diff.lines()
        .filter(|l| l.starts_with("diff --git "))
        .count()
}

/// What the reviewer is asked about job `job`.
pub fn review_prompt(job: JobId, prompt: &str, answer: &str, diff: &str) -> String {
    let diff_part = if diff.trim().is_empty() {
        "(no uncommitted changes in tracked files; read any new files the answer mentions)"
            .to_string()
    } else {
        cap(diff, DIFF_CAP)
    };
    // A build that was given an approved plan is also held to it.
    let plan_note = if prompt.contains(crate::plan::PLAN_MARKER) {
        " The task carries a plan the user approved: also report as concerns, each starting \
         with \"drift:\", files changed outside the planned areas, planned tasks with no \
         matching change, and work the plan never asked for."
    } else {
        ""
    };
    format!(
        "swarmux review of job #{job}\n\n\
         Another agent was given the task below and reported the answer below. \
         Check the git diff (and read files if you need to) for bugs, unsafe or \
         unfinished changes, and anything that does not do what the task asked. \
         Do not edit anything.{plan_note}\n\n\
         Task:\n<<<\n{}\n>>>\n\n\
         Answer:\n<<<\n{}\n>>>\n\n\
         git diff ({} files):\n<<<\n{diff_part}\n>>>\n\n\
         Reply with one JSON object and nothing else:\n\
         {{\"verdict\": \"pass\" | \"concerns\", \"concerns\": [\"<one line each>\"], \"diffs_checked\": <number of file diffs you checked>}}",
        cap(prompt, PROMPT_CAP),
        cap(answer, RESULT_CAP),
        diff_files(diff),
    )
}

fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…[cut]", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_json_verdict() {
        let r = parse_review(r#"{"verdict":"pass","concerns":[],"diffs_checked":3}"#).unwrap();
        assert_eq!(
            r,
            Review {
                verdict: ReviewVerdict::Pass,
                concerns: vec![],
                diffs_checked: 3
            }
        );
    }

    #[test]
    fn verdict_inside_fences_and_prose() {
        let text = "Looked at it.\n```json\n{\"verdict\": \"Concerns\", \"concerns\": [\"no test for {edge}\\nmore\", {\"description\": \"leaks a file\"}, 7, \"\"], \"diffs_checked\": 2.0}\n```\nbye {";
        let r = parse_review(text).unwrap();
        assert_eq!(r.verdict, ReviewVerdict::Concerns);
        assert_eq!(r.concerns, ["no test for {edge}", "leaks a file"]);
        assert_eq!(r.diffs_checked, 2);
    }

    #[test]
    fn skips_objects_without_a_verdict() {
        let text = r#"{"note": "x"} then {"verdict": "pass"}"#;
        let r = parse_review(text).unwrap();
        assert_eq!((r.verdict, r.diffs_checked), (ReviewVerdict::Pass, 0));
    }

    #[test]
    fn invalid_answers_say_why() {
        assert!(parse_review("  ").unwrap_err().contains("no answer"));
        assert!(parse_review("looks fine").unwrap_err().contains("no JSON"));
        assert!(
            parse_review(r#"{"ok": true}"#)
                .unwrap_err()
                .contains("no verdict")
        );
        assert!(
            parse_review(r#"{"verdict":"meh"}"#)
                .unwrap_err()
                .contains("meh")
        );
        assert!(parse_review(r#"{"verdict":1}"#).is_err());
        let neg = parse_review(r#"{"verdict":"pass","diffs_checked":-4}"#).unwrap();
        assert_eq!(neg.diffs_checked, 0);
    }

    #[test]
    fn states_round_trip() {
        let states = [
            ReviewState::Requested,
            ReviewState::Running { job: JobId(4) },
            ReviewState::Done {
                review: Review {
                    verdict: ReviewVerdict::Concerns,
                    concerns: vec!["say \"why\"".into()],
                    diffs_checked: 1,
                },
                job: JobId(5),
                cost_usd: 0.0125,
            },
            ReviewState::Failed {
                error: "no JSON".into(),
                job: None,
                cost_usd: 0.0,
            },
        ];
        for s in states {
            assert_eq!(ReviewState::decode(&s.encode()), Ok(s));
        }
        assert!(ReviewState::decode("{}").is_err());
        assert!(ReviewState::decode("nope").is_err());
    }

    #[test]
    fn prompt_has_task_answer_and_capped_diff() {
        let diff = format!("diff --git a/x b/x\n+{}", "y".repeat(DIFF_CAP));
        let p = review_prompt(JobId(9), "fix it", "fixed", &diff);
        assert!(p.starts_with("swarmux review of job #9"));
        assert!(p.contains("<<<\nfix it\n>>>"));
        assert!(p.contains("git diff (1 files)"));
        assert!(p.contains("…[cut]"));
        assert!(p.len() < DIFF_CAP + 4096);
        assert!(review_prompt(JobId(1), "a", "b", "").contains("no uncommitted changes"));
    }
}
