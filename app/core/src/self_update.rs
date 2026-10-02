use crate::{
    engine::{client, download, remove_history_tree, safe, write_sync},
    model::verify_signed_payload,
    *,
};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub const INSTALLER_ID: &str = "forever-smp-installer";
pub const INSTALLER_REPOSITORY: &str = "rzxx/forever-smp-installer";
pub const INSTALLER_PUBLIC_KEY: &str =
    "1ac27797c7ea21b44c4c6d90641dce081ed000bc5905cde82870ae17e634b91a";
const MAX_APP_SIZE: u64 = 150 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppAsset {
    pub platform: String,
    pub url: String,
    pub size: u64,
    pub sha512: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppRelease {
    pub schema: u32,
    pub app_id: String,
    pub version: String,
    pub notes_en: String,
    pub notes_ru: String,
    pub assets: Vec<AppAsset>,
}
impl AppRelease {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && self.app_id == INSTALLER_ID,
            "Unsupported installer metadata"
        );
        semver::Version::parse(&self.version)?;
        ensure!(!self.assets.is_empty(), "Installer has no download assets");
        let mut platforms = BTreeSet::new();
        for asset in &self.assets {
            ensure!(
                platforms.insert(&asset.platform),
                "Duplicate installer platform"
            );
            ensure!(
                matches!(
                    asset.platform.as_str(),
                    "windows-x86_64" | "macos-aarch64" | "macos-x86_64"
                ),
                "Unsupported installer platform"
            );
            ensure!(
                reqwest::Url::parse(&asset.url)?.scheme() == "https",
                "Installer download must use HTTPS"
            );
            ensure!(
                (1024..=MAX_APP_SIZE).contains(&asset.size),
                "Installer size exceeds limit"
            );
            ensure!(
                asset.sha512.len() == 128 && asset.sha512.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid installer hash"
            );
        }
        Ok(())
    }
    pub fn windows_asset(&self) -> Result<&AppAsset> {
        self.assets
            .iter()
            .find(|a| a.platform == "windows-x86_64")
            .context("This release has no Windows installer")
    }
    pub fn is_newer_than(&self, current: &str) -> Result<bool> {
        Ok(semver::Version::parse(&self.version)? > semver::Version::parse(current)?)
    }
}
#[derive(Clone, Debug)]
pub struct AppOffer {
    pub release: AppRelease,
    pub envelope: Vec<u8>,
}
pub fn verify_app_release(bytes: &[u8], key: &str) -> Result<AppRelease> {
    let release: AppRelease = serde_json::from_str(&verify_signed_payload(bytes, key)?)?;
    release.validate()?;
    Ok(release)
}
pub fn installer_feed(repository: &str) -> Result<String> {
    Ok(github_feed(repository)?.replace("release.signed.json", "installer.signed.json"))
}
pub fn fetch_app_release(repository: &str, key: &str) -> Result<AppOffer> {
    let mut response = client()?
        .get(installer_feed(repository)?)
        .send()?
        .error_for_status()?;
    let mut bytes = vec![];
    Read::by_ref(&mut response)
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    let release = verify_app_release(&bytes, key)?;
    Ok(AppOffer {
        release,
        envelope: bytes,
    })
}
pub fn save_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    write_sync(path, &serde_json::to_vec_pretty(value)?)
}

#[derive(Debug, Serialize, Deserialize)]
struct AppJob {
    schema: u32,
    id: String,
    filename: String,
    old_sha512: String,
    from_version: String,
    envelope: Vec<u8>,
    parent_pid: u32,
    parent_started: u64,
}
struct CheckedJob {
    job: AppJob,
    root: PathBuf,
    stage: PathBuf,
    target: PathBuf,
    release: AppRelease,
}
fn check_job(path: &Path, key: &str) -> Result<CheckedJob> {
    check_job_metadata(path, key, true)
}
fn check_job_metadata(path: &Path, key: &str, require_helper: bool) -> Result<CheckedJob> {
    let path = path.canonicalize()?;
    ensure!(
        fs::metadata(&path)?.len() <= 4 * 1024 * 1024,
        "Invalid app update job size"
    );
    let job: AppJob = serde_json::from_slice(&fs::read(&path)?)?;
    ensure!(job.schema == 1, "Unsupported app update job");
    uuid::Uuid::parse_str(&job.id)?;
    ensure!(
        !job.filename.contains(['/', '\\']) && job.filename.to_ascii_lowercase().ends_with(".exe"),
        "Invalid app filename"
    );
    validate_path(&job.filename, "")?;
    let stage = path
        .parent()
        .context("Missing staging directory")?
        .to_path_buf();
    let root = stage
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .context("Invalid staging location")?
        .to_path_buf();
    ensure!(
        safe(
            &root,
            &format!(".forever-installer/pending/{}/job.json", job.id)
        )? == path,
        "App staging directory is outside its installation"
    );
    let release = verify_app_release(&job.envelope, key)?;
    ensure!(
        release.is_newer_than(&job.from_version)?,
        "Refusing an installer downgrade"
    );
    let target = safe(&root, &job.filename)?;
    if require_helper {
        ensure!(
            sha512(&fs::read(safe(
                &root,
                &format!(".forever-installer/pending/{}/helper.exe", job.id)
            )?)?)
                == job.old_sha512,
            "Changed update helper"
        );
    }
    Ok(CheckedJob {
        job,
        root,
        stage,
        target,
        release,
    })
}
pub fn prepare_app_update(
    executable: &Path,
    current_version: &str,
    offer: &AppOffer,
    key: &str,
    cache: Option<&Path>,
    mut progress: impl FnMut(&str),
) -> Result<PathBuf> {
    let release = verify_app_release(&offer.envelope, key)?;
    ensure!(
        release.is_newer_than(current_version)?,
        "Installer is already current"
    );
    let asset = release.windows_asset()?;
    progress("download-app");
    let bytes = if let Some(cache) = cache {
        ensure!(
            fs::metadata(cache)?.len() == asset.size,
            "Cached app size mismatch"
        );
        fs::read(cache)?
    } else {
        download(&ModFile {
            id: INSTALLER_ID.into(),
            path: "installer.exe".into(),
            size: asset.size,
            sha512: asset.sha512.clone(),
            urls: vec![asset.url.clone()],
            feature: None,
        })?
    };
    progress("prepare-restart");
    stage_app_update(executable, current_version, &offer.envelope, key, &bytes)
}
fn stage_app_update(
    executable: &Path,
    current_version: &str,
    envelope: &[u8],
    key: &str,
    bytes: &[u8],
) -> Result<PathBuf> {
    let release = verify_app_release(envelope, key)?;
    ensure!(
        release.is_newer_than(current_version)?,
        "Installer is already current"
    );
    let asset = release.windows_asset()?;
    ensure!(
        bytes.len() as u64 == asset.size && sha512(bytes) == asset.sha512.to_ascii_lowercase(),
        "Installer download failed verification"
    );
    ensure!(bytes.starts_with(b"MZ"), "Invalid Windows executable");
    let executable = executable.canonicalize()?;
    let root = executable.parent().context("Installer has no folder")?;
    ensure!(
        root.parent().is_some(),
        "Cannot update an app in the filesystem root"
    );
    let filename = executable
        .file_name()
        .context("Missing installer filename")?
        .to_str()
        .context("Invalid installer filename")?
        .to_owned();
    ensure!(
        safe(root, &filename)? == executable,
        "Unsafe installer path"
    );
    ensure!(
        fs2::available_space(root)?
            > asset
                .size
                .saturating_mul(4)
                .saturating_add(10 * 1024 * 1024),
        "Not enough space for the installer update and backup"
    );
    let old = fs::read(&executable)?;
    let id = uuid::Uuid::new_v4().to_string();
    let relative = format!(".forever-installer/pending/{id}");
    let stage = safe(root, &relative)?;
    fs::create_dir_all(&stage)?;
    write_sync(&safe(root, &format!("{relative}/new.exe"))?, bytes)?;
    write_sync(&safe(root, &format!("{relative}/helper.exe"))?, &old)?;
    let processes = sysinfo::System::new_all();
    let parent_pid = std::process::id();
    let parent_started = processes
        .process(sysinfo::Pid::from_u32(parent_pid))
        .context("Cannot identify installer process")?
        .start_time();
    let job = AppJob {
        schema: 1,
        id,
        filename,
        old_sha512: sha512(&old),
        from_version: current_version.into(),
        envelope: envelope.to_vec(),
        parent_pid,
        parent_started,
    };
    let path = stage.join("job.json");
    save_json_atomic(&path, &job)?;
    write_sync(&stage.join("status.txt"), b"prepared")?;
    Ok(path)
}
pub fn launch_app_update(job_path: &Path) -> Result<()> {
    let stage = job_path.parent().context("Missing updater folder")?;
    let mut command = app_command(&stage.join("helper.exe"));
    command.arg("--self-update-helper").arg(job_path);
    command
        .spawn()
        .context("Could not start installer update helper")?;
    Ok(())
}
fn app_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    // Detach inherited pipes as well as the console, so a launcher or
    // maintainer CLI can exit while the updated GUI remains open.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}
fn install_app(job: &CheckedJob) -> Result<()> {
    ensure!(
        sha512(&fs::read(&job.target)?) == job.job.old_sha512,
        "Installer changed while downloading; check again"
    );
    let new = fs::read(job.stage.join("new.exe"))?;
    let asset = job.release.windows_asset()?;
    ensure!(
        new.len() as u64 == asset.size && sha512(&new) == asset.sha512.to_ascii_lowercase(),
        "Staged installer changed"
    );
    write_sync(&job.stage.join("old.exe"), &fs::read(&job.target)?)?;
    // Replace in one filesystem operation: the old executable always remains
    // present until the verified replacement is ready on the same volume.
    write_sync(&job.target, &new)?;
    write_sync(&job.stage.join("status.txt"), b"replaced")?;
    Ok(())
}
fn rollback_app(job: &CheckedJob) -> Result<()> {
    let live = sha512(&fs::read(&job.target)?);
    if live == job.job.old_sha512 {
        return Ok(());
    }
    ensure!(
        live == job.release.windows_asset()?.sha512.to_ascii_lowercase(),
        "Installer changed after replacement; refusing to overwrite it"
    );
    let old = fs::read(job.stage.join("old.exe"))?;
    ensure!(
        sha512(&old) == job.job.old_sha512,
        "Invalid previous installer backup"
    );
    write_sync(&job.target, &old)?;
    write_sync(&job.stage.join("status.txt"), b"rolled-back")?;
    Ok(())
}
pub fn acknowledge_app_update(job_path: &Path, key: &str, running_version: &str) -> Result<()> {
    let job = check_job(job_path, key)?;
    ensure!(
        job.release.version == running_version,
        "Downloaded app version does not match signed metadata"
    );
    ensure!(
        std::env::current_exe()?.canonicalize()? == job.target,
        "Update acknowledgement came from another app"
    );
    ensure!(
        sha512(&fs::read(&job.target)?) == job.release.windows_asset()?.sha512.to_ascii_lowercase(),
        "Installed app hash mismatch"
    );
    write_sync(&job.stage.join("ready.txt"), job.job.id.as_bytes())?;
    Ok(())
}
pub fn app_update_failure(job_path: &Path, key: &str) -> Result<String> {
    let job = check_job(job_path, key)?;
    ensure!(
        sha512(&fs::read(&job.target)?) == job.job.old_sha512,
        "Previous installer was not restored"
    );
    Ok(fs::read_to_string(job.stage.join("error.txt"))?)
}

#[derive(Serialize, Deserialize)]
struct PreviousInstaller {
    schema: u32,
    source_job: String,
    version: String,
    sha512: String,
}

/// Compact confirmed jobs only, after their helpers exit. Preserve one
/// previous executable; unknown, failed and unfinished jobs stay untouched.
pub fn cleanup_installer_updates(executable: &Path, key: &str) -> Result<usize> {
    let executable = executable.canonicalize()?;
    let root = executable.parent().context("Installer has no folder")?;
    let pending = safe(root, ".forever-installer/pending")?;
    if !pending.is_dir() {
        return Ok(0);
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(safe(root, ".forever-installer/update.lock")?)?;
    lock.try_lock_exclusive()
        .context("Installer helper is still running")?;
    let processes = sysinfo::System::new_all();
    let running: Vec<_> = processes
        .processes()
        .values()
        .filter_map(|p| p.exe()?.canonicalize().ok())
        .collect();
    let mut jobs = vec![];
    for entry in fs::read_dir(&pending)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if uuid::Uuid::parse_str(&name).is_err() {
            continue;
        }
        let relative = format!(".forever-installer/pending/{name}");
        let Ok(status) = safe(root, &format!("{relative}/status.txt")) else {
            continue;
        };
        if !fs::read_to_string(&status).is_ok_and(|s| s == "confirmed") {
            continue;
        }
        let Ok(path) = safe(root, &format!("{relative}/job.json")) else {
            continue;
        };
        let Ok(job) = check_job_metadata(&path, key, false) else {
            continue;
        };
        if job.target != executable || running.iter().any(|p| p.starts_with(&job.stage)) {
            continue;
        }
        // Never remove files someone added to an installer job directory.
        let entries = fs::read_dir(&job.stage)?.collect::<std::io::Result<Vec<_>>>()?;
        if entries.iter().any(|e| {
            !e.file_type().is_ok_and(|kind| kind.is_file())
                || !matches!(
                    e.file_name().to_str(),
                    Some(
                        "job.json"
                            | "status.txt"
                            | "old.exe"
                            | "new.exe"
                            | "helper.exe"
                            | "ready.txt"
                    )
                )
        }) {
            continue;
        }
        jobs.push((
            semver::Version::parse(&job.job.from_version)?,
            fs::metadata(&status)?.modified()?,
            job,
        ));
    }
    jobs.sort_by(|a, b| (&b.0, &b.1).cmp(&(&a.0, &a.1)));
    let backup_path = safe(root, ".forever-installer/backups/previous.exe")?;
    let metadata_path = safe(root, ".forever-installer/backups/previous.json")?;
    let previous = fs::read(&metadata_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<PreviousInstaller>(&b).ok());
    let mut preserved = previous.as_ref().and_then(|p| {
        if p.schema == 1 && fs::read(&backup_path).is_ok_and(|b| sha512(&b) == p.sha512) {
            semver::Version::parse(&p.version).ok()
        } else {
            None
        }
    });
    let mut removed = 0;
    for (version, _, job) in jobs {
        let bytes = fs::read(safe(
            root,
            &format!(".forever-installer/pending/{}/old.exe", job.job.id),
        )?)
        .ok();
        if preserved.as_ref().is_none_or(|p| version > *p) {
            let Some(bytes) = bytes.filter(|b| sha512(b) == job.job.old_sha512) else {
                continue;
            };
            fs::create_dir_all(backup_path.parent().unwrap())?;
            write_sync(&backup_path, &bytes)?;
            save_json_atomic(
                &metadata_path,
                &PreviousInstaller {
                    schema: 1,
                    source_job: job.job.id.clone(),
                    version: version.to_string(),
                    sha512: job.job.old_sha512.clone(),
                },
            )?;
            preserved = Some(version);
        }
        if remove_history_tree(root, &format!(".forever-installer/pending/{}", job.job.id)).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}
pub fn run_app_update_helper(job_path: &Path, key: &str) -> Result<()> {
    let job = check_job(job_path, key)?;
    let started = Instant::now();
    loop {
        let processes = sysinfo::System::new_all();
        let parent_alive = processes
            .process(sysinfo::Pid::from_u32(job.job.parent_pid))
            .is_some_and(|p| p.start_time() == job.job.parent_started);
        if !parent_alive {
            break;
        }
        ensure!(
            started.elapsed() < Duration::from_secs(30),
            "The previous app did not close"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Keep the same lock through replacement, startup and rollback. Failure
    // to acquire it must never roll back another helper's successful update.
    let lock_path = safe(&job.root, ".forever-installer/update.lock")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)?;
    lock.try_lock_exclusive()
        .context("Another installer update is running")?;
    let mut launched = None;
    let result = (|| -> Result<()> {
        let processes = sysinfo::System::new_all();
        ensure!(
            !processes.processes().values().any(|p| p
                .exe()
                .and_then(|p| p.canonicalize().ok())
                .is_some_and(|p| p == job.target)),
            "Close the other installer window and try again"
        );
        install_app(&job)?;
        launched = Some(
            app_command(&job.target)
                .arg("--finish-app-update")
                .arg(job_path)
                .spawn()
                .context("Updated app could not start")?,
        );
        let child = launched.as_mut().unwrap();
        let started = Instant::now();
        loop {
            if fs::read_to_string(job.stage.join("ready.txt"))
                .is_ok_and(|ready| ready == job.job.id)
            {
                write_sync(&job.stage.join("status.txt"), b"confirmed")?;
                return Ok(());
            }
            if child.try_wait()?.is_some() {
                bail!("Updated app closed before confirming startup");
            }
            if started.elapsed() >= Duration::from_secs(45) {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Updated app did not confirm startup");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    if let Err(error) = result {
        if let Some(child) = &mut launched {
            let _ = child.kill();
            let _ = child.wait();
        }
        rollback_app(&job).context("Installer rollback failed; keep .forever-installer backups")?;
        write_sync(
            &job.stage.join("error.txt"),
            format!("{error:#}").as_bytes(),
        )?;
        app_command(&job.target)
            .arg("--app-update-failed")
            .arg(job_path)
            .spawn()
            .context("Previous installer could not restart")?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    fn fixture(bytes: &[u8]) -> (Vec<u8>, String) {
        let key = SigningKey::from_bytes(&[73; 32]);
        let release = AppRelease {
            schema: 1,
            app_id: INSTALLER_ID.into(),
            version: "0.1.8".into(),
            notes_en: "A fix".into(),
            notes_ru: "Исправление".into(),
            assets: vec![AppAsset {
                platform: "windows-x86_64".into(),
                url: "https://example.invalid/app.exe".into(),
                size: bytes.len() as u64,
                sha512: sha512(bytes),
            }],
        };
        let payload = serde_json::to_string(&release).unwrap();
        let signed = SignedRelease {
            signature: hex::encode(key.sign(payload.as_bytes()).to_bytes()),
            payload,
        };
        (
            serde_json::to_vec(&signed).unwrap(),
            hex::encode(key.verifying_key().to_bytes()),
        )
    }
    #[test]
    fn signed_installer_metadata_is_distinct_from_pack_and_checks_versions() -> Result<()> {
        let bytes = (*b"MZ")
            .into_iter()
            .chain(vec![0; 1022])
            .collect::<Vec<_>>();
        let (signed, key) = fixture(&bytes);
        let app = verify_app_release(&signed, &key)?;
        assert!(app.is_newer_than("0.1.7")?);
        assert!(!app.is_newer_than("0.1.8")?);
        assert!(!app.is_newer_than("0.1.9")?);
        assert_eq!(app.notes_ru, "Исправление");
        assert!(verify_release(&signed, &key).is_err());
        assert!(verify_app_release(&signed, &"00".repeat(32)).is_err());
        let mut tampered: SignedRelease = serde_json::from_slice(&signed)?;
        tampered.payload = tampered.payload.replace("A fix", "Injected");
        assert!(verify_app_release(&serde_json::to_vec(&tampered)?, &key).is_err());
        assert!(installer_feed("../bad/repo").is_err());
        Ok(())
    }
    #[test]
    fn app_replacement_is_verified_preserves_preferences_and_restores_backup() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let root = folder.path().join("setup[client]");
        fs::create_dir(&root)?;
        let executable = root.join("Forever-SMP.exe");
        fs::write(&executable, b"old executable")?;
        fs::write(root.join("preferences.json"), b"personal choices")?;
        let bytes = (*b"MZ")
            .into_iter()
            .chain(vec![7; 1022])
            .collect::<Vec<_>>();
        let (signed, key) = fixture(&bytes);
        assert!(stage_app_update(&executable, "0.1.7", &signed, &key, b"bad").is_err());
        assert_eq!(fs::read(&executable)?, b"old executable");
        let path = stage_app_update(&executable, "0.1.7", &signed, &key, &bytes)?;
        let job = check_job(&path, &key)?;
        install_app(&job)?;
        assert_eq!(fs::read(&executable)?, bytes);
        assert_eq!(
            fs::read(root.join("preferences.json"))?,
            b"personal choices"
        );
        rollback_app(&job)?;
        assert_eq!(fs::read(&executable)?, b"old executable");
        // Changed executables must never be overwritten by a stale update.
        fs::write(&executable, b"another app")?;
        assert!(install_app(&job).is_err());
        assert!(rollback_app(&job).is_err());
        Ok(())
    }
    #[test]
    fn confirmed_jobs_compact_keep_latest_backup_and_preserve_pending_and_unknown_data()
    -> Result<()> {
        let folder = tempfile::tempdir()?;
        let root = folder.path().join("setup[cleanup]");
        fs::create_dir(&root)?;
        let executable = root.join("Forever-SMP.exe");
        fs::write(&executable, b"previous app")?;
        let bytes = (*b"MZ")
            .into_iter()
            .chain(vec![7; 1022])
            .collect::<Vec<_>>();
        let (signed, key) = fixture(&bytes);
        let newer = stage_app_update(&executable, "0.1.7", &signed, &key, &bytes)?;
        install_app(&check_job(&newer, &key)?)?;
        write_sync(&newer.parent().unwrap().join("status.txt"), b"confirmed")?;
        let older = stage_app_update(&executable, "0.1.6", &signed, &key, &bytes)?;
        install_app(&check_job(&older, &key)?)?;
        write_sync(&older.parent().unwrap().join("status.txt"), b"confirmed")?;
        let pending = stage_app_update(&executable, "0.1.7", &signed, &key, &bytes)?;
        let unknown = stage_app_update(&executable, "0.1.7", &signed, &key, &bytes)?;
        write_sync(&unknown.parent().unwrap().join("status.txt"), b"confirmed")?;
        fs::write(unknown.parent().unwrap().join("personal.txt"), "keep")?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".forever-installer/update.lock"))?;
        lock.try_lock_exclusive()?;
        assert!(cleanup_installer_updates(&executable, &key).is_err());
        assert!(newer.exists());
        drop(lock);
        assert_eq!(cleanup_installer_updates(&executable, &key)?, 2);
        assert!(!newer.parent().unwrap().exists());
        assert!(!older.parent().unwrap().exists());
        assert_eq!(
            fs::read(root.join(".forever-installer/backups/previous.exe"))?,
            b"previous app"
        );
        let backup: PreviousInstaller = serde_json::from_slice(&fs::read(
            root.join(".forever-installer/backups/previous.json"),
        )?)?;
        assert_eq!(backup.version, "0.1.7");
        assert!(pending.exists());
        assert!(unknown.parent().unwrap().join("personal.txt").exists());
        assert_eq!(fs::read(&executable)?, bytes);
        assert_eq!(cleanup_installer_updates(&executable, &key)?, 0);
        Ok(())
    }
}
