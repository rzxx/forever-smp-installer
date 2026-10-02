# Installer releases

The existing release channel is `rzxx/forever-smp-installer`. Version comes from the workspace `Cargo.toml`; notes come from `app-release.toml`. Pack releases have their own version and repository.

1. Change source, run `./scripts/dev.ps1` for UI checks and `./scripts/test.ps1 -Handoff` for automated checks.
2. Bump the app version and update both languages of release notes. Run `./scripts/build-app.ps1`.
3. Sign the generated `installer.json` with the existing private installer key, stored outside the public checkout:

```powershell
$version = '0.1.11' # use the actual new version
$output = "dist/releases/$version"
./target/release/forever-release.exe sign-app "$output/installer.json" ../.private/installer-signing-key.txt "$output/installer.signed.json"
```

4. Review the ZIP and metadata, then create a draft release in this repository with exactly the versioned EXE, versioned ZIP, `installer.json` and `installer.signed.json`. Test that downloaded candidate before marking the draft latest.

The ZIP contains only `Forever-SMP.exe` and a short README. Never upload a directory wholesale. The `handoff-fixture` example is a test binary and must never be shipped.

Published versions are immutable: source publication does not replace the existing 0.1.10 executable. Never overwrite an existing release's assets. Keep `installer.signed.json` and versioned executable names unchanged; installed clients use those URLs. Private signing keys must be backed up and reused, not regenerated. Public trust pins in source are safe to publish.

The metadata signature authenticates updates; it is separate from Windows Authenticode signing. Windows x64 is the supported desktop target.
