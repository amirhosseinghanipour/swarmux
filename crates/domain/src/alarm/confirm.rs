//! Confirming an irreversible call: the user names the target (types it,
//! or picks it from a short rotated list) after the prompt has armed. The
//! delay and the changing layout keep a habitual keystroke from answering.

use crate::Millis;
use crate::attention::destructive_command;

use super::risk::{is_shell, program_and_args};

/// Keys are ignored for this long after the prompt opens.
pub const ARMING_MS: Millis = 1_500;

/// How many choices the list offers.
const CHOICES: usize = 4;

/// What the user has to name: the branch or path the call acts on.
pub fn target_of(tool: &str, subject: &str) -> String {
    let subject = subject.trim();
    if !is_shell(tool) {
        return subject.to_string();
    }
    let segment = subject
        .split(['\n', ';', '|', '&'])
        .find(|s| destructive_command(s))
        .or_else(|| subject.split(['\n', ';', '|', '&']).next_back())
        .unwrap_or(subject);
    match program_and_args(segment) {
        Some((program, args)) => args
            .iter()
            .rev()
            .find(|a| !a.starts_with('-'))
            .map_or(program, |a| a.trim_matches(['"', '\'']).to_string()),
        None => subject.to_string(),
    }
}

/// An open confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Challenge {
    /// The held call it answers.
    pub request: u64,
    pub tool: String,
    pub command: String,
    pub target: String,
    options: Vec<String>,
    correct: usize,
    opened_at: Millis,
}

impl Challenge {
    pub fn new(request: u64, tool: &str, subject: &str, now: Millis) -> Challenge {
        let target = target_of(tool, subject);
        let mut decoys: Vec<String> = Vec::new();
        let parent = target
            .rsplit_once(['/', '\\'])
            .map(|(dir, _)| dir.to_string())
            .filter(|d| !d.is_empty());
        for d in [
            Some("main".to_string()),
            Some("master".to_string()),
            parent,
            Some("origin".to_string()),
            Some(".".to_string()),
            Some("..".to_string()),
            Some("~".to_string()),
        ]
        .into_iter()
        .flatten()
        {
            if d != target && !decoys.contains(&d) {
                decoys.push(d);
            }
        }
        decoys.truncate(CHOICES - 1);
        // The right answer sits in a different place for every request.
        let correct = (request % CHOICES as u64) as usize;
        let mut options = decoys;
        options.insert(correct.min(options.len()), target.clone());
        let correct = options.iter().position(|o| *o == target).unwrap_or(0);
        Challenge {
            request,
            tool: tool.to_string(),
            command: format!("{tool} {subject}").trim().to_string(),
            target,
            options,
            correct,
            opened_at: now,
        }
    }

    /// When the prompt opened.
    pub fn opened_at(&self) -> Millis {
        self.opened_at
    }

    /// The choices in the order shown.
    pub fn options(&self) -> &[String] {
        &self.options
    }

    /// How far the layout is pushed right, `0..=2` columns, so the dialog
    /// is never in quite the same place.
    pub fn shift(&self) -> u16 {
        (self.request % 3) as u16
    }

    pub fn armed(&self, now: Millis) -> bool {
        now - self.opened_at >= ARMING_MS
    }

    /// Milliseconds until keys count.
    pub fn arming_left(&self, now: Millis) -> Millis {
        (ARMING_MS - (now - self.opened_at)).max(0)
    }

    /// Whether `typed` is the target. Nothing counts before arming.
    pub fn accepts_text(&self, typed: &str, now: Millis) -> bool {
        self.armed(now) && typed.trim() == self.target
    }

    /// Whether choice number `n` (1-based) is the target. Nothing counts
    /// before arming.
    pub fn accepts_choice(&self, n: usize, now: Millis) -> bool {
        self.armed(now) && n >= 1 && n - 1 == self.correct
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_is_the_branch_or_path_acted_on() {
        let t = |tool, subject| target_of(tool, subject);
        assert_eq!(t("Bash", "git push origin feat/checkout"), "feat/checkout");
        assert_eq!(t("Bash", "rm -rf target/debug"), "target/debug");
        assert_eq!(t("Bash", "cargo build && rm -r build"), "build");
        assert_eq!(t("Bash", "git reset --hard HEAD~1"), "HEAD~1");
        assert_eq!(t("Bash", "git push --force"), "push");
        assert_eq!(t("Write", "src/lib.rs"), "src/lib.rs");
    }

    #[test]
    fn keys_do_not_count_before_the_prompt_has_armed() {
        let c = Challenge::new(7, "Bash", "git push origin feat/x", 10_000);
        assert!(!c.accepts_text("feat/x", 10_000));
        assert!(!c.accepts_text("feat/x", 11_499));
        assert!(c.accepts_text("feat/x", 11_500));
        assert!(!c.accepts_text("feat/y", 20_000));
        assert_eq!(c.arming_left(10_500), 1_000);
        assert_eq!(c.arming_left(99_000), 0);
        let right = c.options().iter().position(|o| o == "feat/x").unwrap() + 1;
        assert!(!c.accepts_choice(right, 10_100));
        assert!(c.accepts_choice(right, 12_000));
        assert!(!c.accepts_choice(if right == 1 { 2 } else { 1 }, 12_000));
        assert!(!c.accepts_choice(0, 12_000));
    }

    #[test]
    fn the_right_choice_moves_from_request_to_request() {
        let place = |request| {
            let c = Challenge::new(request, "Bash", "rm -rf build", 0);
            assert_eq!(c.options().len(), CHOICES);
            c.options().iter().position(|o| o == "build").unwrap()
        };
        let places: Vec<_> = (0..4).map(place).collect();
        assert_eq!(places, [0, 1, 2, 3]);
        assert_ne!(
            Challenge::new(1, "Bash", "rm -rf build", 0).shift(),
            Challenge::new(2, "Bash", "rm -rf build", 0).shift()
        );
    }
}
