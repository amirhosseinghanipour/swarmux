//! The right side of the Ledger screen: the selected run's bundle, its
//! verification, then the job itself (prompt, result, review, error).

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use swarmux_application::{JobDetail, Stored};
use swarmux_domain::evidence::VerifyReport;
use swarmux_domain::{ReviewState, ReviewVerdict, iso_minute};

use super::{fit, g, label_color, model_mix, panel};
use crate::app::App;
use crate::theme::Theme;
use crate::ui::{short, status_span};

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.look.theme;
    let Some(d) = app.ledger.detail.as_ref() else {
        frame.render_widget(panel(t, " run ".into()), area);
        return;
    };
    let s = &d.summary;
    let mut lines = Vec::new();
    if let Some(stored) = &app.ledger.bundle {
        lines.extend(bundle_lines(app, stored));
        lines.push(Line::raw(""));
    }
    lines.extend(job_lines(d, t));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(panel(t, format!(" job #{} ", s.job_id))),
        area,
    );
}

fn heading(t: &Theme, text: &str) -> Line<'static> {
    Line::styled(text.to_string(), Style::new().fg(t.lead))
}

/// `name  value` with the name in the label colour.
fn field(t: &Theme, name: &str, value: Vec<Span<'static>>) -> Line<'static> {
    let mut spans = vec![Span::styled(format!("{name:<9}"), Style::new().fg(t.label))];
    spans.extend(value);
    Line::from(spans)
}

fn text(t: &Theme, s: String) -> Vec<Span<'static>> {
    vec![Span::styled(s, Style::new().fg(t.text))]
}

fn bundle_lines(app: &App, stored: &Stored) -> Vec<Line<'static>> {
    let t = &app.look.theme;
    let b = &stored.bundle;
    let muted = Style::new().fg(t.muted);
    let mut out = vec![heading(t, "evidence bundle")];
    let signed = match b.signature_ok() {
        Ok(true) => Span::styled(
            format!(" {} signed (hash only)", g(app, "✓", "+")),
            Style::new().fg(t.ok),
        ),
        Ok(false) => Span::styled(" edited since sealing", Style::new().fg(t.error)),
        Err(_) => Span::styled(" unsigned", Style::new().fg(t.warn)),
    };
    out.push(field(
        t,
        "digest",
        vec![
            Span::styled(short_hash(&stored.digest, 16), Style::new().fg(t.text)),
            signed,
        ],
    ));
    let branch = b.branch.clone().unwrap_or_else(|| "detached".into());
    out.push(field(
        t,
        "head",
        text(
            t,
            format!(
                "{} on {}{}",
                short_hash(&b.head_sha, 12),
                fit(&branch, 28),
                if b.dirty { " (uncommitted files)" } else { "" }
            ),
        ),
    ));
    out.push(field(
        t,
        "diff",
        text(
            t,
            format!(
                "{} file(s) +{} -{}",
                b.diff.files, b.diff.insertions, b.diff.deletions
            ),
        ),
    ));
    for c in &b.checks {
        let (mark, color) = if c.passed {
            (g(app, "✓", "+"), t.ok)
        } else {
            (g(app, "✗", "x"), t.error)
        };
        out.push(field(
            t,
            "check",
            vec![
                Span::styled(format!("{mark} "), Style::new().fg(color)),
                Span::styled(fit(&c.command_line(), 34), Style::new().fg(t.text)),
                Span::styled(
                    format!(
                        " {:.1}s{}",
                        c.duration_ms as f64 / 1000.0,
                        if c.flaky { " flaky" } else { "" }
                    ),
                    muted,
                ),
            ],
        ));
    }
    let blocks = b.blocks();
    let seqs: Vec<String> = b.gates.iter().map(|g| format!("#{}", g.seq)).collect();
    out.push(field(
        t,
        "gate",
        text(
            t,
            format!("{} block(s), then pass  {}", blocks, seqs.join(" ")),
        ),
    ));
    if let Some(r) = &b.review {
        out.push(field(
            t,
            "review",
            text(t, format!("{} · {} concern(s)", r.verdict, r.concerns)),
        ));
    }
    let mix: Vec<String> = b
        .cost
        .by_model
        .iter()
        .map(|(m, usd)| format!("{} ${usd:.2}", model_mix(&[(m.clone(), *usd)])))
        .collect();
    out.push(field(
        t,
        "cost",
        text(
            t,
            format!(
                "${:.2} ({}) {}",
                b.cost.total_usd,
                b.cost.basis,
                mix.join(" · ")
            ),
        ),
    ));
    out.push(field(
        t,
        "ledger",
        text(
            t,
            format!(
                "#{} to #{}  head {}",
                b.ledger.from_seq,
                b.ledger.to_seq,
                short_hash(&b.ledger.head_hash, 10)
            ),
        ),
    ));
    if !b.checkpoints.is_empty() {
        out.push(field(
            t,
            "points",
            text(t, format!("{} checkpoint(s)", b.checkpoints.len())),
        ));
    }
    for l in &stored.labels {
        let when = iso_minute(l.at);
        out.push(field(
            t,
            "label",
            vec![
                Span::styled(
                    format!("{} #{}", l.kind.as_str().replace('_', " "), l.pr),
                    Style::new().fg(label_color(t, l.kind)),
                ),
                Span::styled(format!("  {when}"), muted),
            ],
        ));
    }
    out.extend(verify_lines(app, &b.contract.slug));
    out
}

fn short_hash(hash: &str, n: usize) -> String {
    hash.chars().take(n).collect()
}

/// The verification: how to ask, that it runs, or what it found.
fn verify_lines(app: &App, slug: &str) -> Vec<Line<'static>> {
    let t = &app.look.theme;
    let Some(evidence) = app.evidence() else {
        return Vec::new();
    };
    if evidence.verifying(slug) {
        return vec![field(
            t,
            "verify",
            vec![Span::styled(
                "running the checks in a clean checkout…",
                Style::new().fg(t.warn),
            )],
        )];
    }
    let Some(report) = evidence.report(slug) else {
        return vec![field(
            t,
            "verify",
            vec![Span::styled(
                "v re-runs the checks at head in a clean checkout",
                Style::new().fg(t.muted),
            )],
        )];
    };
    report_lines(app, &report)
}

fn report_lines(app: &App, report: &VerifyReport) -> Vec<Line<'static>> {
    let t = &app.look.theme;
    let (mark, color) = if report.matches() {
        (g(app, "✓", "+"), t.ok)
    } else {
        (g(app, "✗", "x"), t.error)
    };
    let mut out = vec![field(
        t,
        "verify",
        vec![Span::styled(
            format!("{mark} {}", report.verdict()),
            Style::new().fg(color).bold(),
        )],
    )];
    let ok = report.items.iter().filter(|i| i.ok).count();
    out.push(Line::styled(
        format!("          {ok} of {} items hold", report.items.len()),
        Style::new().fg(t.muted),
    ));
    for i in report.mismatches() {
        out.push(Line::styled(
            format!(
                "          {} {}: expected {}, found {}",
                g(app, "✗", "x"),
                i.subject,
                i.expected,
                i.found
            ),
            Style::new().fg(t.error),
        ));
    }
    out
}

fn job_lines(d: &JobDetail, t: &Theme) -> Vec<Line<'static>> {
    let s = &d.summary;
    let duration = s
        .duration_ms
        .map_or_else(String::new, |ms| format!(" · {:.1}s", ms as f64 / 1000.0));
    let mut lines = vec![
        Line::from(vec![
            Span::raw(format!("{} · {} · ", s.session_name, s.model)),
            status_span(t, s.status),
        ]),
        Line::styled(
            format!(
                "{} UTC{duration} · ${:.4} · in {} / out {} · cache {}",
                iso_minute(s.created_at),
                s.cost_usd,
                short(s.tokens_in),
                short(s.tokens_out),
                s.cache_ratio
                    .map_or_else(|| "—".to_string(), |c| format!("{:.0}%", c * 100.0))
            ),
            Style::new().fg(t.muted),
        ),
        Line::raw(""),
        heading(t, "prompt"),
    ];
    lines.extend(s.prompt.lines().map(|l| Line::raw(l.to_string())));
    if let Some(text) = d.result.as_ref().map(|r| r.text.as_str())
        && !text.is_empty()
    {
        lines.push(Line::raw(""));
        lines.push(heading(t, "result"));
        lines.extend(text.lines().map(|l| Line::raw(l.to_string())));
    }
    if let Some(review) = &d.review {
        lines.push(Line::raw(""));
        lines.push(heading(t, "review"));
        lines.extend(review_lines(review, t));
    }
    if let Some(e) = &s.error {
        lines.push(Line::raw(""));
        lines.push(Line::styled("error", Style::new().fg(t.error)));
        lines.extend(e.lines().map(|l| Line::raw(l.to_string())));
    }
    lines
}

/// The review's outcome, its concerns and what it cost.
fn review_lines(review: &ReviewState, t: &Theme) -> Vec<Line<'static>> {
    let cost = |c: f64| Span::styled(format!(" · ${c:.4}"), Style::new().fg(t.muted));
    match review {
        ReviewState::Requested => vec![Line::styled("requested", Style::new().fg(t.muted))],
        ReviewState::Running { job } => vec![Line::styled(
            format!("running as job #{job}"),
            Style::new().fg(t.muted),
        )],
        ReviewState::Done {
            review,
            job,
            cost_usd,
        } => {
            let (text, color) = match review.verdict {
                ReviewVerdict::Pass => ("passed", t.ok),
                ReviewVerdict::Concerns => ("concerns", t.error),
            };
            let mut lines = vec![Line::from(vec![
                Span::styled(text, Style::new().fg(color)),
                Span::raw(format!(
                    " · {} diffs checked · job #{job}",
                    review.diffs_checked
                )),
                cost(*cost_usd),
            ])];
            lines.extend(
                review
                    .concerns
                    .iter()
                    .map(|c| Line::styled(format!("- {c}"), Style::new().fg(t.error))),
            );
            lines
        }
        ReviewState::Failed {
            error, cost_usd, ..
        } => vec![Line::from(vec![
            Span::styled(format!("review failed: {error}"), Style::new().fg(t.error)),
            cost(*cost_usd),
        ])],
        ReviewState::Skipped { why } => {
            vec![Line::styled(
                format!("skipped: {why}"),
                Style::new().fg(t.muted),
            )]
        }
    }
}
