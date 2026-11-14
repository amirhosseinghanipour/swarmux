# Workspaces: several repositories worked on as one

Many products are a folder of git repositories: an app, its backend, an
admin site, a landing page. The folder itself is often not a repository,
and a change usually spans more than one of them. A workspace is that
folder: a session on it sees every repository, and a mission of it changes
the ones it names and lands in each of them, or in none.

## Adding a workspace

```sh
swarmux workspace add ~/product            # the folder's name, or --name Product
swarmux workspace list
swarmux workspace show product             # repositories, stacks and commands
swarmux workspace rescan product           # after a repository came or went
swarmux workspace rm product               # forgets it; no file is touched
```

`add` looks for git repositories in the folder itself, in its folders and
in theirs (two levels down). It does not look inside a repository, nor in
hidden folders or ones like `node_modules`, `target` and `build`. Folders
that are not repositories are left out.

For each repository it notes its default branch (`origin`'s HEAD, else the
branch checked out), what it is built with and the commands it offers:

| Found | Stack | Commands |
| --- | --- | --- |
| `pubspec.yaml` with `sdk: flutter` | flutter | `flutter analyze`, `flutter test` |
| `pubspec.yaml` | dart | `dart analyze`, `dart test` |
| `package.json` | node | its scripts, run with npm, pnpm, yarn or bun as the lockfile says |
| `Cargo.toml` | rust | `cargo build`, `cargo test` |
| `pyproject.toml`, `requirements.txt`, `setup.py` | python | `python -m pytest` when it has tests |
| `go.mod` | go | `go build ./...`, `go test ./...` |
| `supabase/config.toml` | supabase | `supabase db lint`, `supabase test db` when it has tests |
| `Gemfile` | ruby | `bundle exec rspec` or `bundle exec rake test` |
| `*.csproj`, `*.fsproj`, `*.sln` | dotnet | `dotnet build`, `dotnet test` |
| `ProjectSettings/ProjectVersion.txt` | unity | none |
| `project.godot` | godot | none |
| `Makefile`, `justfile` | | their targets and recipes |

Each repository is named after its folder (or its path, `services-api`,
when two folders share a name). A rescan keeps the names, so what refers
to a repository still does. Anything swarmux could not find shows as `—`.

## Workspace sessions

On the Sessions screen (6) press `w` and type the workspace's name. The
session works from the workspace's root. Its sessions are listed together
under a line with the workspace's repositories, and on Mission control's
wall their cards sit together and name the workspace and its repositories.

A job of a workspace session runs `claude -p` with every repository it
touches added with `--add-dir`, and with
`CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`, so Claude Code loads each
repository's `CLAUDE.md` (and `.claude/CLAUDE.md`, its rules and
`CLAUDE.local.md`) as well as the root's. The Guard's hooks and settings
reach it exactly as they reach any other job.

## Worktree sets

With worktrees on (the default for a workspace session), each job gets a
worktree set: one worktree per repository it touches, under a task folder
laid out like the workspace, so the agent sees `app/...` and `backend/...`
as in your own folder. The task folder is where the job runs. Each
repository's worktree is on its own branch `swarmux/<session>/<n>`, and the
set shares one block of ports.

- Only repositories are in the set. Other folders and files at the
  workspace root are not copied, except the root's `CLAUDE.md` and
  `CLAUDE.local.md`. Files in the `[worktree] copy` list (`.env` and the
  like) are copied into each repository's worktree, as for any job.
- A checkpoint commits every repository of the set.
- The set is reclaimed as a whole, the deepest worktree first, and files
  left in it are committed to their branches first.
- Without worktrees (`swarmux worktree mode <session> off`) the job runs in
  your folders at the workspace root, with the repositories added.

Which repositories a job touches: the ones its mission names in `repos`,
else all of them. Naming them is cheaper (a checkout per repository) and
keeps the others out of reach; naming none is the default and checks out
everything.

## Workspace missions

A mission written for a workspace session belongs to the workspace. Its
contract is kept in swarmux's folder (`~/.swarmux/workspaces/<id>/missions/`),
since the root is often not a repository; a repository's own missions stay
in its `.swarmux/missions/`. In the composer, `repos` names the
repositories.

```toml
title = "Order endpoint"
goal = "add the orders table and call it from the app"
repos = ["backend-supabase", "app-flutter"]   # producers first

[acceptance]
checks = ["backend-supabase: supabase db lint"]

[acceptance.by_repo]
app-flutter = ["flutter analyze", "flutter test"]
```

A check `repo: command` runs `command` in that repository's folder of the
task folder; one without a prefix runs at the task folder itself. The
`[acceptance.by_repo]` table is the same thing written by repository. A
mission that names a repository its workspace does not have is refused
when it is linted.

**Landing needs one line of configuration.** A workspace mission's checks run
in the task folder, not in a clean copy of a repository, and the verifier
does not let a mission pass on checks that ran next to what the agent left
(see [When the gate cannot vouch](verifier.md#when-the-gate-cannot-vouch)).
Without more, the gate's verdict is Escalate: nothing lands, the mission
waits in *Needs you* and says the checks ran in the worktree. To let your
workspace's missions land on their in-place checks, put this in
`.swarmux/gates.toml` at the workspace root (it is copied into each task
folder, where agents cannot write it):

```toml
[verifier]
worktree_checks = "allow"
```

The bundle and the ledger still record `ran_in = "worktree"`, so what the
checks did not rule out stays visible. A repository's own `gates.toml` does
not apply to a workspace job; the root's does.

When the job ends and its gate passes, the mission lands: in each
repository, in the order `repos` names them, its branch is merged into the
mission's branch `swarmux/<mission>/integration` (made from that
repository's HEAD; your own branches are never written). The mission is
delivered once every repository landed. If one can't take its merge (a
conflict, or git refuses), the repositories merged before it are put back,
nothing stays landed, the mission is escalated and the inbox says which
repository stopped it and why.

With `landing = "manual"` in `[missions]` (see [Configuration](configuration.md#landing))
the gate passing does not land the mission: it waits, verified, on the [Work
screen](work.md) until you land it with `l`, and then lands exactly as above,
including the undo when one repository can't take its merge. The default,
`"auto"`, lands the moment the gate passes.

## Not yet

- A workspace mission builds in one session: plans whose tasks could run
  side by side are not split into a task graph across repositories.
- Checks that run in a repository's folder run in the task folder, not in a
  clean copy (so a mission needs `worktree_checks = "allow"` at the workspace
  root to land, see above), and a repository's own `.swarmux/gates.toml` and
  hidden checks don't apply to a workspace job.
- The evidence bundle of a workspace mission records the first repository
  of its set; the landing of each is on the audit ledger.
