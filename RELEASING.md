# Releasing swarmux

A release is a version tag. Pushing `vX.Y.Z` runs
`.github/workflows/release.yml`, which builds the archives for Windows
(x86_64-msvc), macOS (aarch64 and x86_64) and Linux (x86_64 gnu), signs each
one with minisign, checks the signatures against the public key in the
repository and publishes a GitHub release.

Windows executables are not code-signed. Windows SmartScreen may warn on the
first run; the minisign signature is what `swarmux self-update` checks.

## One-time setup: the signing key

Install [minisign](https://jedisct1.github.io/minisign/) and make a key pair
on a machine you trust:

```sh
minisign -G -p swarmux.pub -s swarmux.key
```

Choose a password. Then:

1. Replace the contents of `crates/infrastructure/src/dist/minisign.pub` with
   the contents of `swarmux.pub` (two lines) and commit it. The key is built
   into the binary; `swarmux self-update` trusts only signatures made by it.
2. In the repository settings, under Secrets and variables, Actions, add:
   - `MINISIGN_SECRET_KEY`: the whole contents of `swarmux.key`
   - `MINISIGN_PASSWORD`: the password you chose
3. Keep `swarmux.key` offline and backed up. Never commit it. If it is lost
   or leaks, make a new pair, ship a release with the new public key and tell
   users to reinstall by hand: builds carrying the old key cannot trust the
   new one.

Until step 1 is done, a build has no usable key: `self-update` refuses to
install anything, and the release workflow fails at its signature check.

## Cutting a release

1. Set `version` in `[workspace.package]` of `Cargo.toml`, commit, and push.
2. `git tag vX.Y.Z && git push origin vX.Y.Z`. The workflow refuses a tag that
   does not match the crate version.
3. When it finishes, the release holds `swarmux-<target>.zip|tar.gz`, one
   `.minisig` per archive and `SHA256SUMS`.

## Package managers

The templates are in `packaging/`. Replace `<VERSION>` and the checksums with
the values from `SHA256SUMS`.

- **winget**: copy the three files in `packaging/winget` to
  `manifests/a/AmirhosseinGhanipour/Swarmux/<VERSION>/` in a fork of
  `microsoft/winget-pkgs` (rename them to `AmirhosseinGhanipour.Swarmux.yaml`,
  `.installer.yaml`, `.locale.en-US.yaml`) and open a pull request, or use
  `wingetcreate update`.
- **scoop**: put `packaging/scoop/swarmux.json` in a bucket repository (for
  example `scoop-swarmux`, under `bucket/`). Its `checkver`/`autoupdate`
  entries let scoop's tooling bump it.
- **Homebrew**: create a repository `homebrew-swarmux`, put
  `packaging/homebrew/swarmux.rb` at `Formula/swarmux.rb`.
- **crates.io**: `swarmux` depends on `swarmux-domain`, `swarmux-application`,
  `swarmux-infrastructure` and `swarmux-tui`; publish those four first, in that
  order, then `swarmux`:
  `cargo publish -p swarmux-domain` and so on. Run `cargo login` once with a
  crates.io token. The names were free when this was written.
