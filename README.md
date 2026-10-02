# Forever SMP Setup

Windows installer and updater, with English and Russian UI. This repository contains its source and the existing signed release channel.

[Download the installer](https://github.com/rzxx/forever-smp-installer/releases/latest) · [Modpack source and downloads](https://github.com/rzxx/forever-smp-releases)

Extract the ZIP into a writable folder and run `Forever-SMP.exe`. Your launcher handles Minecraft, Fabric, Java and accounts. Close Minecraft before updating. The app remembers your game folder, optional mods and personal settings; app and pack updates are reviewed together. Server access is arranged privately with the owner.

Распакуйте ZIP и запустите `Forever-SMP.exe`. Minecraft, Fabric, Java и учётные записи настраивает лаунчер. Перед обновлением закройте Minecraft. Приложение сохраняет папку игры, выбранные моды и личные настройки.

## Develop

Use Windows x64, PowerShell 7, Rust, MSVC C++ build tools and the Windows SDK. Scripts find the SDK shader compiler used by GPUI. Dependencies are pinned in `Cargo.lock`.

Clone the existing pack repository beside this checkout for local UI testing:

```powershell
git clone https://github.com/rzxx/forever-smp-installer installer
git clone https://github.com/rzxx/forever-smp-releases modpack
cd installer
./scripts/dev.ps1 -BuildPack
```

After that, run `./scripts/dev.ps1` from this repo for each installer iteration. It rebuilds and opens `target/debug/forever-smp.exe`, reads a local recipe from the pack checkout, and uses `dist/dev/profile` plus an initially empty `dist/dev/game`. No invitation or signing key is needed. Local mode disables installer self-updates. Keep game-folder selections inside disposable test folders.

Use `-BuildPack` after changing the modpack, `-PackRoot <checkout>` for another pack checkout, or `-BuildOnly` to compile without opening the app. A raw `cargo build` only compiles; it does not prepare a test profile or package a release.

```powershell
./scripts/test.ps1       # tests + lint for ordinary changes
./scripts/test.ps1 -Full # also test Windows replacement/rollback when changing the updater
./scripts/build-app.ps1  # full checks + unsigned candidate in dist/releases/<app-version>
```

Each command uses the normal `target` cache in this repository. The full Windows updater checks use a public test key and never use production signing keys. Checks and builds run locally; no GitHub Actions runner is used on pushes or PRs. [Release steps](docs/releases.md) describe the local signing/upload command. Public builds and tests never publish or deploy.

## Source

| Path | Owns |
| --- | --- |
| `app/core` | Pack planning, file transactions, signature checks, app replacement/rollback |
| `app/desktop` | GPUI wizard, preferences and Forever SMP defaults |
| `app/release` | Metadata export/signing CLI, also used by the modpack repo |
| `scripts` | Development, verification and packaging entry points |

The engine is reusable in parts; the desktop app and recipe mapping still contain Forever SMP branding, IDs and trust pins. Adapting it to another pack requires changing those deliberately. Public verification keys belong in source; private signing keys, invitations and live server data do not. Source publication keeps the existing repository names, release URLs, asset filenames and update feeds.

App version lives in workspace `Cargo.toml`, notes in `app-release.toml`. Pack versions are independent. Windows x64 is supported; macOS has not been packaged or verified. Source is MIT licensed; dependencies retain their own licenses.
