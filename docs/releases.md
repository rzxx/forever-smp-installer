# Installer releases

Installer releases are independent of modpack releases. Version comes from workspace `Cargo.toml`; notes come from `app-release.toml`. The existing channel is `rzxx/forever-smp-installer`.

In the owner's workspace, bump the installer version, update both languages of app notes, then run from this repository:

```powershell
../scripts/release.ps1 installer
```

That one command runs the installer tests, clippy and Windows replacement/rollback fixtures on your PC, builds the EXE/ZIP, signs `installer.json` with the existing local key, and verifies the signature against the compiled installer trust pin. Output is `installer/dist/releases/<app-version>`. It does not build or release a pack.

Add `-Upload` to build locally and upload a draft to the installer repo; add `-Publish` to build locally and publish it as latest. Upload requires clean, committed source already pushed to this repo. Bilingual notes and the four asset filenames are supplied automatically. Published versions are refused; a failed draft upload can be retried.

For an unsigned build from this public checkout alone, use `./scripts/build-app.ps1`. Full checks run automatically; no private keys are needed. Signing/upload automation stays in the owner's workspace. No GitHub Actions runner is used for checks or builds.

The release assets are the versioned EXE, versioned ZIP, `installer.json` and `installer.signed.json`. The ZIP contains only `Forever-SMP.exe` and a short README. Keep the signed feed filename and versioned executable filenames unchanged: installed clients use those URLs. Never ship the `handoff-fixture` test example, keys or invitations.

Private signing keys stay local, are backed up and reused, and are never regenerated per release. Public trust pins belong in source. Metadata signing is separate from Windows Authenticode signing. Windows x64 is the supported desktop target.
