# Telemetry

Background collectors feed the dashboard's stations. Each runs on its own
thread; a source that fails keeps its last reading and shows why. Nothing is
estimated: what isn't known shows as `—`.

This is swarmux reading your machine, not reporting on you: it sends no usage
data anywhere. The only network use is what you ask for or turn on: `ntfy`
notifications, `swarmux self-update` and the `gh` calls for GitHub (plus
Claude Code's own traffic, which swarmux starts for your jobs; see
[SECURITY.md](../SECURITY.md#what-leaves-your-machine)).

- **Spend & burn** from Claude Code's transcripts (`~/.claude/projects`, or
  `$CLAUDE_CONFIG_DIR/projects`), like `ccusage`: cost per day, week, model
  and project, five-hour billing blocks with the burn rate and a projection,
  a 12-week activity calendar and the token flow of the last hour. Only
  usage numbers, model names, timestamps and folders are read, never message
  text. Read positions are cached in `~/.swarmux/usage.db`, so after the
  first pass only new lines are read.
- **Session wall**: every Claude Code session on the machine, live ones
  (active in the last two minutes) first, swarmux's own marked, with each
  session's context use and a near-compaction flag at 80%.
- **Usage limits** from Claude Code's `rate_limit_event` (five-hour and
  weekly windows): use, reset countdown and when the current pace reaches
  the limit. The latest reading is kept across restarts.
- **Jobs**: token flow and cache hit ratio, tool calls (count, average
  time, a live ticker), most read and edited files, the agent tree with each
  agent's current tool, the latest `TodoWrite` list, and each session's MCP
  servers, plugins, skills, agents and Claude Code version.
- **Git** for the selected session's folder (branch, ahead/behind, staged,
  unstaged and untracked files, diff size, last five commits) and **GitHub**
  through `gh` when it is installed and logged in (open pull requests, the
  latest CI run of the branch).
- **System**: CPU per core with history, memory and swap, network rates,
  the home disk, battery, uptime and Claude Code processes.

```toml
[telemetry]
usage = true          # transcripts
system = true
git = true
github = true
usage_secs = 15       # how often each source is read
system_secs = 1
git_secs = 5
github_secs = 60
# transcripts_path = "D:/claude/projects"

# Prices are built in (USD per million tokens); override or add models.
# Cache writes default to 1.25x (5 min) and 2x (1 h) the input price,
# cache reads to 0.1x.
[pricing."claude-opus-5-5"]
input = 4.0
output = 20.0
cache_read = 0.20
```

