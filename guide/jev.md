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

```toml
[jev]
provider = "typesafe"   # "off" (default), "typesafe" or "openrouter"
threshold = 0.85        # answers below this go to you instead
# timeout_ms = 2000     # default: 2000 for typesafe, 5000 for openrouter
```

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

