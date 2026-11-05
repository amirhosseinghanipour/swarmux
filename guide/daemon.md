# The background service

swarmux keeps working when its window is closed. The jobs, the Guard, the
done-gate, missions and their task graphs, the watchdog, the usage governor,
the Board and notifications all run in a background service, one per user
(per data folder). The window you open with `swarmux` attaches to it, the way
`tmux attach` does, and closing the window leaves the work running.

```sh
swarmux                    # attach; starts the service first when it isn't running
swarmux daemon status      # pid, version, uptime, jobs and windows attached
swarmux daemon stop        # stop it; running jobs resume at the next start
swarmux daemon stop --wait # start nothing new, let running jobs finish, then stop
swarmux daemon start       # start it without opening a window
swarmux daemon run         # run it in this terminal, to watch it (Ctrl+C stops it)
```

## Leaving and coming back

`q` in an attached window detaches: the window closes and the work goes on.
swarmux says what keeps going, for example
`2 jobs keep running in the background · swarmux daemon stop to stop them`.
The status bar shows `q detach` instead of `q quit` while you are attached.

Run `swarmux` again, in any terminal, to see where things are. Everything that
happened in between is there: finished jobs on the Queue and Ledger screens,
the audit ledger, the digest of what happened while you were away.

You can attach more than one window at once. They all show the same screen,
as tmux does: an approval you answer in one is gone from the others, and a
prompt typed in one shows in all of them. Each window draws at its own size
and in the colours its terminal supports. New sessions start in the folder of
the window you last attached from.

## While no window is attached

Approvals and Board questions wait in the inbox with their usual timeouts
(`approval_timeout_secs` for the Guard). [Notifications](notifications.md)
still go out, so you know to attach: the Windows toast and ntfy work as
always, and the terminal sequences (`osc777`, `osc9`, the bell) are written
to the windows attached, or left out when there are none. With no window
attached you count as away, so P2 items notify too (outside quiet hours).

## Stopping and restarting

`swarmux daemon stop` stops at once: nothing new starts, running jobs are
stopped and marked to resume, and the service exits. When it starts again
(with `swarmux` or `swarmux daemon start`), those jobs continue in their own
Claude sessions and worktrees, so at most the step under way is lost.
Missions carry on where they were, task graphs included: see
[after a restart](missions.md#task-graphs). Queued jobs stay queued.

`swarmux daemon stop --wait` lets the running jobs finish first and returns at
once; the service starts nothing new meanwhile and exits when the last one is
done. Ctrl+C on `swarmux daemon run`, and on Linux and macOS a plain `kill`,
stop the service the same way as `daemon stop`.

If the service crashes, swarmux stops following the jobs it ran. On Windows
they end with it; on Linux and macOS an agent may run on until it finishes by
itself. At the next start those jobs show as interrupted: missions pick theirs
up again by themselves, and you can run a plain job again from the Queue
screen (`r`).

## Versions

A window and the service must be the same swarmux version. After an update,
`swarmux` tells you when the service still runs the old one. When nothing is
running it offers to restart the service with the new version; otherwise it
tells you to let the jobs finish (or stop them with `swarmux daemon stop`,
from which they resume) and run `swarmux` again.

## One service per data folder

The service holds a lock in the data folder (`~/.swarmux/daemon.lock`) for as
long as it runs, and writes its pid beside it (`daemon.pid`). A second
`swarmux daemon start` says the service already runs. The lock belongs to the
running process, so a crash leaves none behind: the next start takes over.
`swarmux doctor` reports whether the service runs, its version and whether it
answers.

`swarmux --no-daemon` runs everything in the window, as before the service
existed: quitting stops its jobs. It refuses to start while a service runs
on the same data folder, and the other way round, so two engines never share
one database.

`swarmux --demo` never uses the service: it plays in-process, in a throwaway
folder.

## Where it lives

| What | Where |
| --- | --- |
| Log | `~/.swarmux/daemon.log`, moved to `daemon.log.1` past 4 MB |
| Lock and pid | `~/.swarmux/daemon.lock`, `~/.swarmux/daemon.pid` |
| Connection, Windows | a named pipe, `\\.\pipe\swarmux-<user>-<id>`, that only your user may open and that refuses remote clients |
| Connection, Linux and macOS | `~/.swarmux/daemon.sock`, mode `0600` |

The connection is local only: there is no network listener. On Windows the
service starts without a console, in a process group of its own and outside
any job object of the terminal that started it, so closing the terminal ends
neither the service nor its agents. On Linux and macOS it starts a session of
its own, so the terminal's hang-up never reaches it.

Starting the service when you log in is not set up by swarmux; `swarmux`
starts it the first time you open a window.
