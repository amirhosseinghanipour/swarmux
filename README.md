# swarmux

**A prompt-first command center for Claude Code.**

Write, template, refine and queue prompts. Dispatch them to several Claude Code
sessions and their subagent teams. Watch every agent, model, token, decision and
review live in one terminal.

> Status: early development. Not usable yet.

## Planned

- **Prompt library** with `{{variables}}`, a real editor and one-key refinement
- **Queue & dispatch** across sessions with a concurrency cap and RAM guard
- **Live dashboard**: lead session, subagents, context, tokens, cost, timeline, log
- **Guard**: one permission policy across all sessions and an automatic loop
  breaker, settled in code in under a millisecond so the models never see it
- **JEV (optional, bring your own key)**: the hosted Jev model for fuzzy
  decisions such as unmatched tool calls and which agent should take a prompt
- **Review step**: a reviewer model checks each job's diff before it is done
- **History**: every prompt with cost and outcome; re-run or fork from any point

Single Rust binary. Windows, macOS, Linux. Works with Claude Code only.

## Try the skeleton

```sh
cargo run              # TUI: 1-5 switch screens, q quits
cargo run -- doctor    # checks Claude Code is installed
```

## Design



## License

MIT
