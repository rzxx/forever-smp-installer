use crate::*;
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

const STATE: &str = ".forever-smp/state.json";
const JOURNAL: &str = ".forever-smp/pending/journal.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub install: Vec<ModFile>,
    pub remove: Vec<ModFile>,
    pub presets: Vec<Preset>,
    pub conflicts: Vec<String>,
    pub extras: Vec<String>,
    pub previous: Option<State>,
    pub choices: BTreeMap<String, bool>,
    pub recommended: bool,
    pub release: Release,
}

pub(crate) fn safe(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_path(relative, "")?;
    let mut current = root.to_path_buf();
    for part in relative.split('/') {
        current.push(part);
        if let Ok(metadata) = fs::symlink_metadata(&current) {
            ensure!(
                !metadata.file_type().is_symlink(),
                "Symbolic link in managed path: {}",
                current.display()
            );
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                ensure!(
                    metadata.file_attributes() & 0x400 == 0,
                    "Reparse point in managed path: {}",
                    current.display()
                );
            }
            ensure!(
                current.canonicalize()?.starts_with(root),
                "Managed path escapes game directory"
            );
        }
    }
    Ok(current)
}

fn root_path(root: &Path) -> Result<PathBuf> {
    ensure!(root.is_dir(), "Select an existing Minecraft game folder");
    let root = root.canonicalize()?;
    ensure!(
        root.parent().is_some(),
        "Do not use a filesystem root as the game folder"
    );
    Ok(root)
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }
    ensure!(path.is_file(), "Expected file: {}", path.display());
    Ok(Some(fs::read(path)?))
}

pub(crate) fn write_sync(path: &Path, content: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("File has no parent")?)?;
    let temporary = path.with_file_name(format!(".write-{}", uuid::Uuid::new_v4()));
    let mut f = File::create(&temporary)?;
    f.write_all(content)?;
    f.sync_all()?;
    drop(f);
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            let _ = fs::remove_file(temporary);
            return Err(error.into());
        }
    }
    #[cfg(not(windows))]
    {
        fs::rename(temporary, path)?;
        File::open(path.parent().unwrap())?.sync_all()?;
    }
    Ok(())
}

pub fn load_state(root: &Path, release: &Release) -> Result<Option<State>> {
    let root = root_path(root)?;
    if let Some(bytes) = read_bytes(&safe(&root, STATE)?)? {
        let state: State = serde_json::from_slice(&bytes)?;
        validate_state(&state)?;
        return Ok(Some(state));
    }
    // Adopt only files explicitly recorded by our previous CMD installer.
    let old = safe(&root, "forever-smp-install.json")?;
    if !old.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(old)?;
    let text = std::str::from_utf8(&bytes)?.trim_start_matches('\u{feff}');
    let legacy: serde_json::Value = serde_json::from_str(text)?;
    ensure!(
        legacy["side"] == "client",
        "This is not a client installation"
    );
    let mut mods = vec![];
    for item in legacy["mods"]
        .as_array()
        .context("Invalid old install receipt")?
    {
        let path = item["path"].as_str().context("Missing path")?.to_owned();
        validate_path(&path, "mods/")?;
        let hash = item["sha512"].as_str().context("Missing hash")?.to_owned();
        mods.push(ModFile {
            id: path.clone(),
            path,
            sha512: hash,
            size: 0,
            urls: vec![],
            feature: None,
        });
    }
    let optional = legacy["optionalMods"]
        .as_array()
        .context("Missing optional choices")?;
    let choices = release
        .features
        .iter()
        .map(|feature| {
            let enabled = release
                .mods
                .iter()
                .filter(|m| m.feature.as_ref() == Some(&feature.id))
                .any(|m| {
                    optional.iter().any(|old| {
                        old.as_str().is_some_and(|path| {
                            legacy_feature(path) == Some(feature.id.as_str()) || path == m.path
                        })
                    })
                });
            (feature.id.clone(), enabled)
        })
        .collect();
    let state = State {
        schema: 1,
        pack_id: PACK_ID.into(),
        version: legacy["packVersion"]
            .as_str()
            .context("Missing pack version")?
            .into(),
        minecraft: legacy["minecraft"]
            .as_str()
            .context("Missing Minecraft version")?
            .into(),
        fabric: legacy["fabricLoader"]
            .as_str()
            .context("Missing Fabric version")?
            .into(),
        recommended: false,
        choices,
        mods,
        baselines: BTreeMap::new(),
    };
    validate_state(&state)?;
    Ok(Some(state))
}

/// Explicitly adopt a known pack release, taking ownership only of matching JARs.
pub fn adopt(root: &Path, known: &Release) -> Result<State> {
    known.validate()?;
    let root = root_path(root)?;
    let _lock = lock(&root)?;
    ensure_game_closed(&root)?;
    ensure!(
        !safe(&root, STATE)?.exists() && !safe(&root, "forever-smp-install.json")?.exists(),
        "This folder is already managed"
    );
    ensure!(
        !safe(&root, JOURNAL)?.exists(),
        "Recover the pending update first"
    );
    let mut mods = vec![];
    for file in &known.mods {
        if let Some(bytes) = read_bytes(&safe(&root, &file.path)?)? {
            ensure!(
                sha512(&bytes) == file.sha512.to_ascii_lowercase(),
                "Cannot adopt modified JAR: {}",
                file.path
            );
            mods.push(file.clone());
        }
    }
    ensure!(
        !mods.is_empty(),
        "No matching pack mods found in this folder"
    );
    let choices = known
        .features
        .iter()
        .map(|f| {
            (
                f.id.clone(),
                mods.iter().any(|m| m.feature.as_ref() == Some(&f.id)),
            )
        })
        .collect();
    let mut baselines = BTreeMap::new();
    for preset in &known.presets {
        if read_bytes(&safe(&root, &preset.path)?)?.as_deref() == Some(preset.content.as_bytes()) {
            baselines.insert(preset.path.clone(), preset.content.clone());
        }
    }
    let state = State {
        schema: 1,
        pack_id: PACK_ID.into(),
        version: known.version.clone(),
        minecraft: known.minecraft.clone(),
        fabric: known.fabric.clone(),
        recommended: false,
        choices,
        mods,
        baselines,
    };
    write_sync(&safe(&root, STATE)?, &serde_json::to_vec_pretty(&state)?)?;
    Ok(state)
}

pub fn legacy_feature(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next()?.to_ascii_lowercase();
    [
        ("iris-", "iris"),
        ("distanthorizons-", "distant-horizons"),
        ("jade-", "jade"),
        ("firstperson-", "first-person"),
        ("notenoughanimations-", "not-enough-animations"),
        ("cameraoverhaul-", "camera-overhaul"),
        ("betterstats-", "better-statistics"),
        ("tcdcommons-", "better-statistics"),
        ("clock-in-", "clock-in"),
    ]
    .into_iter()
    .find(|(prefix, _)| name.starts_with(prefix))
    .map(|(_, id)| id)
}

fn validate_state(state: &State) -> Result<()> {
    ensure!(
        state.schema == 1 && state.pack_id == PACK_ID,
        "Unknown installation state"
    );
    let mut paths = BTreeSet::new();
    for file in &state.mods {
        validate_path(&file.path, "mods/")?;
        ensure!(
            file.path.ends_with(".jar")
                && file.path.matches('/').count() == 1
                && paths.insert(file.path.to_ascii_lowercase()),
            "Invalid receipt path"
        );
        ensure!(
            file.sha512.len() == 128 && hex::decode(&file.sha512).is_ok(),
            "Invalid receipt hash"
        );
    }
    for path in state.baselines.keys() {
        validate_path(path, "config/")?;
    }
    Ok(())
}

pub fn plan(
    root: &Path,
    release: &Release,
    choices: BTreeMap<String, bool>,
    recommended: bool,
) -> Result<Plan> {
    release.validate()?;
    let root = root_path(root)?;
    ensure!(
        !safe(&root, JOURNAL)?.exists(),
        "An interrupted update needs recovery first"
    );
    let previous = load_state(&root, release)?;
    if let Some(old) = &previous {
        ensure!(
            semver::Version::parse(&release.version)? >= semver::Version::parse(&old.version)?,
            "Refusing a release downgrade. Use Restore previous pack instead."
        );
        ensure!(
            old.minecraft == release.minecraft && old.fabric == release.fabric,
            "Minecraft/Fabric changed. Create a separate instance for this runtime migration."
        );
    }
    let selected = release.selected(&choices)?;
    let mut install = vec![];
    for file in &selected {
        let bytes = read_bytes(&safe(&root, &file.path)?)?;
        if bytes
            .as_ref()
            .is_some_and(|b| sha512(b) == file.sha512.to_ascii_lowercase())
        {
            continue;
        }
        if bytes.is_some() {
            let old = previous
                .as_ref()
                .and_then(|s| s.mods.iter().find(|m| m.path == file.path));
            ensure!(
                old.is_some_and(
                    |m| sha512(bytes.as_ref().unwrap()) == m.sha512.to_ascii_lowercase()
                ),
                "Unexpected modified JAR: {}. Restore or move it before updating.",
                file.path
            );
        }
        install.push(file.clone());
    }
    let mut remove = vec![];
    if let Some(old) = &previous {
        for file in &old.mods {
            if selected.iter().any(|m| m.path == file.path) {
                continue;
            }
            if let Some(bytes) = read_bytes(&safe(&root, &file.path)?)? {
                ensure!(
                    sha512(&bytes) == file.sha512.to_ascii_lowercase(),
                    "Retired JAR has been modified: {}",
                    file.path
                );
                remove.push(file.clone());
            }
        }
    }
    let mut presets = vec![];
    let mut conflicts = vec![];
    for preset in &release.presets {
        if preset
            .feature
            .as_ref()
            .is_some_and(|id| !selected.iter().any(|m| m.feature.as_ref() == Some(id)))
        {
            continue;
        }
        let live = read_bytes(&safe(&root, &preset.path)?)?;
        if live.as_deref() == Some(preset.content.as_bytes()) {
            continue;
        }
        let baseline = previous
            .as_ref()
            .and_then(|s| s.baselines.get(&preset.path));
        if live.is_none() || baseline.is_some_and(|b| live.as_deref() == Some(b.as_bytes())) {
            presets.push(preset.clone());
        } else if baseline.is_none_or(|b| b != &preset.content) {
            conflicts.push(preset.path.clone());
        }
    }
    let mut extras = vec![];
    let mods_dir = safe(&root, "mods")?;
    if mods_dir.is_dir() {
        for entry in fs::read_dir(mods_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.to_ascii_lowercase().ends_with(".jar") {
                continue;
            }
            let path = format!("mods/{name}");
            safe(&root, &path)?;
            if !selected.iter().any(|m| m.path == path)
                && !previous
                    .as_ref()
                    .is_some_and(|s| s.mods.iter().any(|m| m.path == path))
            {
                extras.push(path);
            }
        }
    }
    Ok(Plan {
        install,
        remove,
        presets,
        conflicts,
        extras,
        previous,
        choices,
        recommended,
        release: release.clone(),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Change {
    path: String,
    old: bool,
    before: Option<String>,
    after: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Journal {
    schema: u32,
    committed: bool,
    changes: Vec<Change>,
}

fn lock(root: &Path) -> Result<File> {
    let path = safe(root, ".forever-smp/update.lock")?;
    fs::create_dir_all(path.parent().unwrap())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("Another updater is using this game folder")?;
    Ok(file)
}

pub fn ensure_game_closed(root: &Path) -> Result<()> {
    let system = sysinfo::System::new_all();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_ascii_lowercase();
        if !name.contains("java") {
            continue;
        }
        let cmd = process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        ensure!(
            !targets_game(root, &cmd, process.cwd()),
            "Close Minecraft before updating (Java process {})",
            process.pid()
        );
    }
    Ok(())
}

fn targets_game(root: &Path, command: &str, cwd: Option<&Path>) -> bool {
    let normal = |p: &Path| {
        p.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    let game = normal(root);
    if command.replace('/', "\\").contains(&game) {
        return true;
    }
    let is_client = command.contains("net.minecraft.client")
        || command.contains("knotclient")
        || command.contains("org.prismlauncher");
    if !is_client {
        return false;
    }
    match cwd {
        Some(cwd) => {
            let cwd = normal(cwd);
            cwd == game || root.parent().is_some_and(|parent| normal(parent) == cwd)
        }
        // If the OS cannot identify a running client's folder, fail closed.
        None => true,
    }
}

/// Delete only a validated UUID transaction tree, refusing reparse points
/// throughout. Check the whole tree before deleting any files.
pub(crate) fn remove_history_tree(root: &Path, relative: &str) -> Result<()> {
    let id = relative
        .strip_prefix(".forever-smp/backups/")
        .or_else(|| relative.strip_prefix(".forever-installer/pending/"))
        .context("Cleanup is outside managed history")?;
    uuid::Uuid::parse_str(id)?;
    let target = safe(root, relative)?;
    ensure!(target.is_dir(), "History is not a directory");
    let mut pending = vec![relative.to_owned()];
    let mut directories = vec![];
    let mut files = vec![];
    while let Some(directory) = pending.pop() {
        ensure!(
            directory.matches('/').count() < 32,
            "Unexpected history depth"
        );
        for entry in fs::read_dir(safe(root, &directory)?)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .context("Invalid history filename")?
                .to_owned();
            let child = format!("{directory}/{name}");
            let path = safe(root, &child)?;
            if path.is_dir() {
                pending.push(child);
            } else {
                files.push(child);
            }
        }
        directories.push(directory);
    }
    // Leave markers until last so interrupted cleanup can be retried.
    files.sort_by_key(|path| {
        matches!(
            path.rsplit('/').next(),
            Some("journal.json" | "job.json" | "status.txt")
        )
    });
    for path in files {
        fs::remove_file(safe(root, &path)?)?;
    }
    for path in directories.into_iter().rev() {
        fs::remove_dir(safe(root, &path)?)?;
    }
    Ok(())
}

fn prune_game_backups_locked(root: &Path) -> Result<usize> {
    let directory = safe(root, ".forever-smp/backups")?;
    if !directory.is_dir() {
        return Ok(0);
    }
    let mut completed = vec![];
    let mut failed = vec![];
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if uuid::Uuid::parse_str(&name).is_err() {
            continue;
        }
        let relative = format!(".forever-smp/backups/{name}");
        let Ok(path) = safe(root, &format!("{relative}/journal.json")) else {
            continue;
        };
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(journal) = serde_json::from_slice::<Journal>(&bytes) else {
            continue;
        };
        if journal.schema != 1 {
            continue;
        }
        let item = (fs::metadata(&path)?.modified()?, relative);
        if journal.committed {
            completed.push(item);
        } else {
            failed.push(item);
        }
    }
    let mut removed = 0;
    for (mut entries, retain) in [(completed, 3), (failed, 1)] {
        entries.sort_by_key(|item| std::cmp::Reverse(item.0));
        for (_, relative) in entries.into_iter().skip(retain) {
            if remove_history_tree(root, &relative).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

pub fn cleanup_game_backups(root: &Path) -> Result<usize> {
    let root = root_path(root)?;
    let _lock = lock(&root)?;
    // Leave every recovery dependency alone until recovery has finished.
    if safe(&root, ".forever-smp/pending")?.exists() {
        return Ok(0);
    }
    prune_game_backups_locked(&root)
}

fn archive_pending(root: &Path) -> Result<()> {
    let pending = safe(root, ".forever-smp/pending")?;
    if pending.exists() {
        let destination = safe(
            root,
            &format!(".forever-smp/backups/{}", uuid::Uuid::new_v4()),
        )?;
        fs::create_dir_all(destination.parent().unwrap())?;
        fs::rename(pending, destination)?;
    }
    // Housekeeping failure must not turn a committed update into an error.
    let _ = prune_game_backups_locked(root);
    Ok(())
}

fn recover_locked(root: &Path) -> Result<bool> {
    let path = safe(root, JOURNAL)?;
    if !path.exists() {
        archive_pending(root)?;
        return Ok(false);
    }
    let journal: Journal = serde_json::from_slice(&fs::read(path)?)?;
    ensure!(journal.schema == 1, "Unknown transaction journal");
    if !journal.committed {
        for change in journal.changes.iter().rev() {
            ensure!(
                change.path.starts_with("mods/")
                    || change.path.starts_with("config/")
                    || change.path == STATE,
                "Invalid journal path"
            );
            let destination = safe(root, &change.path)?;
            if change.old {
                let backup = safe(
                    root,
                    &format!(".forever-smp/pending/backup/{}", change.path),
                )?;
                let bytes =
                    fs::read(&backup).context("Transaction backup is missing; recovery stopped")?;
                ensure!(
                    change
                        .before
                        .as_ref()
                        .is_some_and(|hash| sha512(&bytes) == *hash),
                    "Transaction backup failed verification"
                );
                write_sync(&destination, &bytes)?;
            } else if destination.exists() {
                fs::remove_file(destination)?;
            }
        }
    }
    archive_pending(root)?;
    Ok(!journal.committed)
}

pub fn recover(root: &Path) -> Result<bool> {
    let root = root_path(root)?;
    let _lock = lock(&root)?;
    ensure_game_closed(&root)?;
    recover_locked(&root)
}

pub fn restore_last(root: &Path) -> Result<()> {
    let root = root_path(root)?;
    let _lock = lock(&root)?;
    ensure_game_closed(&root)?;
    ensure!(
        !safe(&root, JOURNAL)?.exists(),
        "Recover the pending update first"
    );
    let backups = safe(&root, ".forever-smp/backups")?;
    let mut entries: Vec<_> = if backups.is_dir() {
        fs::read_dir(&backups)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().join("journal.json").is_file())
            .collect()
    } else {
        vec![]
    };
    entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());
    // Skip rolled-back/failed transactions; only a committed journal is a usable prior pack.
    for entry in entries.into_iter().rev() {
        let journal_path = entry.path().join("journal.json");
        let mut journal: Journal = serde_json::from_slice(&fs::read(&journal_path)?)?;
        if !journal.committed {
            continue;
        }
        let relative = format!(
            ".forever-smp/backups/{}",
            entry.file_name().to_string_lossy()
        );
        let source = safe(&root, &relative)?;
        // Validate backups before making this transaction pending.
        for change in &journal.changes {
            let live = read_bytes(&safe(&root, &change.path)?)?.map(|b| sha512(&b));
            ensure!(
                live == change.after,
                "File changed since the update: {}. Preserve it before restoring the previous pack.",
                change.path
            );
            if change.old {
                let data = fs::read(safe(&root, &format!("{relative}/backup/{}", change.path))?)?;
                ensure!(
                    change
                        .before
                        .as_ref()
                        .is_some_and(|hash| sha512(&data) == *hash),
                    "Invalid rollback backup"
                );
            }
        }
        journal.committed = false;
        write_sync(&journal_path, &serde_json::to_vec_pretty(&journal)?)?;
        fs::rename(source, safe(&root, ".forever-smp/pending")?)?;
        recover_locked(&root)?;
        return Ok(());
    }
    bail!("No previous pack backup is available")
}

pub(crate) fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent("ForeverSMP-Updater/0.1")
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.url().scheme() != "https" || attempt.previous().len() >= 10 {
                attempt.stop()
            } else {
                attempt.follow()
            }
        }))
        .build()?)
}

pub fn fetch_release(repository: &str, public_key: &str) -> Result<Release> {
    let mut response = client()?
        .get(github_feed(repository)?)
        .send()?
        .error_for_status()?;
    let mut bytes = vec![];
    Read::by_ref(&mut response)
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    verify_release(&bytes, public_key)
}

pub(crate) fn download(file: &ModFile) -> Result<Vec<u8>> {
    let client = client()?;
    let mut last = None;
    for url in &file.urls {
        let result = (|| -> Result<Vec<u8>> {
            let mut response = client.get(url).send()?.error_for_status()?;
            let mut bytes = vec![];
            Read::by_ref(&mut response)
                .take(file.size + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 == file.size
                    && sha512(&bytes) == file.sha512.to_ascii_lowercase(),
                "Download failed size/hash verification: {}",
                file.path
            );
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => return Ok(bytes),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("No download URL")))
}

/// Update only managed paths. Cache is optional and never establishes ownership.
pub fn apply(
    root: &Path,
    original: &Plan,
    replace_conflicts: bool,
    cache: Option<&Path>,
    progress: impl Fn(&str),
) -> Result<()> {
    let root = root_path(root)?;
    let _lock = lock(&root)?;
    ensure_game_closed(&root)?;
    ensure!(
        !safe(&root, JOURNAL)?.exists(),
        "Recover interrupted update first"
    );
    archive_pending(&root)?;
    let mut current = plan(
        &root,
        &original.release,
        original.choices.clone(),
        original.recommended,
    )?;
    ensure!(
        current.previous.is_some() || current.extras.is_empty(),
        "This folder contains unowned JARs. Adopt its previous pack release or choose a clean game folder before installing."
    );
    let backup_size: u64 = current
        .remove
        .iter()
        .chain(current.install.iter())
        .filter_map(|m| fs::metadata(root.join(&m.path)).ok())
        .map(|m| m.len())
        .sum();
    let staged_size: u64 = current.install.iter().map(|m| m.size).sum();
    ensure!(
        fs2::available_space(&root)?
            > staged_size
                .saturating_mul(2)
                .saturating_add(backup_size)
                .saturating_add(10 * 1024 * 1024),
        "Not enough free disk space for verified downloads and backups"
    );
    ensure!(
        serde_json::to_vec(&current.previous)? == serde_json::to_vec(&original.previous)?,
        "Installation changed. Check updates again."
    );
    ensure!(
        current.conflicts == original.conflicts && current.extras == original.extras,
        "Local files changed. Review the new update plan."
    );
    if replace_conflicts {
        for path in &current.conflicts {
            current.presets.push(
                current
                    .release
                    .presets
                    .iter()
                    .find(|p| &p.path == path)
                    .unwrap()
                    .clone(),
            );
        }
    }
    let mut operations: Vec<(String, Option<Vec<u8>>, Option<String>)> = vec![];
    for file in &current.install {
        progress(&file.path);
        let cached = cache.and_then(|dir| {
            let source = dir.join(file.path.rsplit('/').next().unwrap());
            fs::read(source).ok().filter(|b| {
                b.len() as u64 == file.size && sha512(b) == file.sha512.to_ascii_lowercase()
            })
        });
        let bytes = match cached {
            Some(bytes) => bytes,
            None => download(file)?,
        };
        let before = read_bytes(&safe(&root, &file.path)?)?.map(|b| sha512(&b));
        operations.push((file.path.clone(), Some(bytes), before));
    }
    for file in &current.remove {
        operations.push((
            file.path.clone(),
            None,
            Some(file.sha512.to_ascii_lowercase()),
        ));
    }
    for preset in &current.presets {
        let before = read_bytes(&safe(&root, &preset.path)?)?.map(|b| sha512(&b));
        operations.push((
            preset.path.clone(),
            Some(preset.content.as_bytes().to_vec()),
            before,
        ));
    }
    let selected = current.release.selected(&current.choices)?;
    let mut baselines = current
        .previous
        .as_ref()
        .map(|s| s.baselines.clone())
        .unwrap_or_default();
    for preset in &current.release.presets {
        if preset
            .feature
            .as_ref()
            .is_some_and(|id| !selected.iter().any(|m| m.feature.as_ref() == Some(id)))
        {
            continue;
        }
        // Remember the last reviewed shipped defaults, including Keep decisions.
        // The user's live file remains separate and is not overwritten here.
        baselines.insert(preset.path.clone(), preset.content.clone());
    }
    let state = State {
        schema: 1,
        pack_id: PACK_ID.into(),
        version: current.release.version.clone(),
        minecraft: current.release.minecraft.clone(),
        fabric: current.release.fabric.clone(),
        recommended: current.recommended,
        choices: current.choices,
        mods: selected,
        baselines,
    };
    let state_bytes = serde_json::to_vec_pretty(&state)?;
    let old_state = read_bytes(&safe(&root, STATE)?)?;
    if operations.is_empty() && old_state.as_deref() == Some(state_bytes.as_slice()) {
        progress("Already current / Уже обновлено");
        return Ok(());
    }
    let before = old_state.map(|b| sha512(&b));
    operations.push((STATE.into(), Some(state_bytes), before));
    let mut journal = Journal {
        schema: 1,
        committed: false,
        changes: vec![],
    };
    // Stage everything before touching the active pack.
    for (path, bytes, _) in &operations {
        if let Some(bytes) = bytes {
            write_sync(
                &safe(&root, &format!(".forever-smp/pending/stage/{path}"))?,
                bytes,
            )?;
        }
    }
    write_sync(
        &safe(&root, JOURNAL)?,
        &serde_json::to_vec_pretty(&journal)?,
    )?;
    let result = (|| -> Result<()> {
        ensure_game_closed(&root)?;
        for (path, bytes, expected) in &operations {
            let destination = safe(&root, path)?;
            let live = read_bytes(&destination)?;
            ensure!(
                live.as_ref().map(|b| sha512(b)) == *expected,
                "File changed while downloading: {path}"
            );
            if let Some(old) = &live {
                write_sync(
                    &safe(&root, &format!(".forever-smp/pending/backup/{path}"))?,
                    old,
                )?;
            }
            journal.changes.push(Change {
                path: path.clone(),
                old: live.is_some(),
                before: expected.clone(),
                after: bytes.as_ref().map(|b| sha512(b)),
            });
            write_sync(
                &safe(&root, JOURNAL)?,
                &serde_json::to_vec_pretty(&journal)?,
            )?;
            progress(path);
            if destination.exists() {
                fs::remove_file(&destination)?;
            }
            if bytes.is_some() {
                fs::create_dir_all(destination.parent().unwrap())?;
                fs::rename(
                    safe(&root, &format!(".forever-smp/pending/stage/{path}"))?,
                    destination,
                )?;
            }
        }
        journal.committed = true;
        write_sync(
            &safe(&root, JOURNAL)?,
            &serde_json::to_vec_pretty(&journal)?,
        )?;
        archive_pending(&root)?;
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(recovery) = recover_locked(&root) {
            bail!(
                "{error:#}\nRecovery also failed: {recovery:#}. Keep .forever-smp/backups and pending."
            );
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn release(version: &str, name: &str, bytes: &[u8]) -> Release {
        Release {
            schema: 1,
            pack_id: PACK_ID.into(),
            version: format!("{version}.0.0"),
            minecraft: "26.3".into(),
            fabric: "0.19.5".into(),
            java: 25,
            notes_en: String::new(),
            notes_ru: String::new(),
            features: vec![Feature {
                id: "extra".into(),
                en: "Extra".into(),
                ru: "Extra".into(),
                default: true,
                requires: vec![],
            }],
            mods: vec![ModFile {
                id: "test".into(),
                path: format!("mods/{name}"),
                sha512: sha512(bytes),
                size: bytes.len() as u64,
                urls: vec!["https://example.invalid/test.jar".into()],
                feature: None,
            }],
            presets: vec![Preset {
                path: "config/test.json".into(),
                content: "old".into(),
                feature: None,
            }],
        }
    }
    fn apply_cached(root: &Path, r: &Release, cache: &Path, replace: bool) -> Result<()> {
        let old = load_state(root, r)?;
        let p = plan(root, r, r.choices(old.as_ref()), true)?;
        apply(root, &p, replace, Some(cache), |_| {})
    }
    #[test]
    fn bracketed_camera_filename_and_game_folder_install_disable_and_restore() -> Result<()> {
        let temporary = tempfile::tempdir()?;
        let game = temporary.path().join("game[client]");
        let cache = temporary.path().join("cache[downloads]");
        fs::create_dir(&game)?;
        fs::create_dir(&cache)?;
        let name = "CameraOverhaul-v2.1.2-fabric+mc[26.3-plus].jar";
        let bytes = b"verified camera fixture";
        fs::write(cache.join(name), bytes)?;
        let mut r = release("1", name, bytes);
        r.mods[0].feature = Some("extra".into());
        apply_cached(&game, &r, &cache, false)?;
        assert_eq!(fs::read(game.join(&r.mods[0].path))?, bytes);
        let state = load_state(&game, &r)?.unwrap();
        let current = plan(&game, &r, r.choices(Some(&state)), true)?;
        assert!(current.install.is_empty());
        assert!(current.remove.is_empty());
        assert!(current.presets.is_empty());
        apply(&game, &current, false, Some(&cache), |_| {})?;

        let disabled = plan(&game, &r, BTreeMap::from([("extra".into(), false)]), false)?;
        assert_eq!(disabled.remove.len(), 1);
        apply(&game, &disabled, false, Some(&cache), |_| {})?;
        assert!(!game.join(&r.mods[0].path).exists());
        restore_last(&game)?;
        assert_eq!(fs::read(game.join(&r.mods[0].path))?, bytes);
        assert!(load_state(&game, &r)?.unwrap().choices["extra"]);
        Ok(())
    }
    #[test]
    fn update_removes_retired_mods_preserves_personal_data_and_rolls_back() -> Result<()> {
        let game = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        fs::write(cache.path().join("one.jar"), b"one")?;
        fs::write(cache.path().join("two.jar"), b"two")?;
        apply_cached(
            game.path(),
            &release("1", "one.jar", b"one"),
            cache.path(),
            false,
        )?;
        fs::write(game.path().join("config/test.json"), "personal")?;
        fs::write(game.path().join("options.txt"), "keys")?;
        fs::write(game.path().join("mods/user.jar"), "user")?;
        let mut r = release("3", "two.jar", b"two");
        r.presets[0].content = "new".into();
        let old = load_state(game.path(), &r)?.unwrap();
        let p = plan(game.path(), &r, r.choices(Some(&old)), true)?;
        assert_eq!(p.conflicts, ["config/test.json"]);
        assert_eq!(p.extras, ["mods/user.jar"]);
        apply(game.path(), &p, false, Some(cache.path()), |_| {})?;
        assert!(!game.path().join("mods/one.jar").exists());
        assert!(game.path().join("mods/two.jar").exists());
        assert_eq!(
            fs::read_to_string(game.path().join("config/test.json"))?,
            "personal"
        );
        assert_eq!(fs::read_to_string(game.path().join("options.txt"))?, "keys");
        let saved = load_state(game.path(), &r)?.unwrap();
        assert!(
            plan(game.path(), &r, r.choices(Some(&saved)), true)?
                .conflicts
                .is_empty()
        );
        restore_last(game.path())?;
        assert!(game.path().join("mods/one.jar").exists());
        assert!(!game.path().join("mods/two.jar").exists());
        assert_eq!(load_state(game.path(), &r)?.unwrap().version, "1.0.0");
        Ok(())
    }
    #[test]
    fn unchanged_presets_update_modified_jars_block_and_runtime_migration_blocks() -> Result<()> {
        let game = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        fs::write(cache.path().join("one.jar"), b"one")?;
        let mut r = release("1", "one.jar", b"one");
        apply_cached(game.path(), &r, cache.path(), false)?;
        r.version = "2.0.0".into();
        r.presets[0].content = "new".into();
        apply_cached(game.path(), &r, cache.path(), false)?;
        assert_eq!(
            fs::read_to_string(game.path().join("config/test.json"))?,
            "new"
        );
        fs::write(game.path().join("mods/one.jar"), b"tampered")?;
        assert!(plan(game.path(), &r, BTreeMap::new(), false).is_err());
        r.minecraft = "future".into();
        assert!(plan(game.path(), &r, BTreeMap::new(), false).is_err());
        Ok(())
    }
    #[test]
    fn failed_download_changes_nothing_and_recovery_restores_interrupted_write() -> Result<()> {
        let game = tempfile::tempdir()?;
        let r = release("1", "one.jar", b"one");
        let p = plan(game.path(), &r, BTreeMap::new(), false)?;
        // Invalid HTTPS URL fails without deleting or replacing anything.
        assert!(apply(game.path(), &p, false, None, |_| {}).is_err());
        assert!(!game.path().join(STATE).exists());
        let root = root_path(game.path())?;
        write_sync(&safe(&root, "config/test.json")?, b"new")?;
        write_sync(
            &safe(&root, ".forever-smp/pending/backup/config/test.json")?,
            b"old",
        )?;
        let journal = Journal {
            schema: 1,
            committed: false,
            changes: vec![Change {
                path: "config/test.json".into(),
                old: true,
                before: Some(sha512(b"old")),
                after: Some(sha512(b"new")),
            }],
        };
        write_sync(&safe(&root, JOURNAL)?, &serde_json::to_vec(&journal)?)?;
        assert!(recover(game.path())?);
        assert_eq!(
            fs::read_to_string(game.path().join("config/test.json"))?,
            "old"
        );
        Ok(())
    }
    #[test]
    fn feature_choices_and_paths_and_uuid() -> Result<()> {
        let mut r = release("1", "one.jar", b"one");
        r.mods[0].feature = Some("extra".into());
        assert_eq!(r.selected(&BTreeMap::new())?.len(), 0);
        assert_eq!(r.selected(&r.choices(None))?.len(), 1);
        for path in [
            "mods/../secret",
            "mods/a\\b.jar",
            "mods/C:evil.jar",
            "mods/CON.jar",
            "mods/a./evil.jar",
        ] {
            assert!(validate_path(path, "mods/").is_err());
        }
        assert!(access_request("TestPlayer")?.contains("bb77495a-a740-3169-a238-69654c8bd2c1"));
        assert!(access_request("bad name").is_err());
        assert!(!targets_game(
            Path::new("C:/test/new-game"),
            "org.prismlauncher.entrypoint",
            Some(Path::new("C:/other/minecraft"))
        ));
        assert!(targets_game(
            Path::new("C:/test/new-game"),
            "org.prismlauncher.entrypoint",
            Some(Path::new("C:/test/new-game"))
        ));
        Ok(())
    }
    #[test]
    fn adoption_preserves_disabled_features_and_rollbacks_refuse_changed_files() -> Result<()> {
        let game = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        let mut r = release("1", "one.jar", b"one");
        fs::create_dir(game.path().join("mods"))?;
        fs::write(game.path().join("mods/one.jar"), b"one")?;
        let adopted = adopt(game.path(), &r)?;
        assert!(!adopted.choices["extra"]);
        assert_eq!(
            plan(game.path(), &r, r.choices(Some(&adopted)), false)?
                .install
                .len(),
            0
        );
        r.version = "2.0.0".into();
        r.presets[0].content = "second".into();
        apply_cached(game.path(), &r, cache.path(), false)?;
        fs::write(game.path().join("config/test.json"), b"personal later")?;
        assert!(restore_last(game.path()).is_err());
        assert_eq!(
            fs::read_to_string(game.path().join("config/test.json"))?,
            "personal later"
        );
        let mut old = r.clone();
        old.version = "1.0.0".into();
        assert!(plan(game.path(), &old, BTreeMap::new(), false).is_err());
        Ok(())
    }
    #[test]
    fn disabling_option_removes_unneeded_library_and_new_defaults_preserve_existing_choices()
    -> Result<()> {
        let game = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        let mut r = release("1", "one.jar", b"one");
        r.mods[0].feature = Some("extra".into());
        let mut library = r.mods[0].clone();
        library.id = "library".into();
        library.path = "mods/library.jar".into();
        r.mods.push(library);
        fs::write(cache.path().join("one.jar"), b"one")?;
        fs::write(cache.path().join("library.jar"), b"one")?;
        apply_cached(game.path(), &r, cache.path(), false)?;
        let p = plan(
            game.path(),
            &r,
            BTreeMap::from([("extra".into(), false)]),
            false,
        )?;
        assert_eq!(p.remove.len(), 2);
        apply(game.path(), &p, false, Some(cache.path()), |_| {})?;
        let state = load_state(game.path(), &r)?.unwrap();
        r.features.push(Feature {
            id: "new".into(),
            en: "New".into(),
            ru: "New".into(),
            default: true,
            requires: vec![],
        });
        assert!(r.choices(Some(&state))["new"]);
        assert!(!r.choices(Some(&state))["extra"]);
        Ok(())
    }
    #[test]
    fn history_retention_preserves_three_rollbacks_and_active_or_unknown_data() -> Result<()> {
        let game = tempfile::tempdir()?;
        let cache = tempfile::tempdir()?;
        fs::write(cache.path().join("one.jar"), b"one")?;
        let unknown = game
            .path()
            .join(format!(".forever-smp/backups/{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&unknown)?;
        fs::write(unknown.join("personal.txt"), "keep unknown")?;
        for version in 1..=7 {
            let mut r = release(&version.to_string(), "one.jar", b"one");
            r.presets[0].content = format!("preset {version}");
            apply_cached(game.path(), &r, cache.path(), false)?;
        }
        let count = || -> Result<usize> {
            Ok(fs::read_dir(game.path().join(".forever-smp/backups"))?
                .filter_map(|e| e.ok())
                .filter(|e| {
                    fs::read(e.path().join("journal.json"))
                        .ok()
                        .and_then(|b| serde_json::from_slice::<Journal>(&b).ok())
                        .is_some_and(|j| j.committed)
                })
                .count())
        };
        assert_eq!(count()?, 3);
        assert!(unknown.join("personal.txt").exists());
        let pending = game.path().join(".forever-smp/pending");
        fs::create_dir(&pending)?;
        fs::write(pending.join("keep.txt"), "recovery")?;
        assert_eq!(cleanup_game_backups(game.path())?, 0);
        assert!(pending.join("keep.txt").exists());
        fs::remove_file(pending.join("keep.txt"))?;
        fs::remove_dir(pending)?;
        for expected in [6, 5, 4] {
            restore_last(game.path())?;
            assert_eq!(
                fs::read_to_string(game.path().join("config/test.json"))?,
                format!("preset {expected}")
            );
        }
        assert!(restore_last(game.path()).is_err());
        assert!(unknown.join("personal.txt").exists());
        assert!(remove_history_tree(game.path(), "../outside").is_err());
        Ok(())
    }
}
