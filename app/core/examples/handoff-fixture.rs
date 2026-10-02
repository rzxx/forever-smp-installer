//! Disposable process-level updater fixture. Never packaged as the desktop app.
use anyhow::{Result, bail};
use ed25519_dalek::{Signer, SigningKey};
use forever_core::*;
use std::{fs, path::Path};

fn main() -> Result<()> {
    // Public, deterministic test key; unrelated to either production trust pin.
    let key = SigningKey::from_bytes(&[73; 32]);
    let public_key = hex::encode(key.verifying_key().to_bytes());
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("sign") if args.len() == 3 => {
            let payload = fs::read_to_string(&args[1])?;
            serde_json::from_str::<AppRelease>(&payload)?.validate()?;
            let signature = hex::encode(key.sign(payload.as_bytes()).to_bytes());
            fs::write(
                &args[2],
                serde_json::to_vec(&SignedRelease { payload, signature })?,
            )?;
            Ok(())
        }
        Some("rehearse") if args.len() == 4 => {
            let envelope = fs::read(&args[2])?;
            let release = verify_app_release(&envelope, &public_key)?;
            let job = prepare_app_update(
                Path::new(&args[1]),
                "0.0.0",
                &AppOffer { release, envelope },
                &public_key,
                Some(Path::new(&args[3])),
                |_| {},
            )?;
            launch_app_update(&job)?;
            println!("{}", job.display());
            Ok(())
        }
        Some("--self-update-helper") if args.len() == 2 => {
            run_app_update_helper(Path::new(&args[1]), &public_key)
        }
        Some("--finish-app-update") if args.len() == 2 => {
            acknowledge_app_update(Path::new(&args[1]), &public_key, env!("CARGO_PKG_VERSION"))?;
            fs::write(
                Path::new(&args[1])
                    .parent()
                    .unwrap()
                    .join("smoke-ready.txt"),
                "ready",
            )?;
            Ok(())
        }
        Some("--app-update-failed") if args.len() == 2 => {
            let error = app_update_failure(Path::new(&args[1]), &public_key)?;
            fs::write(
                Path::new(&args[1])
                    .parent()
                    .unwrap()
                    .join("smoke-failure.txt"),
                error,
            )?;
            Ok(())
        }
        _ => bail!(
            "Usage: handoff-fixture sign <metadata> <signed> | rehearse <target> <signed> <cache>"
        ),
    }
}
