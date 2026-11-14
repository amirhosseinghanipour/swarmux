# Sessions: lean mode and routing

## Lean mode

Press `l` on the Sessions screen to make a session lean. Its jobs run with
`--setting-sources project,local`, so your user-level Claude Code settings
(hooks, plugins) don't run inside them, which makes jobs start much faster.
Project and local settings and swarmux's own Guard hook still apply. Lean
sessions are marked `lean` in the list.

## Let swarmux route

Press `P` on the Dashboard (instead of `p`) or `D` on the Prompts screen
(instead of `d`) and swarmux picks the session. The status bar says
`routing…`, then the chosen session and a one-line reason, and the job's log
and the guard panel record it before the prompt is queued.

Routing makes one cheap `claude -p --model haiku` call with no tools, no
session file and none of your user-level settings. It reads the first 2000
characters of the prompt and, for each session, its name, folder, model and
latest three prompts, and answers with a session and a model suggestion
(shown, not applied). The call's cost (usually well under $0.001) is shown
next to the reason. If Haiku fails or names an unknown session, the selected
session takes the prompt and swarmux says why. Routing uses your Claude plan.


## Telling an agent something

Three ways to reach the agent of a session, by when you want it to hear you:

| You want | Do |
| --- | --- |
| a **new** task in the session, as a new job | `p` on [Focus](focus.md), or the Prompts screen |
| to tell a job that is **running** something, from inside swarmux | `m` on [Work](work.md#message-a-running-agent): queued until its turn ends, or stop the turn and continue the session with it (`M` or `Tab` switches). Recorded in the [ledger](ledger-and-evidence.md#what-you-told-an-agent), and the Guard is unchanged |
| to talk to the session **live**, as in Claude Code | `t` on Focus opens it in a terminal tab (`claude --resume <id>`): take it over there. swarmux does not follow what you type in that tab |
| to answer for work that **ended** | `s` on Work: [send it back](work.md#send-back) with a note |

swarmux does not write into a running `claude`, so a message never arrives in
the middle of a turn: see what each mode promises, and what it does not, in
[Message a running agent](work.md#message-a-running-agent).
