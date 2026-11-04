# Usage governor

Before a queued job starts, swarmux looks at the subscription windows (5-hour
and weekly) as the status lines and the jobs' own `rate_limit_event`s report
them, and forecasts where each is heading at the current pace:

- **Hold.** While a window is at `hold_at` or above, new jobs wait. The queue
  says why and until when (`deferred: 5h at 97%, resumes 15:58`) and they start
  by themselves once the window resets. Reviews are verification: they keep
  running until the window is actually full.
- **Downgrade.** From `downgrade_at`, or when the forecast reaches `hold_at`
  within `horizon_minutes`, work on a pricier model runs on `downgrade_to`.
- **Auto-resume.** A job a usage limit stopped is resumed in its own Claude
  session and worktree after the window resets, also across a restart of
  swarmux.
- **Budget.** Each job is told its cap and the windows in one line after its
  prompt, and runs with `--max-budget-usd` when it has a cap. A mission's job
  may spend the planning and implementation shares of its budget; the
  verification (20%) and review (10%) shares stay reserved.
- **Battery.** On battery at most `battery_concurrency` jobs run at once.

```toml
[governor]
enabled = true
hold_at = 0.97            # new jobs wait from here until the window resets
downgrade_at = 0.90       # pricier models run on downgrade_to from here
downgrade_to = "sonnet"
horizon_minutes = 30      # how far ahead a window is forecast
battery_concurrency = 2
# max_job_usd = 5.0       # --max-budget-usd for every job (missions use their own budget)
budget_line = true        # tell each job its budget
load_hold_at = 0.70       # new jobs wait while you spend more of your time than this in the inbox
```

