# Working on the installer

Work from this directory with ordinary Git and Cargo commands. This repo owns the Rust installer, update engine and recipe CLI; pack source is in the sibling `../modpack` repository.

- Run: `./scripts/dev.ps1`. Add `-BuildPack` on the first run or after pack edits. This builds current source and opens an isolated local profile; `-BuildOnly` just compiles.
- Check: `./scripts/test.ps1` runs tests and lint. Use `-Full` when changing app replacement/rollback; release builds run those Windows process checks automatically.
- Package: `./scripts/build-app.ps1` runs full checks and writes an unsigned candidate to `dist/releases/<version>`. Owner signing/upload uses `../scripts/release.ps1 installer`; see [release steps](docs/releases.md).

Run locally; do not add automatic GitHub Actions. Use current source through the dev script for UI testing. Version is in `Cargo.toml`, notes in `app-release.toml`; installer releases are independent of pack releases.

Preserve existing IDs, optional feature IDs, trust pins and feed URLs. Keep keys, invitations and live/player data out of source. The engine has reusable pieces; branding and trust pins are specific to Forever SMP.
