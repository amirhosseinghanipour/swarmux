//! JEV: the optional hosted decision model, used only after the user agreed
//! to what is sent.

use std::cell::Cell;
use std::rc::Rc;

use crate::ports::{AdviceRequest, AdviceSink, Clock, ConsentStore, DecisionAdvisor, HookRequest};
use crate::{AppResult, JevTrace};

/// Longest text (subject or prompt summary) sent to the provider.
pub const SUMMARY_MAX: usize = 200;
/// Steps already tried that a recovery question names.
const RECOVERY_TRIED_MAX: usize = 6;

/// Whether JEV may be asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JevState {
    /// Enabled in the settings; the user hasn't agreed yet.
    NeedsConsent,
    /// The user said no for this run.
    Declined,
    On,
}

/// The configured provider, the user's consent and the confidence needed.
pub struct Jev {
    advisor: Rc<dyn DecisionAdvisor>,
    consents: Rc<dyn ConsentStore>,
    clock: Rc<dyn Clock>,
    threshold: f64,
    state: Cell<JevState>,
}

impl Jev {
    /// Reads whether the user already agreed to this provider.
    pub fn new(
        advisor: Rc<dyn DecisionAdvisor>,
        consents: Rc<dyn ConsentStore>,
        clock: Rc<dyn Clock>,
        threshold: f64,
    ) -> AppResult<Jev> {
        let state = if consents.has_consent(advisor.provider())? {
            JevState::On
        } else {
            JevState::NeedsConsent
        };
        Ok(Jev {
            advisor,
            consents,
            clock,
            threshold,
            state: Cell::new(state),
        })
    }

    pub fn provider(&self) -> &str {
        self.advisor.provider()
    }

    pub fn host(&self) -> &str {
        self.advisor.host()
    }

    pub fn has_key(&self) -> bool {
        self.advisor.has_key()
    }

    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    pub fn state(&self) -> JevState {
        self.state.get()
    }

    /// Consented, with a key: decisions are put to it.
    pub fn is_on(&self) -> bool {
        self.state.get() == JevState::On && self.has_key()
    }

    /// The consent question should be shown (a key exists, no answer yet).
    pub fn wants_consent(&self) -> bool {
        self.state.get() == JevState::NeedsConsent && self.has_key()
    }

    /// Remembers that the user agreed, and turns JEV on.
    pub fn grant(&self) -> AppResult<()> {
        self.consents
            .grant_consent(self.provider(), self.clock.now())?;
        self.state.set(JevState::On);
        Ok(())
    }

    /// Keeps JEV off for this run; it asks again next time.
    pub fn decline(&self) {
        self.state.set(JevState::Declined);
    }

    /// What will be sent and where, for the consent screen.
    pub fn disclosure(&self) -> Vec<String> {
        vec![
            format!(
                "JEV ({}) is enabled in config.toml. Before it is used, this is what swarmux sends to {}:",
                self.provider(),
                self.host()
            ),
            format!(
                "· for tool calls no rule matches: the tool name and a summary of its command, path or URL (at most {SUMMARY_MAX} characters)"
            ),
            format!(
                "· for \"let swarmux route\": the first {SUMMARY_MAX} characters of the prompt and the names of your sessions"
            ),
            format!(
                "· when the watchdog thinks an agent is stuck: what it saw and what it tried, one line each (at most {SUMMARY_MAX} characters)"
            ),
            "Never file contents, diffs, folders or the rest of the prompt. Answers below the threshold still come to you.".into(),
        ]
    }

    /// Asks whether `request` may run. Only the tool name and its subject
    /// (command, path or URL) are sent, never the rest of its input. A call
    /// with no subject (an MCP or unknown tool) is not sent at all: there
    /// is nothing to judge, and `None` comes back. Otherwise what was sent
    /// is returned: the provider, the model, a hash of the exact request and
    /// whether the subject was cut.
    pub(crate) fn ask_tool(&self, request: &HookRequest, sink: AdviceSink) -> Option<JevTrace> {
        if request.subject.trim().is_empty() {
            return None;
        }
        let summary = summarize(&request.subject, SUMMARY_MAX);
        let truncated = flatten(&request.subject).chars().count() > SUMMARY_MAX;
        let advice = AdviceRequest::ToolCall {
            tool: request.tool_name.clone(),
            summary,
        };
        let trace = JevTrace {
            provider: self.provider().to_string(),
            model: self.advisor.model().to_string(),
            prompt_sha256: self.advisor.prompt_sha256(&advice),
            truncated,
        };
        self.advisor.advise(advice, sink);
        Some(trace)
    }

    /// Asks whether a stuck agent should carry on, change approach or stop.
    /// Only the watchdog's own one-line notes are sent.
    pub(crate) fn ask_recovery(&self, situation: &str, tried: &[String], sink: AdviceSink) {
        self.advisor.advise(
            AdviceRequest::Recovery {
                situation: summarize(situation, SUMMARY_MAX),
                tried: tried
                    .iter()
                    .rev()
                    .take(RECOVERY_TRIED_MAX)
                    .rev()
                    .map(|t| summarize(t, SUMMARY_MAX))
                    .collect(),
            },
            sink,
        );
    }

    /// Asks which of `candidates` should take `prompt`.
    pub(crate) fn ask_session(&self, prompt: &str, candidates: Vec<String>, sink: AdviceSink) {
        self.advisor.advise(
            AdviceRequest::Session {
                prompt: summarize(prompt, SUMMARY_MAX),
                candidates,
            },
            sink,
        );
    }
}

fn flatten(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `s` on one line, cut to `max` characters.
pub fn summarize(s: &str, max: usize) -> String {
    let flat = flatten(s);
    match flat.char_indices().nth(max) {
        Some((i, _)) => flat[..i].to_string(),
        None => flat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeAdvisor, FakeConsents, FixedClock};

    fn jev(consents: Rc<FakeConsents>) -> (Jev, Rc<FakeAdvisor>) {
        let advisor = Rc::new(FakeAdvisor::default());
        let clock = Rc::new(FixedClock::new(7));
        let jev = Jev::new(advisor.clone(), consents, clock, 0.85).unwrap();
        (jev, advisor)
    }

    #[test]
    fn needs_consent_until_granted_and_remembers_it() {
        let consents = Rc::new(FakeConsents::default());
        let (j, _) = jev(consents.clone());
        assert_eq!(j.state(), JevState::NeedsConsent);
        assert!(!j.is_on());
        j.grant().unwrap();
        assert!(j.is_on());
        assert_eq!(consents.granted(), [("typesafe".to_string(), 7)]);
        let (again, _) = jev(consents);
        assert_eq!(again.state(), JevState::On);
    }

    #[test]
    fn without_a_key_it_is_off_and_asks_nothing() {
        let (j, advisor) = jev(Rc::new(FakeConsents::default()));
        advisor.set_no_key(true);
        assert!(!j.wants_consent());
        j.grant().unwrap();
        assert!(!j.is_on());
        advisor.set_no_key(false);
        assert!(j.is_on());
    }

    #[test]
    fn decline_lasts_this_run() {
        let consents = Rc::new(FakeConsents::default());
        let (j, _) = jev(consents.clone());
        j.decline();
        assert_eq!(j.state(), JevState::Declined);
        assert!(consents.granted().is_empty());
    }

    #[test]
    fn disclosure_names_host_and_limits() {
        let (j, _) = jev(Rc::new(FakeConsents::default()));
        let text = j.disclosure().join("\n");
        assert!(text.contains("fake.example"), "{text}");
        assert!(text.contains("200 characters"), "{text}");
        assert!(text.contains("Never file contents"), "{text}");
    }

    #[test]
    fn requests_are_shortened_before_leaving() {
        let (j, advisor) = jev(Rc::new(FakeConsents::default()));
        let request = HookRequest {
            request_id: 1,
            job_id: None,
            tool_name: "Bash".into(),
            subject: format!("echo {}\nsecond line", "x".repeat(300)),
            input: "{\"secret\":\"file contents\"}".into(),
            cwd: "/w".into(),
            content: String::new(),
            ..HookRequest::default()
        };
        j.ask_tool(&request, Box::new(|_| {}));
        j.ask_session(&"y".repeat(5000), vec!["a".into()], Box::new(|_| {}));
        let sent = advisor.requests();
        let AdviceRequest::ToolCall { tool, summary } = &sent[0] else {
            panic!("{sent:?}")
        };
        assert_eq!(tool, "Bash");
        assert_eq!(summary.chars().count(), SUMMARY_MAX);
        assert!(!summary.contains('\n') && !summary.contains("secret"));
        let AdviceRequest::Session { prompt, candidates } = &sent[1] else {
            panic!("{sent:?}")
        };
        assert_eq!(prompt.chars().count(), SUMMARY_MAX);
        assert_eq!(candidates, &["a"]);
    }

    #[test]
    fn summarize_flattens_and_cuts_on_chars() {
        assert_eq!(summarize("a\n  b\tc", 10), "a b c");
        assert_eq!(summarize("éééé", 2), "éé");
    }
}
