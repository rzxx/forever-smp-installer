# Working on the installer

This repository owns the Rust installer, update engine and recipe CLI. The Minecraft pack source lives in `rzxx/forever-smp-releases`, usually checked out beside this repo as `../modpack`.

- UI iteration: `./scripts/dev.ps1` builds current source and runs with local metadata and an isolated profile. Use `-BuildPack` after pack changes, `-PackRoot` for another checkout, or `-BuildOnly` to compile without opening a window.
- Verification: `./scripts/test.ps1 -Handoff` runs engine/GPUI tests, clippy and real Windows replacement/rollback fixtures. It needs no private keys.
- Packaging: `./scripts/build-app.ps1 -Handoff` runs local checks and makes an unsigned candidate under `dist/releases/<app-version>`. It never publishes. In the private parent workspace, `../scripts/release.ps1 installer` also signs/verifies it; `-Upload` uploads a draft and `-Publish` publishes this installer only.
- Run verification locally. Do not add automatic GitHub Actions checks or builds. Installer releases never require a pack release or matching pack version.
- Do not test by opening an old executable from `dist/bundles` or a downloaded release. Identify the exact executable you tested.
- Preserve app/pack IDs, feature IDs, production public keys and feed URLs unless a migration is explicitly requested. App and pack versions are independent.
- `.private`, `dist` and `target` are local data. Never commit keys, invitations, live server details or player data. Public verification keys and deterministic test keys belong in source.

The engine has reusable pieces, but Forever SMP identity, branding and installer trust pins remain product-specific. Do not claim this is a fully generic installer.
