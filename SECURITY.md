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
- **Keychain keys.** API keys (for JEV) are read from the environment or the OS
  keychain. Leaking them to logs, the ledger, the database or the network
  beyond their provider is in scope.
- **Guard and the done-gate.** A way for an agent to get a tool call approved
  that the policy should hold, or to pass or skip a gate it should not, is in
  scope.

## Out of scope

- Issues that need an attacker who already controls your account or machine.
- The Windows executable not being code-signed; this is documented. Release
  archives are signed with minisign instead.
- Problems in Claude Code itself; report those to Anthropic.
