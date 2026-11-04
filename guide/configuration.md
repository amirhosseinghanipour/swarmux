# Configuration

Optional `~/.swarmux/config.toml` (these are the defaults):

```toml
max_concurrent = 3       # jobs running at the same time
min_free_ram_mb = 1536   # queued jobs wait while free memory is lower
default_model = "sonnet" # model for new sessions
```

The other settings live next to the feature they control:

| Section | Settings | Guide |
| --- | --- | --- |
| `[telemetry]`, `[pricing]` | which sources run, how often, model prices | [Telemetry](telemetry.md) |
| `theme`, `layout`, `icons`, `motion`, `[themes]`, `[[layouts]]` | look and feel | [Themes](themes.md) |
| `[guard]` | permission rules and the loop breaker | [Guard](guard.md) |
| `[jev]` | the optional hosted decision model | [JEV](jev.md) |
| `gates.toml` in a repository | the done-gate's checks and protected paths | [Done-gate](done-gate.md) |
| `[verifier]` | clean copies and hidden checks | [Verifier](verifier.md) |
| `[worktree]` | where worktrees go, port blocks | [Worktrees](worktrees.md) |
| `[governor]` | budgets, forecasts, auto-resume | [Usage governor](governor.md) |
| `[notify]` | toasts, bell and `ntfy` | [Notifications](notifications.md) |
