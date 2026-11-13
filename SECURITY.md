# Security policy

## Reporting a vulnerability

Please report security problems privately, using GitHub's private vulnerability
reporting: go to the repository's **Security** tab and choose **Report a
vulnerability**. Do not open a public issue or pull request for it.

Include what you found, the swarmux version (`swarmux --version`), your
operating system and the steps to reproduce it. You can expect an
acknowledgement within a few days. We will work with you on a fix and credit
you in the release notes if you wish.

## Supported versions

Only the latest release gets security fixes.

## What is in scope

- **Hooks and the IPC pipe.** swarmux gives each job hooks that report back to
  the running app over a local pipe. Anything that lets another user or process
  forge a decision, answer an approval, or read or inject events is in scope.
- **Self-update.** `swarmux self-update` installs a release only when its
  minisign signature verifies against the public key built into the binary.
  Bypassing that check, or making it trust another key, is in scope.
- **The ledger signing key.** The private key that signs the ledger's anchor
  checkpoints is kept in the OS keychain (or in `audit.key` where there is no
  keychain). Leaking it to logs, the ledger, the database or the network is in
  scope. swarmux holds no other secret of yours: it has no API keys of its own.
- **Guard and the done-gate.** A way for an agent to get a tool call approved
  that the policy should hold, or to pass or skip a gate it should not, is in
  scope.

## What leaves your machine

swarmux itself opens a network connection in only three cases, each of which
you start or turn on:

- `swarmux self-update` fetches a release from GitHub and installs it only
  if its minisign signature verifies;
- `[notify] ntfy`, when you set a topic URL, sends a short title and the kind
  of item (never a command, path, prompt or session name) to that URL;
- the `gh` command, where it is installed, is run for the GitHub readings
  (pull requests and CI runs) and for outcomes; what it sends is `gh`'s own.

Everything else that leaves is Claude Code's own traffic: swarmux starts
`claude` for your jobs, and for its small Haiku helpers (routing a prompt,
diagnosing a stuck agent, reading what a finished job left, linting and
planning missions), which send Claude Code the text each of them is given, to
Anthropic, under your own Claude Code login or API key. No hosted model decides your tool calls: the
Guard is code and your rules, and a call no rule matches waits for you. swarmux
sends no usage data, telemetry or crash reports anywhere.

## Out of scope

- Issues that need an attacker who already controls your account or machine.
- The Windows executable not being code-signed; this is documented. Release
  archives are signed with minisign instead.
- Problems in Claude Code itself; report those to Anthropic.
