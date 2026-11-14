# Compatibility

What swarmux promises not to break, for people who write config, packs,
scripts or tools around it, and what it keeps the right to change. This page
describes what the code does now; where a promise is not enforced yet, it says
so.

## The rules

1. **Every external contract is versioned.** A file format carries
   `schema_version` (or `BUNDLE_SCHEMA` for the evidence bundle); a protocol
   carries a version in its handshake. Versions are whole numbers.
2. **Additive changes only within a major version.** Within one version a
   change may add an optional key, a new allowed word, a new field in an
   output or a new ledger payload field. It may not remove, rename or
   re-purpose one, and a default does not change the meaning of an existing
   file. A change that cannot be additive raises the version, and the old
   version keeps being read (see rule 6).
3. **Field names and ids are never reused.** A removed field is *reserved*:
   it stays out of the format for good, so an old file never means something
   new. A key that was removed is read, ignored and named in a warning (the
   `[jev]` section is the one example today).
4. **Stable or experimental.** A contract is stable unless it says it is not.
   Experimental surfaces are marked `v0-experimental` (in the JSON Schema's
   description, in the guide, in the output) or live under a `proposed.*` key;
   they may change without notice and published packs may not use them.
5. **Deprecation takes at least 12 months.** A deprecated key, command or
   field is announced in the changelog and in `swarmux doctor` and
   `swarmux config lint` (a warning, never an error, until it is removed), keeps
   working for at least 12 months, and is removed only in a release whose
   changelog says so. Compatibility shims in the code carry a tag so they can be
   found and removed on time:

   ```
   // COMPAT(<name>, <since version>, <date added>): <what it keeps working>; remove after <date + 12 months>
   ```

   `grep -rn "COMPAT(" crates src` lists them.
6. **Old integrity data is verifiable forever.** A ledger entry, a hash chain,
   a checkpoint and an evidence bundle written by any released version is read
   and verified by every later one: the ledger's hash format is frozen (see
   `crates/domain/src/ledger.rs`), the bundle reader accepts every schema from
   `BUNDLE_SCHEMA_OLDEST` to `BUNDLE_SCHEMA`, and unknown fields in a ledger
   payload are ignored, never rejected. Tests keep this true: the chain is
   verified on every kind of entry, and a version 1 bundle fixture still
   verifies. What is **not** yet in place is a fixture from *every released
   version*, because no version has been released; the first release adds one.
7. **Internal protocols are not promised.** The cell stream of "terminal
   attach", the background service's local socket protocol and the hook IPC
   between `swarmux hook` and the app are internal, change with the binary and are
   never offered to third parties.

## The contracts

| Contract | Version now | Reads | Notes |
| --- | --- | --- | --- |
| `config.toml` and the layers (`managed.toml`, workspace, `.swarmux/config.toml`) | `schema_version = 1` | 1; a newer number is a warning, unknown keys are ignored | Experimental: the layering API (which tiers exist, pack and mission tiers). The keys themselves are stable. JSON Schema in `schema/config.schema.json`, `managed.schema.json`. |
| `.swarmux/gates.toml` | `schema_version = 1` | 1 | An unknown key or an unusable value makes the gate escalate. JSON Schema in `schema/gates.schema.json`. |
| Mission contract (`.swarmux/missions/<slug>.toml`) | `schema_version = 1` | 1, and files with none | An unknown key refuses the contract. `schema/mission.schema.json`. |
| Knowledge file (`swarmux knowledge export`) | `schema_version = 1` | any number; unknown item keys are left out | `schema/knowledge.schema.json`. |
| `swarmux keys export`, `swarmux theme export` (TOML) | `schema_version = 1` | 1 | `schema/keys.schema.json`, `schema/theme.schema.json`. |
| Evidence bundle | `BUNDLE_SCHEMA = 2` | 1 and 2 | The signature is a hash, not a public-key signature. |
| Ledger entry | the hash format is frozen | every entry ever written | Kinds are never removed; payloads only gain fields. The `Policy` kind's `event` words (`trust_granted`, `trust_revoked`, `config_loaded`, `guard_rule_added`, `guard_fallback`, `gate_selftest`, `paths_migrated`, ...) are additive. |
| Console read model (what the Work and Deck screens read) | `SCHEMA_VERSION = 1` | 1 | New enum words appear (a new kind of inbox item, `trust`); a client must tolerate words it does not know. Marked `v0-experimental` until a second consumer exists. |
| Command line | none | | Output meant for people may change. `config get`, `config export`, `keys export`, `theme export`, `gate lint`'s exit code and the exit codes of `lint` commands are scripting surfaces and follow rules 2 and 5. |
| Environment | | | `SWARMUX_CONFIG_DIR`, `SWARMUX_STATE_DIR`, `SWARMUX_MANAGED_FILE`, `SWARMUX_WORKSPACE`, `SWARMUX_CLAUDE_BIN` and `SWARMUX_<SECTION>__<KEY>` are stable. |
| Folders | | | An existing `~/.swarmux` is read in place for ever; this is a permanent shim (`COMPAT(legacy-home, 0.1.0, 2026-11-14)` in `paths.rs`), not a deprecation. |

## Extension surfaces

Not yet stable, and said so: practice packs, third-party deciders, adapters,
exporters and the daemon's data API do not exist as contracts yet. What exists
for them is the key registry (`crates/infrastructure/src/config_registry.rs`):
a pack registers its settings by building a `KeyDef` (type, default, merge kind,
grants or restricts, which tiers may set it) and the layering, `lint`, `explain`,
environment overrides and JSON Schema treat them like any other key. That
registry and the tier list are **experimental** until the first pack ships.

## When something has to break

A change that cannot be made additively raises the version of the contract it
changes, ships the reader for the old one, says so under *Changed* in the
changelog and says what to do. A security fix may break a contract at once if
the alternative is leaving a hole open; the changelog then says so plainly.
