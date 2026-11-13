//! The work product: what the outcome built against what was planned, what
//! the agent says against what the checks found, the evidence, and what
//! landing does. Every line comes from the console snapshot.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use swarmux_application::console::{
    CheckRunView, CheckStatus, ChecksSource, ConsoleSnapshot, GateVerdict, HoldState, HoldView,
    LandState, LandingMode, OutcomeView, PlanStatus, RiskTier, SignatureState, StageKey,
    StageState, StageView, StepKind, TaskState,
};
use swarmux_domain::Millis;

use super::util::{
    Lines, Mark, arrow, clock, columns, duration, fit, heading, mark, pair, quote, span,
};
use crate::app::App;
use crate::ui::mission::age;
use crate::work::{countdown, holds_of, short};

/// Narrowest right pane that shows two columns.
const TWO_COLUMNS_FROM: usize = 104;

/// The whole tab, as rows.
pub(super) fn body(app: &App, snap: &ConsoleSnapshot, o: &OutcomeView, w: usize) -> Lines {
    let now = app.now();
    let mut lines: Lines = Vec::new();
    let holds = holds_of(snap, o);
    let hold_lines = |w: usize| -> Lines {
        let mut out = Vec::new();
        for h in &holds {
            out.extend(hold(app, h, now, w));
            out.push(Line::default());
        }
        out
    };
    if w >= TWO_COLUMNS_FROM {
        let lw = (w - 3) / 2;
        let rw = w - 3 - lw;
        let mut left = hold_lines(lw);
        left.extend(plan_drift(app, o, lw));
        let right = claim_checks(app, o, rw);
        lines.extend(columns(left, lw, 3, right));
    } else {
        lines.extend(hold_lines(w));
        lines.extend(plan_drift(app, o, w));
        lines.push(Line::default());
        lines.extend(claim_checks(app, o, w));
    }
    lines.push(Line::default());
    lines.extend(evidence(app, o, now, w));
    let landing = landing(app, o, w);
    if !landing.is_empty() {
        lines.push(Line::default());
        lines.extend(landing);
    }
    lines
}

// ---------------------------------------------------------------------
// The Guard hold
// ---------------------------------------------------------------------

/// The call the Guard held or refused, the rule, the agent's own words
/// (quoted and labelled, never interpreted) and the last steps.
fn hold(app: &App, h: &HoldView, now: Millis, w: usize) -> Lines {
    let t = &app.look.theme;
    let held = h.state == HoldState::Held;
    let (title, color) = if held {
        ("THE CALL GUARD HELD", t.warn)
    } else {
        ("THE CALL GUARD REFUSED", t.error)
    };
    let mut out = vec![heading(app, title, "", w, color)];
    let call = format!("{} {}", h.tool, h.target);
    out.push(Line::from(vec![
        span("  ", Style::default()),
        span(
            fit(&call, w.saturating_sub(2)),
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        ),
    ]));
    let key_w = 5;
    let rule = match (&h.rule, &h.knowledge_rule) {
        (Some(r), Some(k)) => format!("{r} (knowledge {k})"),
        (Some(r), None) => r.clone(),
        (None, _) => "none matched: the Guard asks by default".to_string(),
    };
    out.push(indent(pair(
        t,
        "rule",
        key_w,
        vec![span(
            fit(&rule, w.saturating_sub(9)),
            Style::new().fg(t.guard),
        )],
    )));
    if let Some(r) = h.risk {
        let tier = match r.tier {
            RiskTier::Routine => "routine",
            RiskTier::Elevated => "elevated",
            RiskTier::Destructive => "destructive",
        };
        let mut text = format!("{tier} · {:.0}%", r.score * 100.0);
        if h.destructive {
            text.push_str(" · typed confirm, never batched");
        }
        if r.secret {
            text.push_str(" · may be sending a secret out");
        }
        let hue = match r.tier {
            RiskTier::Destructive => t.error,
            RiskTier::Elevated => t.warn,
            RiskTier::Routine => t.muted,
        };
        out.push(indent(pair(
            t,
            "risk",
            key_w,
            vec![span(fit(&text, w.saturating_sub(9)), Style::new().fg(hue))],
        )));
    }
    let timing = if held {
        let mut s = format!("{} ago", age(now - h.at));
        if let Some(d) = h.deadline_at {
            s.push_str(&format!(" · auto-deny in {}", countdown(d, now)));
        }
        s
    } else {
        format!("{} ago", age(now - h.at))
    };
    out.push(indent(pair(
        t,
        if held { "held" } else { "when" },
        key_w,
        vec![span(timing, Style::new().fg(t.text))],
    )));

    if let Some(words) = &h.agent_words {
        out.push(Line::default());
        let since = if words.steps_since == 0 {
            "just before the call".to_string()
        } else {
            format!("{} steps before the call", words.steps_since)
        };
        out.push(Line::from(vec![
            span(
                "AGENT'S OWN WORDS",
                Style::new().fg(t.label).add_modifier(Modifier::BOLD),
            ),
            span(
                format!("  stream {} · {since}", clock(words.at)),
                Style::new().fg(t.muted),
            ),
        ]));
        out.extend(quote(&words.text, w, 2, 5, Style::new().fg(t.text)));
    }

    if !h.steps.is_empty() {
        out.push(Line::default());
        out.push(heading(app, "LAST STEPS", "before the call", w, t.label));
        for s in &h.steps {
            let (kind, hue) = match s.kind {
                StepKind::Text => ("said", t.label),
                StepKind::ToolCall => ("call", t.text),
                StepKind::ToolResult if s.ok == Some(false) => ("failed", t.error),
                StepKind::ToolResult => ("result", t.muted),
            };
            let head = format!("{} {kind:<6} ", clock(s.at));
            let room = w.saturating_sub(head.chars().count() + 2);
            out.push(Line::from(vec![
                span(format!("  {head}"), Style::new().fg(t.dim)),
                span(fit(&s.text, room), Style::new().fg(hue)),
            ]));
        }
    }
    out
}

fn indent(mut line: Line<'static>) -> Line<'static> {
    line.spans.insert(0, Span::raw("  "));
    line
}

// ---------------------------------------------------------------------
// Plan against drift
// ---------------------------------------------------------------------

fn plan_drift(app: &App, o: &OutcomeView, w: usize) -> Lines {
    let t = &app.look.theme;
    let mut out = vec![heading(app, "PLAN vs DRIFT", "", w, t.label)];
    let Some(plan) = &o.plan else {
        out.push(Line::from(span(
            "  no plan: it worked from the prompt",
            Style::new().fg(t.muted),
        )));
        return out;
    };
    match plan.status {
        PlanStatus::Proposed => out.push(Line::from(span(
            "  proposed: approve it or send it back",
            Style::new().fg(t.warn),
        ))),
        PlanStatus::Planning => out.push(Line::from(span(
            "  the planner is reading the repository",
            Style::new().fg(t.muted),
        ))),
        PlanStatus::Failed => out.push(Line::from(span(
            format!(
                "  planning failed: {}",
                plan.note.as_deref().unwrap_or("no reason given")
            ),
            Style::new().fg(t.error),
        ))),
        _ => {}
    }
    if !plan.summary.is_empty() {
        out.push(Line::from(span(
            format!("  {}", fit(&plan.summary, w.saturating_sub(2))),
            Style::new().fg(t.text),
        )));
    }
    let files_w = (w / 3).max(14);
    for task in plan.tasks.iter().take(8) {
        let state = match task.state {
            Some(TaskState::Merged) => Some((Mark::Pass, t.ok)),
            Some(TaskState::Failed | TaskState::Conflict) => Some((Mark::Fail, t.error)),
            Some(TaskState::Running | TaskState::Gate | TaskState::Merging) => {
                Some((Mark::Open, t.label))
            }
            Some(TaskState::Waiting) => Some((Mark::Idle, t.dim)),
            None => None,
        };
        let mut spans = vec![span("  ", Style::default())];
        if let Some((m, hue)) = state {
            spans.push(span(format!("{} ", mark(app, m)), Style::new().fg(hue)));
        }
        let head = format!("task {} ", task.id);
        let title_w = w.saturating_sub(head.chars().count() + files_w + 5).max(8);
        spans.push(span(head, Style::new().fg(t.dim)));
        spans.push(span(fit(&task.title, title_w), Style::new().fg(t.text)));
        if !task.files.is_empty() {
            spans.push(span("  ", Style::default()));
            spans.push(span(
                fit(&task.files.join(" "), files_w),
                Style::new().fg(t.muted),
            ));
        }
        out.push(Line::from(spans));
    }
    if plan.tasks.len() > 8 {
        out.push(Line::from(span(
            format!("  +{} more tasks", plan.tasks.len() - 8),
            Style::new().fg(t.muted),
        )));
    }
    out.push(Line::default());
    match &plan.drift {
        Some(d) => {
            let hue = if d.drifted { t.warn } else { t.ok };
            out.push(Line::from(vec![
                span("  drift ", Style::new().fg(t.muted)),
                span(fit(&d.summary, w.saturating_sub(9)), Style::new().fg(hue)),
            ]));
            for f in d.outside.iter().take(4) {
                out.push(Line::from(vec![
                    span(
                        format!("  {} ", mark(app, Mark::Flag)),
                        Style::new().fg(t.warn),
                    ),
                    span(fit(f, w.saturating_sub(16)), Style::new().fg(t.text)),
                    span("  outside plan", Style::new().fg(t.warn)),
                ]));
            }
            if d.outside_count as usize > d.outside.len().min(4) {
                out.push(Line::from(span(
                    format!(
                        "    +{} more outside the plan",
                        d.outside_count as usize - d.outside.len().min(4)
                    ),
                    Style::new().fg(t.muted),
                )));
            }
            for id in d.untouched.iter().take(4) {
                out.push(Line::from(span(
                    format!("  planned task {id} untouched"),
                    Style::new().fg(t.muted),
                )));
            }
            out.push(Line::from(span(
                "  drift is reported, not blocking",
                Style::new().fg(t.dim),
            )));
        }
        None => out.push(Line::from(span(
            "  drift is checked once the build is out",
            Style::new().fg(t.dim),
        ))),
    }
    out
}

// ---------------------------------------------------------------------
// The claim against the checks
// ---------------------------------------------------------------------

fn claim_checks(app: &App, o: &OutcomeView, w: usize) -> Lines {
    let t = &app.look.theme;
    let mut out = vec![heading(app, "CLAIM vs CHECKS", "", w, t.label)];
    match &o.claim {
        Some(c) => {
            out.push(Line::from(span(
                format!("agent's claim · job #{}", c.job),
                Style::new().fg(t.muted),
            )));
            out.extend(quote(&c.text, w, 1, 5, Style::new().fg(t.text)));
            if c.truncated {
                out.push(Line::from(span(
                    "  (cut: the full text is in Focus)",
                    Style::new().fg(t.dim),
                )));
            }
        }
        None => out.push(Line::from(span(
            "no claim yet: the job has not reported",
            Style::new().fg(t.muted),
        ))),
    }
    out.push(Line::default());
    out.extend(check_summary(app, o, w, false));
    out
}

/// `CHECKS 9 of 9 passed`, each run, then the stages. `full` also lists
/// what each stage found.
pub(super) fn check_summary(app: &App, o: &OutcomeView, w: usize, full: bool) -> Lines {
    let t = &app.look.theme;
    let c = &o.checks;
    let mut out: Lines = Vec::new();
    let source = match c.source {
        ChecksSource::None => "not run yet",
        ChecksSource::Gate => "as the gate ran them",
        ChecksSource::Bundle => "from the sealed bundle",
    };
    let mut head = vec![
        span(
            "CHECKS ",
            Style::new().fg(t.label).add_modifier(Modifier::BOLD),
        ),
        span(
            format!("{} of {} passed", c.passed, c.total),
            Style::new().fg(if c.failed > 0 { t.error } else { t.text }),
        ),
        span(format!(" · {source}"), Style::new().fg(t.muted)),
    ];
    if let Some(g) = c.gate {
        let (word, hue) = match g.verdict {
            GateVerdict::Pass => ("gate passed", t.ok),
            GateVerdict::Block => ("gate blocked", t.warn),
            GateVerdict::Escalate => ("gate gave up", t.error),
        };
        let blocks = match g.max_blocks {
            Some(max) => format!(" ({} of {max} stops)", g.blocks),
            None if g.blocks > 0 => format!(
                " ({} stop{})",
                g.blocks,
                if g.blocks == 1 { "" } else { "s" }
            ),
            None => String::new(),
        };
        head.push(span(format!(" · {word}{blocks}"), Style::new().fg(hue)));
    }
    out.push(Line::from(head));
    for r in &c.runs {
        out.extend(run_lines(app, r, w));
    }
    if c.runs.is_empty() && o.contract.as_ref().is_some_and(|c| !c.done_when.is_empty()) {
        out.push(Line::from(span(
            "  none of the checks has run yet",
            Style::new().fg(t.muted),
        )));
    }
    // In a narrow pane the stages that passed fold into one line: what
    // needs a look is listed, the rest is counted.
    if !full && w < TWO_COLUMNS_FROM {
        out.extend(stages_compact(app, &c.stages, w));
    } else {
        out.push(Line::default());
        for s in &c.stages {
            out.extend(stage_lines(app, s, w, full));
        }
    }
    out
}

fn run_lines(app: &App, r: &CheckRunView, w: usize) -> Lines {
    let t = &app.look.theme;
    let (m, word, hue) = match r.status {
        CheckStatus::Passed => (Mark::Ready, "pass", t.ok),
        CheckStatus::Flaky => (Mark::Flag, "flaky", t.warn),
        CheckStatus::Failed => (Mark::Fail, "FAIL", t.error),
        CheckStatus::TimedOut => (Mark::Fail, "TIMEOUT", t.error),
        CheckStatus::Error => (Mark::Fail, "ERROR", t.error),
    };
    let dur = duration(r.duration_ms);
    let name_w = w.saturating_sub(2 + 2 + 8 + 7).max(8);
    let mut out = vec![Line::from(vec![
        span(format!("  {} ", mark(app, m)), Style::new().fg(hue)),
        span(
            format!("{:<name_w$}", fit(&r.name, name_w)),
            Style::new().fg(t.text),
        ),
        span(format!(" {word:<7}"), Style::new().fg(hue)),
        span(format!("{dur:>7}"), Style::new().fg(t.muted)),
    ])];
    if let Some(line) = &r.first_failure {
        out.push(Line::from(vec![
            span("      ", Style::default()),
            span(fit(line, w.saturating_sub(6)), Style::new().fg(t.error)),
        ]));
    }
    out
}

fn stage_lines(app: &App, s: &StageView, w: usize, full: bool) -> Lines {
    let t = &app.look.theme;
    let (m, hue) = match s.state {
        StageState::Passed => (Mark::Ready, t.ok),
        StageState::Failed => (Mark::Fail, t.error),
        StageState::Flagged => (Mark::Flag, t.warn),
        StageState::NotRun => (Mark::Idle, t.dim),
    };
    let name = stage_name(s.key);
    // Hidden checks are only ever shown as counts: which ones ran stays
    // with the verifier.
    let summary = match (s.key, s.ran, s.passed) {
        (StageKey::HiddenChecks, Some(ran), Some(passed)) => format!("{passed} of {ran} passed"),
        _ => s.summary.clone(),
    };
    let name_w = 16;
    let mut out = vec![Line::from(vec![
        span(format!("  {} ", mark(app, m)), Style::new().fg(hue)),
        span(format!("{name:<name_w$}"), Style::new().fg(t.text)),
        span(
            fit(&summary, w.saturating_sub(name_w + 4)),
            Style::new().fg(if s.state == StageState::NotRun {
                t.dim
            } else {
                t.muted
            }),
        ),
    ])];
    if full && s.key != StageKey::HiddenChecks {
        for item in s.items.iter().take(6) {
            out.push(Line::from(span(
                format!("      {}", fit(item, w.saturating_sub(6))),
                Style::new().fg(hue),
            )));
        }
    }
    out
}

// ---------------------------------------------------------------------
// Evidence and landing
// ---------------------------------------------------------------------

fn evidence(app: &App, o: &OutcomeView, now: Millis, w: usize) -> Lines {
    let t = &app.look.theme;
    let mut out = vec![heading(app, "EVIDENCE", "", w, t.label)];
    match &o.evidence {
        Some(e) => {
            let (sig, hue) = match e.signature {
                SignatureState::Valid => ("signature valid", t.ok),
                SignatureState::Invalid => ("signature does not match", t.error),
                SignatureState::Unsigned => ("unsigned", t.warn),
                SignatureState::Unchecked => ("signature uses an algorithm this version cannot check", t.warn),
            };
            out.push(Line::from(vec![
                span("  bundle ", Style::new().fg(t.muted)),
                span(short(&e.bundle_hash).to_string(), Style::new().fg(t.text)),
                span(
                    format!(" · sealed {} ago · ledger #{}–#{}", age(now - e.sealed_at), e.seq_from, e.seq_to),
                    Style::new().fg(t.muted),
                ),
            ]));
            out.push(Line::from(vec![
                span("  ", Style::default()),
                span(sig, Style::new().fg(hue)),
                span(
                    if e.anchored {
                        " · covered by a signed ledger checkpoint"
                    } else {
                        " · no signed ledger checkpoint yet"
                    },
                    Style::new().fg(t.muted),
                ),
            ]));
        }
        None => out.push(Line::from(span(
            "  sealed when the work is delivered: the bundle, its ledger range and signature show here",
            Style::new().fg(t.muted),
        ))),
    }
    if let Some(d) = &o.diff {
        out.push(Line::from(vec![
            span("  diff ", Style::new().fg(t.muted)),
            span(format!("{} files ", d.files), Style::new().fg(t.text)),
            span(format!("+{}", d.insertions), Style::new().fg(t.ok)),
            span(" ", Style::default()),
            span(format!("-{}", d.deletions), Style::new().fg(t.error)),
        ]));
    }
    out
}

/// What landing does (or did), by the real landing model: each
/// repository's branch into the mission's integration branch, producers
/// first, all or none. Nothing of the user's own branches is touched.
fn landing(app: &App, o: &OutcomeView, w: usize) -> Lines {
    let t = &app.look.theme;
    let Some(l) = &o.landing else {
        return Vec::new();
    };
    let title = match l.mode {
        LandingMode::Preview => "WHAT LANDING DOES",
        LandingMode::Result => "WHAT LANDING DID",
    };
    let mut out = vec![heading(app, title, "", w, t.label)];
    let key_w = 13;
    let names: Vec<&str> = l.repos.iter().map(|r| r.repo.as_str()).collect();
    out.push(pair(
        t,
        "integration",
        key_w,
        vec![span(
            fit(&l.into, w.saturating_sub(key_w + 1)),
            Style::new().fg(t.title).add_modifier(Modifier::BOLD),
        )],
    ));
    if !names.is_empty() {
        let sep = format!(" {} ", arrow(app));
        out.push(pair(
            t,
            "order",
            key_w,
            vec![span(
                fit(
                    &format!("producers first: {}", names.join(&sep)),
                    w.saturating_sub(key_w + 1),
                ),
                Style::new().fg(t.text),
            )],
        ));
        out.push(pair(
            t,
            "scope",
            key_w,
            vec![span(
                fit(
                    "all or none: if one repository cannot land, the others are put back",
                    w.saturating_sub(key_w + 1),
                ),
                Style::new().fg(t.text),
            )],
        ));
    }
    if !l.queue.is_empty() {
        out.push(pair(
            t,
            "merge queue",
            key_w,
            vec![span(
                fit(&l.queue.join(" → "), w.saturating_sub(key_w + 1)),
                Style::new().fg(t.text),
            )],
        ));
    }
    out.push(pair(
        t,
        "your branches",
        key_w,
        vec![span(
            fit(
                "untouched: you merge the integration branch yourself",
                w.saturating_sub(key_w + 1),
            ),
            Style::new().fg(t.text),
        )],
    ));
    if l.mode == LandingMode::Result {
        for r in &l.repos {
            let (m, hue, word) = match r.state {
                LandState::Landed => (Mark::Pass, t.ok, "landed"),
                LandState::Failed => (Mark::Fail, t.error, "could not land"),
                LandState::Undone => (Mark::Flag, t.warn, "put back"),
                LandState::Skipped => (Mark::Idle, t.dim, "skipped"),
                LandState::Waiting => (Mark::Idle, t.dim, "waiting"),
            };
            let mut text = format!("{:<14} {word}", fit(&r.repo, 14));
            if let Some(c) = &r.commit {
                text.push_str(&format!(" {}", c.chars().take(8).collect::<String>()));
            }
            if let Some(n) = &r.note {
                text.push_str(&format!(" · {n}"));
            }
            out.push(Line::from(vec![
                span(format!("  {} ", mark(app, m)), Style::new().fg(hue)),
                span(fit(&text, w.saturating_sub(4)), Style::new().fg(t.text)),
            ]));
        }
    } else {
        out.push(Line::from(span(
            format!("  {}", fit(&l.summary, w.saturating_sub(2))),
            Style::new().fg(t.dim),
        )));
    }
    out
}

/// The gate's stages in a line or two: each one that failed or was
/// flagged on its own row, then how many passed and which did not run.
fn stages_compact(app: &App, stages: &[StageView], w: usize) -> Lines {
    let t = &app.look.theme;
    let mut out: Lines = Vec::new();
    for s in stages
        .iter()
        .filter(|s| matches!(s.state, StageState::Failed | StageState::Flagged))
    {
        out.extend(stage_lines(app, s, w, false));
    }
    let passed = stages
        .iter()
        .filter(|s| s.state == StageState::Passed)
        .count();
    let not_run: Vec<&str> = stages
        .iter()
        .filter(|s| s.state == StageState::NotRun)
        .map(|s| stage_name(s.key))
        .collect();
    let mut spans = vec![span(
        format!("  {} ", mark(app, Mark::Ready)),
        Style::new().fg(t.ok),
    )];
    spans.push(span(
        format!("{passed} of {} gate stages passed", stages.len()),
        Style::new().fg(t.muted),
    ));
    if !not_run.is_empty() {
        spans.push(span(
            format!(" · not run: {}", not_run.join(", ")),
            Style::new().fg(t.dim),
        ));
    }
    let line = Line::from(spans);
    out.push(if line.width() > w {
        Line::from(span(
            fit(
                &format!("  {passed} of {} gate stages passed", stages.len()),
                w,
            ),
            Style::new().fg(t.muted),
        ))
    } else {
        line
    });
    out
}

fn stage_name(key: StageKey) -> &'static str {
    match key {
        StageKey::ProtectedFiles => "protected pins",
        StageKey::TestWeakening => "test-weakening",
        StageKey::BaseSuite => "base suite",
        StageKey::HiddenChecks => "hidden checks",
        StageKey::EvidenceGaps => "evidence gaps",
        StageKey::Drift => "drift",
        StageKey::Review => "reviewer",
    }
}
