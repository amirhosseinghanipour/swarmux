//! State of the Missions screen: the selected card and the composer, where
//! a mission is written and the lint's questions are answered inline.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use swarmux_domain::mission::{Budget, Clarification, Contract, Mission};

use crate::ui::input::Input;

#[derive(Default)]
pub struct MissionsScreen {
    /// The selected card, by place (cards never move).
    pub selected: usize,
    pub composer: Option<Composer>,
    /// The mission whose proposed plan a note is being written for.
    pub note_for: Option<String>,
}

impl MissionsScreen {
    pub fn move_by(&mut self, delta: isize, count: usize) {
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(count.saturating_sub(1));
    }
}

/// The composer's fields, top to bottom; answers follow them.
pub const FIELDS: [&str; 7] = [
    "title", "goal", "checks", "work in", "protect", "budget", "repos",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    None,
    /// Save and lint.
    Save,
    /// Protect the oracles the lint found.
    Fix,
    /// Save, then run.
    Run,
    Cancel,
}

/// Writing a new mission, or changing one that is not out.
pub struct Composer {
    /// The mission once saved.
    pub slug: Option<String>,
    pub inputs: [Input; 7],
    /// One per question of the lint, in order.
    pub answers: Vec<Input>,
    /// Index into the fields, then into the answers.
    pub focus: usize,
}

impl Composer {
    pub fn blank() -> Composer {
        Composer {
            slug: None,
            inputs: Default::default(),
            answers: Vec::new(),
            focus: 0,
        }
    }

    pub fn edit(m: &Mission) -> Composer {
        let c = &m.contract;
        let mut composer = Composer {
            slug: Some(m.slug.clone()),
            inputs: [
                Input::with_text(&c.title),
                Input::with_text(&c.goal),
                Input::with_text(&c.acceptance.checks.join("; ")),
                Input::with_text(&c.constraints.paths_rw.join(", ")),
                Input::with_text(&c.constraints.protected.join(", ")),
                Input::with_text(&budget_text(&c.budget)),
                Input::with_text(&c.repos.join(", ")),
            ],
            answers: Vec::new(),
            focus: 1,
        };
        composer.sync(m);
        composer
    }

    /// One answer field per question, keeping what was typed.
    pub fn sync(&mut self, m: &Mission) {
        let questions = &m.contract.clarifications;
        while self.answers.len() < questions.len() {
            let q = &questions[self.answers.len()];
            self.answers.push(Input::with_text(&q.answer));
        }
        self.answers.truncate(questions.len());
        self.focus = self.focus.min(self.len() - 1);
    }

    fn len(&self) -> usize {
        FIELDS.len() + self.answers.len()
    }

    /// The answer being typed, by question.
    pub fn answering(&self) -> Option<usize> {
        self.focus.checked_sub(FIELDS.len())
    }

    /// Every key goes to the composer while it is open.
    pub fn handle(&mut self, key: KeyEvent) -> ComposerAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return ComposerAction::Cancel,
            KeyCode::Char('s') if ctrl => return ComposerAction::Save,
            KeyCode::Char('f') if ctrl => return ComposerAction::Fix,
            KeyCode::Char('r') if ctrl => return ComposerAction::Run,
            KeyCode::Tab | KeyCode::Down | KeyCode::Enter => {
                self.focus = (self.focus + 1) % self.len();
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.focus = (self.focus + self.len() - 1) % self.len();
            }
            _ => {
                // Enter and Esc are handled above, so inputs never submit.
                let input = match self.answering() {
                    Some(i) => &mut self.answers[i],
                    None => &mut self.inputs[self.focus],
                };
                input.handle(key);
            }
        }
        ComposerAction::None
    }

    /// The contract the fields describe, with `asked`'s questions answered
    /// from the answer fields.
    pub fn contract(&self, asked: &[Clarification]) -> Result<Contract, String> {
        let [title, goal, checks, rw, protect, budget, repos] = &self.inputs;
        let goal = goal.text.trim();
        if goal.is_empty() {
            return Err("a mission needs a goal".into());
        }
        Ok(Contract {
            title: title.text.trim().to_string(),
            goal: goal.to_string(),
            acceptance: swarmux_domain::mission::Acceptance {
                checks: split(&checks.text, ';'),
                human_signoff: false,
            },
            constraints: swarmux_domain::mission::Constraints {
                paths_rw: split(&rw.text, ','),
                protected: split(&protect.text, ','),
            },
            budget: parse_budget(&budget.text)?,
            repos: split(&repos.text, ','),
            clarifications: asked
                .iter()
                .enumerate()
                .map(|(i, q)| Clarification {
                    question: q.question.clone(),
                    answer: self
                        .answers
                        .get(i)
                        .map_or_else(|| q.answer.clone(), |a| a.text.trim().to_string()),
                })
                .collect(),
        })
    }
}

fn split(text: &str, sep: char) -> Vec<String> {
    text.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// `$2.50 90m` → usd 2.5, 90 minutes. Either may be left out; `1h30`
/// and `2h` work for the time.
pub fn parse_budget(text: &str) -> Result<Budget, String> {
    let mut budget = Budget::default();
    for word in text.split([' ', ',', '/']).filter(|w| !w.is_empty()) {
        let w = word.trim().to_ascii_lowercase();
        if let Some(mins) = minutes(&w) {
            budget.wall_minutes = Some(mins);
            continue;
        }
        let usd: f64 = w
            .trim_start_matches('$')
            .parse()
            .map_err(|_| format!("budget: {word:?} is neither dollars nor a time (90m, 2h)"))?;
        if !(usd.is_finite() && usd > 0.0) {
            return Err(format!("budget: {word:?} must be above 0"));
        }
        budget.usd = Some(usd);
    }
    Ok(budget)
}

fn minutes(w: &str) -> Option<u32> {
    if let Some(m) = w.strip_suffix('m') {
        return m.parse().ok();
    }
    let (h, rest) = w.split_once('h')?;
    let h: u32 = h.parse().ok()?;
    let m: u32 = if rest.is_empty() {
        0
    } else {
        rest.parse().ok()?
    };
    Some(h * 60 + m)
}

fn budget_text(b: &Budget) -> String {
    let mut parts = Vec::new();
    if let Some(usd) = b.usd {
        parts.push(format!("${usd:.2}"));
    }
    if let Some(m) = b.wall_minutes {
        parts.push(format!("{m}m"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(c: &mut Composer, s: &str) {
        for ch in s.chars() {
            c.handle(key(KeyCode::Char(ch)));
        }
    }

    #[test]
    fn budgets_read_dollars_and_times() {
        let b = parse_budget("$2.50 90m").unwrap();
        assert_eq!((b.usd, b.wall_minutes), (Some(2.5), Some(90)));
        assert_eq!(parse_budget("1h30").unwrap().wall_minutes, Some(90));
        assert_eq!(parse_budget("2h, 3").unwrap().usd, Some(3.0));
        assert_eq!(parse_budget("").unwrap(), Budget::default());
        assert!(parse_budget("lots").unwrap_err().contains("lots"));
        assert!(parse_budget("$0").is_err());
        assert_eq!(budget_text(&parse_budget("$2.5 90m").unwrap()), "$2.50 90m");
    }

    #[test]
    fn fields_make_the_contract() {
        let mut c = Composer::blank();
        typed(&mut c, "Fast search");
        c.handle(key(KeyCode::Tab));
        typed(&mut c, "make search fast");
        c.handle(key(KeyCode::Enter));
        typed(&mut c, "cargo test -p search; sh bench.sh");
        c.handle(key(KeyCode::Tab));
        typed(&mut c, "src/search/**");
        c.handle(key(KeyCode::Tab));
        typed(&mut c, "tests/**, bench.sh,");
        c.handle(key(KeyCode::Tab));
        typed(&mut c, "$2");
        let contract = c.contract(&[]).unwrap();
        assert_eq!(contract.title, "Fast search");
        assert_eq!(
            contract.acceptance.checks,
            ["cargo test -p search", "sh bench.sh"]
        );
        assert_eq!(contract.constraints.protected, ["tests/**", "bench.sh"]);
        assert_eq!(contract.budget.usd, Some(2.0));
        assert_eq!(
            c.handle(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
            ComposerAction::Save
        );
        assert!(Composer::blank().contract(&[]).is_err(), "a goal is needed");
    }

    #[test]
    fn answers_follow_the_questions_and_keep_what_was_typed() {
        let mut m = Mission::new("m", "/r", Contract::default(), 0);
        m.contract.goal = "g".into();
        m.contract.clarifications = vec![Clarification {
            question: "Which corpus?".into(),
            answer: String::new(),
        }];
        let mut c = Composer::edit(&m);
        c.handle(key(KeyCode::BackTab));
        assert_eq!(c.focus, 0, "from the goal back to the title");
        c.handle(key(KeyCode::BackTab));
        assert_eq!(c.answering(), Some(0), "wraps to the last answer");
        typed(&mut c, "bench");
        m.contract.clarifications.push(Clarification {
            question: "p99?".into(),
            answer: "yes".into(),
        });
        c.sync(&m);
        assert_eq!(c.answers.len(), 2);
        let contract = c.contract(&m.contract.clarifications).unwrap();
        let answers: Vec<&str> = contract
            .clarifications
            .iter()
            .map(|q| q.answer.as_str())
            .collect();
        assert_eq!(answers, ["bench", "yes"]);
        m.contract.clarifications.clear();
        c.sync(&m);
        assert!(c.answers.is_empty());
        assert!(c.focus < FIELDS.len());
    }
}
