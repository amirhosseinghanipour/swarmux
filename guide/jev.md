# JEV (optional, bring your own key)

JEV is a hosted decision model from TypeSafe AI, called the way the
open-source [jev-layer](https://github.com/typakon4/jev-layer) project calls
it. swarmux can ask it three fuzzy questions:

- a tool call no Guard rule matches: allow or deny, with a confidence;
- "let swarmux route": which session, with a confidence (Haiku is only asked
  when JEV isn't sure);
- the [watchdog](watchdog.md) thinks an agent is stuck: carry on, change
  approach or stop.

Answers at or above `threshold` are applied and logged as `jev` with their
confidence and latency; anything else (lower confidence, no key, a timeout,
an HTTP error, an invalid reply) falls back to asking you, or to Haiku, and is
logged too. JEV is asked off the UI thread and never holds up the screen.

**What JEV may decide about a tool call.** It may deny any call it is asked
about. It may allow only calls up to `max_allow` (default `R1`: reads and
changes that stay in the worktree); a confident allow above that goes to you,
and the record says why. It is never asked about a call that can't be taken
back (a delete, a force push, a history rewrite): only your typed
confirmation allows those, however sure the model is. It is not asked about a
call with nothing to judge either, such as an MCP or unknown tool, whose
command, path or URL swarmux doesn't know. Calls held by a protected path or a
knowledge rule are yours alone, as before. JEV only ever decides the one call
in front of it; it can't create a rule.

```toml
[jev]
provider = "typesafe"   # "off" (default), "typesafe" or "openrouter"
threshold = 0.85        # answers below this go to you instead
# timeout_ms = 2000     # default: 2000 for typesafe, 5000 for openrouter
# max_allow = "R1"      # "R0".."R3": the riskiest call JEV may allow; R4 is not a choice
```

`max_allow` is read line by line: a value that isn't valid is replaced by `"R1"`
and `swarmux doctor` and `swarmux guard lint` name the line.

**What is recorded.** Every decision JEV makes or hands back is in the ledger
with the provider, the model, the SHA-256 of exactly what was sent, and whether
the summary of the call was cut at 200 characters (the answer covers the whole
call, the model saw the cut text).

| Provider | Endpoint | Model | Key |
| --- | --- | --- | --- |
| `typesafe` | `https://api.typesafe.ai/v1/systemone` | `jev-1.14.0` | `TYPESAFE_API_KEY` |
| `openrouter` | `https://openrouter.ai/api/alpha/decisions` | `typesafe/jev-1.14` | `OPENROUTER_API_KEY` |

Keys are read only from those environment variables or the OS keychain,
never from `config.toml`, and are never written to the database or logs.

```sh
swarmux jev set-key typesafe   # asks for the key without echoing it, keeps it in the keychain
swarmux jev test               # one sample decision: verdict, confidence, latency
swarmux doctor                 # shows the provider and whether a key is found
```

**Privacy.** With JEV on, decision context goes to the chosen provider: the
tool name and a summary of its command, path or URL (at most 200
characters), the first 200 characters of a prompt and your session names, or
the watchdog's own one-line notes on a stuck agent.
Never file contents, diffs or folders. The first time a provider is enabled,
swarmux shows exactly this and the host, and uses JEV only after you press
`y`; the answer is remembered per provider.

