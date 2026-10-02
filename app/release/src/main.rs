use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signer, SigningKey};
use forever_core::*;
use rand::RngCore;
use std::{collections::BTreeMap, fs, io::Read, path::Path};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("build-installer") if args.len() == 6 => {
            let bytes = fs::read(&args[1])?;
            let settings: toml::Value = toml::from_str(&fs::read_to_string(&args[4])?)?;
            github_feed(&args[3])?;
            let release = AppRelease {
                schema: 1,
                app_id: INSTALLER_ID.into(),
                version: args[2].clone(),
                notes_en: settings["notes_en"]
                    .as_str()
                    .context("Missing English app notes")?
                    .into(),
                notes_ru: settings["notes_ru"]
                    .as_str()
                    .context("Missing Russian app notes")?
                    .into(),
                assets: vec![AppAsset {
                    platform: "windows-x86_64".into(),
                    url: format!(
                        "https://github.com/{}/releases/download/v{}/Forever-SMP-Setup-{}-Windows-x64.exe",
                        args[3], args[2], args[2]
                    ),
                    size: bytes.len() as u64,
                    sha512: sha512(&bytes),
                }],
            };
            release.validate()?;
            fs::write(&args[5], serde_json::to_vec_pretty(&release)?)?;
            Ok(())
        }
        Some("check-app-feed") if args.len() == 3 => {
            let offer = fetch_app_release(&args[1], &args[2])?;
            println!(
                "Verified installer feed: {} / {}",
                offer.release.app_id, offer.release.version
            );
            Ok(())
        }
        Some("cleanup-installer") if args.len() == 3 => {
            println!(
                "Removed {} confirmed installer jobs; retained one previous installer",
                cleanup_installer_updates(Path::new(&args[1]), &args[2])?
            );
            Ok(())
        }
        Some("cleanup-game") if args.len() == 2 => {
            println!(
                "Removed {} old transactions; retained three completed and one failed transaction",
                cleanup_game_backups(Path::new(&args[1]))?
            );
            Ok(())
        }
        Some("build") if args.len() == 4 => build(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
        ),
        Some("keygen") if args.len() == 2 => {
            let path = Path::new(&args[1]);
            ensure!(
                !path.exists(),
                "Refusing to replace an existing signing key"
            );
            let mut seed = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut seed);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, hex::encode(seed))?;
            println!(
                "Public release key: {}",
                hex::encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes())
            );
            println!("Keep the private key outside public release artifacts.");
            Ok(())
        }
        Some("sign" | "sign-app") if args.len() == 4 => {
            let payload = fs::read_to_string(&args[1])?;
            if args[0] == "sign-app" {
                serde_json::from_str::<AppRelease>(&payload)?.validate()?;
            } else {
                serde_json::from_str::<Release>(&payload)?.validate()?;
            }
            let seed: [u8; 32] = hex::decode(fs::read_to_string(&args[2])?.trim())?
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid private key"))?;
            let signature = SigningKey::from_bytes(&seed).sign(payload.as_bytes());
            fs::write(
                &args[3],
                serde_json::to_vec_pretty(&SignedRelease {
                    payload,
                    signature: hex::encode(signature.to_bytes()),
                })?,
            )?;
            Ok(())
        }
        Some("check") if args.len() == 2 => {
            let release: Release = serde_json::from_slice(&fs::read(&args[1])?)?;
            release.validate()?;
            println!(
                "{}: {} client files, {} features, {} presets",
                release.version,
                release.mods.len(),
                release.features.len(),
                release.presets.len()
            );
            Ok(())
        }
        Some("verify") if args.len() == 3 => {
            let release = verify_release(&fs::read(&args[1])?, &args[2])?;
            println!(
                "Verified pack candidate: {} / {}",
                release.pack_id, release.version
            );
            Ok(())
        }
        Some("verify-app") if args.len() == 2 => {
            let release = verify_app_release(&fs::read(&args[1])?, INSTALLER_PUBLIC_KEY)?;
            println!(
                "Verified installer candidate: {} / {}",
                release.app_id, release.version
            );
            Ok(())
        }
        Some("check-feed") if args.len() == 3 => {
            let release = fetch_release(&args[1], &args[2])?;
            println!(
                "Verified public feed: {} / {} / {} client files",
                release.pack_id,
                release.version,
                release.mods.len()
            );
            Ok(())
        }
        Some("install") if (3..=4).contains(&args.len()) => {
            let release: Release = serde_json::from_slice(&fs::read(&args[1])?)?;
            let game = Path::new(&args[2]);
            recover(game)?;
            let old = load_state(game, &release)?;
            let plan = plan(
                game,
                &release,
                release.choices(old.as_ref()),
                old.as_ref().is_none_or(|s| s.recommended),
            )?;
            println!(
                "{} installs, {} removals, {} personal config conflicts kept, {} extra mods kept",
                plan.install.len(),
                plan.remove.len(),
                plan.conflicts.len(),
                plan.extras.len()
            );
            apply(game, &plan, false, args.get(3).map(Path::new), |p| {
                println!("{p}")
            })
        }
        Some("restore") if args.len() == 2 => restore_last(Path::new(&args[1])),
        _ => bail!(
            "Usage:\n  forever-release build <workspace> <mrpack> <output-directory>\n  forever-release check <release.json>\n  forever-release verify <release.signed.json> <public-key>\n  forever-release verify-app <installer.signed.json>\n  forever-release install <release.json> <game-directory> [verified-cache-directory]\n  forever-release restore <game-directory>\n  forever-release keygen <private-key-path>\n  forever-release sign <release.json> <private-key-path> <release.signed.json>\n  forever-release check-feed <repository> <public-key>\n  forever-release build-installer <exe> <app-version> <repository> <notes.toml> <output.json>\n  forever-release sign-app <installer.json> <private-key-path> <installer.signed.json>\n  forever-release check-app-feed <repository> <public-key>"
        ),
    }
}

fn build(root: &Path, pack: &Path, output: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(fs::File::open(pack)?)?;
    let mut json = String::new();
    archive
        .by_name("modrinth.index.json")?
        .read_to_string(&mut json)?;
    let index: serde_json::Value = serde_json::from_str(&json)?;
    let settings: toml::Value = toml::from_str(&fs::read_to_string(root.join("release.toml"))?)?;
    let mut mapping = BTreeMap::new();
    for entry in fs::read_dir(root.join("mods"))? {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let value: toml::Value = toml::from_str(&fs::read_to_string(path)?)?;
        let filename = value["filename"]
            .as_str()
            .context("Missing mod filename")?
            .to_owned();
        let id = value["update"]["modrinth"]["mod-id"]
            .as_str()
            .context("Missing Modrinth project ID")?
            .to_owned();
        mapping.insert(filename, id);
    }
    let mut mods = vec![];
    for file in index["files"].as_array().context("Invalid MRPack")? {
        if file["env"]["client"] == "unsupported" {
            continue;
        }
        let path = file["path"].as_str().context("Missing mod path")?;
        let filename = path.rsplit('/').next().unwrap();
        let feature = if file["env"]["client"] == "optional" {
            Some(
                legacy_feature(path)
                    .context("Optional mod needs a stable feature ID")?
                    .into(),
            )
        } else {
            None
        };
        let project_id = file["downloads"]
            .as_array()
            .and_then(|urls| urls.first())
            .and_then(|v| v.as_str())
            .and_then(|url| url.strip_prefix("https://cdn.modrinth.com/data/"))
            .and_then(|s| s.split('/').next());
        mods.push(ModFile {
            id: mapping
                .get(filename)
                .map(String::as_str)
                .or(project_id)
                .context("MRPack/source pin mismatch")?
                .into(),
            path: path.into(),
            sha512: file["hashes"]["sha512"]
                .as_str()
                .context("Missing hash")?
                .into(),
            size: file["fileSize"].as_u64().context("Missing size")?,
            urls: file["downloads"]
                .as_array()
                .context("Missing downloads")?
                .iter()
                .map(|v| v.as_str().context("Invalid URL").map(String::from))
                .collect::<Result<_>>()?,
            feature,
        });
    }
    mods.sort_by(|a, b| a.id.cmp(&b.id));
    let mut features: Vec<Feature> = settings["features"]
        .as_array()
        .context("Missing feature descriptors")?
        .iter()
        .map(|v| v.clone().try_into().map_err(anyhow::Error::from))
        .collect::<Result<_>>()?;
    // Historical recipe support for adoption/rehearsal; absent in current releases.
    if mods
        .iter()
        .any(|m| m.feature.as_deref() == Some("clock-in"))
    {
        features.push(Feature {
            id: "clock-in".into(),
            en: "Retired Clock-In".into(),
            ru: "Удалённый Clock-In".into(),
            default: true,
            requires: vec![],
        });
    }
    let mut presets = vec![];
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if entry.is_dir() || entry.name() == "modrinth.index.json" {
            continue;
        }
        ensure!(
            entry.name().starts_with("overrides/config/"),
            "Unreviewed file in public MRPack: {}",
            entry.name()
        );
        let path = entry.name().strip_prefix("overrides/").unwrap().to_owned();
        ensure!(
            settings["public_presets"]
                .as_array()
                .context("Missing reviewed public presets")?
                .iter()
                .any(|v| v.as_str() == Some(&path)),
            "Unreviewed public preset: {path}"
        );
        let feature = match path.as_str() {
            "config/firstperson.json" => Some("first-person".into()),
            "config/cameraoverhaul.toml" => Some("camera-overhaul".into()),
            _ => None,
        };
        let mut content = String::new();
        entry.read_to_string(&mut content)?;
        presets.push(Preset {
            path,
            content,
            feature,
        });
    }
    presets.sort_by(|a, b| a.path.cmp(&b.path));
    let release = Release {
        schema: 1,
        pack_id: PACK_ID.into(),
        version: index["versionId"]
            .as_str()
            .context("Missing version")?
            .into(),
        minecraft: index["dependencies"]["minecraft"]
            .as_str()
            .context("Missing Minecraft")?
            .into(),
        fabric: index["dependencies"]["fabric-loader"]
            .as_str()
            .context("Missing Fabric")?
            .into(),
        java: 25,
        notes_en: settings["notes_en"]
            .as_str()
            .context("Missing public English notes")?
            .into(),
        notes_ru: settings["notes_ru"]
            .as_str()
            .context("Missing public Russian notes")?
            .into(),
        mods,
        features,
        presets,
    };
    release.validate()?;
    fs::create_dir_all(output)?;
    fs::write(
        output.join("release.json"),
        serde_json::to_vec_pretty(&release)?,
    )?;
    fs::copy(pack, output.join(pack.file_name().unwrap()))?;
    println!(
        "Built public recipe {} into {}. Review notes and presets before publishing.",
        release.version,
        output.display()
    );
    Ok(())
}
