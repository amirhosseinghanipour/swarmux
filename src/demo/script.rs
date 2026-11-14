//! The scenario, as a list of steps at fixed ticks.
//!
//! One tick is a tenth of a second of the demo. The demo's clock runs 30
//! times faster, so half a minute of watching is a quarter of an hour of
//! agent time. Every number is made up.

/// The demo's clock at tick 0, in milliseconds since the epoch: 2026-11-12
/// 02:10 UTC. Only the clock of the demo; the real time is never touched.
pub const START: i64 = 1_794_449_400_000;
/// Milliseconds of the demo clock for each tick: half a minute per second.
pub const CLOCK_MS_PER_TICK: i64 = 3_000;
/// Ticks in a second of the demo.
pub const TICKS_PER_SEC: u64 = 10;

/// What happens at a tick.
#[derive(Clone, Debug)]
pub enum Act {
    /// A prompt goes to a session; the job is known by `label` from now on.
    Dispatch {
        label: &'static str,
        session: &'static str,
        prompt: &'static str,
    },
    /// A mission goes out as a job known by `label`.
    Mission {
        label: &'static str,
        slug: String,
    },
    /// The job's `claude` reports that it started.
    Init(&'static str),
    Text(&'static str, &'static str),
    /// A tool call and what came back.
    Tool {
        label: &'static str,
        name: &'static str,
        summary: &'static str,
        ok: bool,
        preview: &'static str,
    },
    /// A token count; the numbers come from the seed.
    Usage(&'static str),
    /// A usage-window reading, reported by the job `label`.
    Limit {
        label: &'static str,
        five_hour: f64,
        seven_day: f64,
    },
    /// A tool call that reaches the Guard, which answers from its policy:
    /// refused, allowed, or held for the user.
    Call {
        label: &'static str,
        tool: &'static str,
        subject: &'static str,
    },
    /// The agent says it is done: the gate judges.
    Stop {
        label: &'static str,
        message: &'static str,
    },
    /// The operator types a message to the running job (queue mode, the
    /// default), through the same service the Work screen's line uses.
    Message {
        label: &'static str,
        text: &'static str,
    },
    /// The job that carries `label`'s message, once the session continued
    /// with it, is known by `then` from now on.
    Follow {
        label: &'static str,
        then: &'static str,
    },
    /// The job ends with this result.
    Finish {
        label: &'static str,
        cost_usd: f64,
        turns: u32,
        minutes: u64,
        text: &'static str,
    },
}

fn at(secs: f64) -> u64 {
    (secs * TICKS_PER_SEC as f64).round() as u64
}

/// A job that works steadily from `from` to `to` seconds: a token count
/// every half second, a tool call or a line of text in between.
fn steady(
    out: &mut Vec<(u64, Act)>,
    label: &'static str,
    from: f64,
    to: f64,
    tools: &[(&'static str, &'static str)],
    lines: &[&'static str],
) {
    out.push((at(from + 0.4), Act::Init(label)));
    let mut t = from + 1.0;
    let (mut n_tool, mut n_line) = (0, 0);
    let mut step = 0;
    while t < to - 0.5 {
        out.push((at(t), Act::Usage(label)));
        if step % 3 == 1 {
            let (name, summary) = tools[n_tool % tools.len()];
            n_tool += 1;
            out.push((
                at(t + 0.2),
                Act::Tool {
                    label,
                    name,
                    summary,
                    ok: true,
                    preview: "ok",
                },
            ));
        } else if step % 3 == 2 {
            out.push((at(t + 0.2), Act::Text(label, lines[n_line % lines.len()])));
            n_line += 1;
        }
        step += 1;
        t += 0.8;
    }
}

/// The whole scenario, in order of tick.
#[allow(clippy::vec_init_then_push)]
pub fn timeline(mission: &str) -> Vec<(u64, Act)> {
    let mut v: Vec<(u64, Act)> = Vec::new();

    // Four agents on prompts, and the mission.
    v.push((
        at(0.3),
        Act::Dispatch {
            label: "data-pipeline",
            session: "data-pipeline",
            prompt: "Backfill the events table from the March export and check the row counts",
        },
    ));
    v.push((
        at(0.5),
        Act::Dispatch {
            label: "mobile-app",
            session: "mobile-app",
            prompt: "Update the push notification module and fix the Android channel crash",
        },
    ));
    v.push((
        at(0.7),
        Act::Dispatch {
            label: "docs-site",
            session: "docs-site",
            prompt: "Regenerate the API reference and fix the broken links",
        },
    ));
    v.push((
        at(0.9),
        Act::Dispatch {
            label: "web-checkout",
            session: "web-checkout",
            prompt: "Add retry with backoff to the payment form submit",
        },
    ));
    v.push((
        at(1.1),
        Act::Mission {
            label: "api-gateway",
            slug: mission.to_string(),
        },
    ));

    steady(
        &mut v,
        "data-pipeline",
        1.2,
        27.0,
        &[
            ("Read", "etl/backfill.py"),
            ("Bash", "python etl/backfill.py --dry-run"),
            ("Grep", "events_raw"),
            ("Edit", "etl/backfill.py"),
            ("Bash", "python etl/check_counts.py"),
        ],
        &[
            "The March export has 2.1M rows; loading in batches.",
            "Batch sizes look fine, checking the counts next.",
            "Row counts match the source.",
        ],
    );
    steady(
        &mut v,
        "mobile-app",
        1.4,
        29.0,
        &[
            ("Read", "android/app/src/Push.kt"),
            ("Grep", "NotificationChannel"),
            ("Edit", "android/app/src/Push.kt"),
            ("Bash", "./gradlew :app:testDebug"),
        ],
        &[
            "The crash comes from a channel created before the permission.",
            "Moving the channel setup after the permission check.",
        ],
    );
    steady(
        &mut v,
        "docs-site",
        1.6,
        9.0,
        &[
            ("Bash", "npm run docs:build"),
            ("Grep", "href=\"/api"),
            ("Edit", "docs/api/index.md"),
            ("Read", "docs/api/auth.md"),
        ],
        &[
            "Found 14 broken links, all under /api.",
            "Regenerated the reference pages.",
        ],
    );
    steady(
        &mut v,
        "web-checkout",
        1.8,
        23.0,
        &[
            ("Read", "src/checkout/submit.ts"),
            ("Grep", "retry"),
            ("Edit", "src/checkout/submit.ts"),
            ("Bash", "npm test -- submit"),
        ],
        &[
            "Adding exponential backoff with jitter.",
            "The submit handler needs a cap on retries.",
        ],
    );

    // The mission: a premature "done" the gate sends back, then a pass.
    v.push((at(2.4), Act::Init("api-gateway")));
    v.push((
        at(2.8),
        Act::Text(
            "api-gateway",
            "Reading the invoice module and the finance spec.",
        ),
    ));
    v.push((
        at(3.4),
        Act::Tool {
            label: "api-gateway",
            name: "Read",
            summary: "src/invoice.rs",
            ok: true,
            preview: "ok",
        },
    ));
    v.push((at(3.6), Act::Usage("api-gateway")));
    v.push((
        at(4.6),
        Act::Tool {
            label: "api-gateway",
            name: "Edit",
            summary: "src/invoice.rs",
            ok: true,
            preview: "ok",
        },
    ));
    v.push((at(5.0), Act::Usage("api-gateway")));
    v.push((
        at(6.4),
        Act::Tool {
            label: "api-gateway",
            name: "Bash",
            summary: "cargo build",
            ok: true,
            preview: "Finished dev profile",
        },
    ));
    v.push((
        at(9.2),
        Act::Text(
            "api-gateway",
            "Rounding now goes half up. That should be everything, I am done.",
        ),
    ));
    v.push((
        at(9.8),
        Act::Stop {
            label: "api-gateway",
            message: "Rounding now goes half up. That should be everything, I am done.",
        },
    ));
    v.push((
        at(10.8),
        Act::Text(
            "api-gateway",
            "The gate ran the tests and one fails: invoice::rounds_half_up. Fixing it.",
        ),
    ));
    v.push((
        at(11.6),
        Act::Tool {
            label: "api-gateway",
            name: "Edit",
            summary: "src/invoice.rs",
            ok: true,
            preview: "ok",
        },
    ));
    v.push((at(12.0), Act::Usage("api-gateway")));
    v.push((
        at(13.4),
        Act::Tool {
            label: "api-gateway",
            name: "Bash",
            summary: "cargo test",
            ok: true,
            preview: "42 passed",
        },
    ));
    v.push((at(14.2), Act::Text("api-gateway", "All 42 tests pass now.")));
    v.push((
        at(14.6),
        Act::Stop {
            label: "api-gateway",
            message: "All 42 tests pass now.",
        },
    ));
    v.push((
        at(15.2),
        Act::Finish {
            label: "api-gateway",
            cost_usd: 1.84,
            turns: 14,
            minutes: 38,
            text: "Invoice totals round half up; the suite passes.",
        },
    ));

    // The Guard refuses an edit to a protected test, and the agent presses on.
    v.push((
        at(6.0),
        Act::Text(
            "web-checkout",
            "The retry test is in the way. I will edit its expectations.",
        ),
    ));
    v.push((
        at(6.4),
        Act::Call {
            label: "web-checkout",
            tool: "Edit",
            subject: "tests/checkout_oracle.rs",
        },
    ));
    v.push((
        at(6.5),
        Act::Tool {
            label: "web-checkout",
            name: "Edit",
            summary: "tests/checkout_oracle.rs",
            ok: false,
            preview: "denied by the Guard: protected path tests/** (gate)",
        },
    ));

    // The agent says what it will do next. The console quotes this line, as
    // its own words, beside the call it is about to make.
    v.push((
        at(7.2),
        Act::Text(
            "web-checkout",
            "Refused. I will push the branch with the test as it was.",
        ),
    ));

    // Refused, the agent pushes the branch with the test as it was: a push is
    // irreversible, so it is held for the user as a P1.
    v.push((
        at(7.4),
        Act::Call {
            label: "web-checkout",
            tool: "Bash",
            subject: "git push origin HEAD:main",
        },
    ));

    // The docs agent runs `cargo check` on its crates: the owner's standing
    // rule allows it, and the Guard lets these through without a word.
    for (i, subject) in [
        "cargo check -p docs-core",
        "cargo check -p docs-theme",
        "cargo check -p docs-search",
        "cargo check -p docs-api",
        "cargo check -p docs-nav",
        "cargo check -p docs-i18n",
        "cargo check -p docs-assets",
    ]
    .into_iter()
    .enumerate()
    {
        v.push((
            at(3.0 + 0.8 * i as f64),
            Act::Call {
                label: "docs-site",
                tool: "Bash",
                subject,
            },
        ));
    }

    // It says it is done; the gate builds the site and finds broken links, and
    // sends it back. The agent then asks for two things no rule covers, and
    // waits for the user: a held call keeps the agent where it is.
    v.push((
        at(8.8),
        Act::Stop {
            label: "docs-site",
            message: "Reference regenerated, 14 links fixed.",
        },
    ));
    v.push((
        at(9.6),
        Act::Text(
            "docs-site",
            "Three links under /api/auth still break. I need a link checker.",
        ),
    ));
    for (i, subject) in [
        "npm install --save-dev markdown-link-check",
        "cargo install --locked mdbook-linkcheck",
    ]
    .into_iter()
    .enumerate()
    {
        v.push((
            at(10.0 + 0.4 * i as f64),
            Act::Call {
                label: "docs-site",
                tool: "Bash",
                subject,
            },
        ));
    }

    // Usage windows creeping up.
    for (i, (five, week)) in [
        (0.31, 0.12),
        (0.38, 0.13),
        (0.44, 0.14),
        (0.51, 0.15),
        (0.57, 0.16),
        (0.63, 0.17),
    ]
    .into_iter()
    .enumerate()
    {
        v.push((
            at(2.0 + 5.0 * i as f64),
            Act::Limit {
                label: "data-pipeline",
                five_hour: five,
                seven_day: week,
            },
        ));
    }

    // The operator tells the checkout agent something while it works (it is
    // not a task of the release graph, so it can be messaged). The message
    // waits for the end of its turn; the session then continues with it,
    // through the same service the Work screen uses, and the new turn
    // confirms it.
    v.push((
        at(9.0),
        Act::Message {
            label: "web-checkout",
            text: "Also keep the retry count in the settings file, not hard-coded.",
        },
    ));
    v.push((
        at(23.8),
        Act::Follow {
            label: "web-checkout",
            then: "web-checkout-2",
        },
    ));
    v.push((at(24.2), Act::Init("web-checkout-2")));
    v.push((
        at(24.8),
        Act::Text(
            "web-checkout-2",
            "Moved the retry count into the settings file, as you asked.",
        ),
    ));
    v.push((
        at(25.2),
        Act::Stop {
            label: "web-checkout-2",
            message: "Moved the retry count into the settings file, as you asked.",
        },
    ));
    v.push((
        at(25.6),
        Act::Finish {
            label: "web-checkout-2",
            cost_usd: 0.11,
            turns: 3,
            minutes: 2,
            text: "Retry count now comes from the settings file.",
        },
    ));

    // The rest say they are done, pass their gates and finish one after
    // another.
    for (secs, label, cost, turns, minutes, text) in [
        (
            23.0,
            "web-checkout",
            1.12,
            11,
            24,
            "Submit retries with backoff; the Guard kept the tests as they were.",
        ),
        (
            27.0,
            "data-pipeline",
            3.47,
            32,
            71,
            "Backfill done; row counts match the source.",
        ),
        (
            29.0,
            "mobile-app",
            2.05,
            19,
            46,
            "Channel setup moved after the permission check.",
        ),
    ] {
        v.push((
            at(secs - 0.5),
            Act::Stop {
                label,
                message: text,
            },
        ));
        v.push((
            at(secs),
            Act::Finish {
                label,
                cost_usd: cost,
                turns,
                minutes,
                text,
            },
        ));
    }

    v.sort_by_key(|(tick, _)| *tick);
    v
}
