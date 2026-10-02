use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, VerifyingKey};
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use sha2::Sha512;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
};

pub const PACK_ID: &str = "forever-smp";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Feature {
    pub id: String,
    pub en: String,
    pub ru: String,
    pub default: bool,
    #[serde(default)]
    pub requires: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModFile {
    pub id: String,
    pub path: String,
    pub sha512: String,
    pub size: u64,
    pub urls: Vec<String>,
    pub feature: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preset {
    pub path: String,
    pub content: String,
    pub feature: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub schema: u32,
    pub pack_id: String,
    pub version: String,
    pub minecraft: String,
    pub fabric: String,
    pub java: u32,
    pub notes_en: String,
    pub notes_ru: String,
    pub features: Vec<Feature>,
    pub mods: Vec<ModFile>,
    pub presets: Vec<Preset>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct State {
    pub schema: u32,
    pub pack_id: String,
    pub version: String,
    pub minecraft: String,
    pub fabric: String,
    pub recommended: bool,
    pub choices: BTreeMap<String, bool>,
    pub mods: Vec<ModFile>,
    pub baselines: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SignedRelease {
    pub payload: String,
    pub signature: String,
}

pub fn verify_release(bytes: &[u8], public_key: &str) -> Result<Release> {
    let payload = verify_signed_payload(bytes, public_key)?;
    let release: Release = serde_json::from_str(&payload)?;
    release.validate()?;
    Ok(release)
}

pub(crate) fn verify_signed_payload(bytes: &[u8], public_key: &str) -> Result<String> {
    ensure!(
        bytes.len() <= 2 * 1024 * 1024,
        "Release metadata exceeds size limit"
    );
    let envelope: SignedRelease = serde_json::from_slice(bytes)?;
    let key: [u8; 32] = hex::decode(public_key)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid release key"))?;
    let signature = Signature::from_slice(&hex::decode(envelope.signature)?)?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(envelope.payload.as_bytes(), &signature)
        .context("Release signature is invalid")?;
    Ok(envelope.payload)
}

pub fn sha512(bytes: &[u8]) -> String {
    hex::encode(Sha512::digest(bytes))
}

pub fn validate_path(path: &str, prefix: &str) -> Result<()> {
    ensure!(
        path.starts_with(prefix) && path.len() > prefix.len(),
        "Invalid managed path: {path}"
    );
    ensure!(
        !path.contains(['\\', ':', '\0']) && !path.ends_with([' ', '.']),
        "Unsafe path: {path}"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "Unsafe path: {path}"
    );
    for part in path.split('/') {
        ensure!(
            !part.is_empty() && part != "." && part != ".." && !part.ends_with([' ', '.']),
            "Unsafe path: {path}"
        );
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        ensure!(
            !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                && !((stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.len() == 4
                    && stem.as_bytes()[3].is_ascii_digit()),
            "Reserved path: {path}"
        );
    }
    Ok(())
}

impl Release {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1 && self.pack_id == PACK_ID,
            "Unsupported pack or schema"
        );
        ensure!(
            !self.version.is_empty() && !self.minecraft.is_empty() && !self.fabric.is_empty(),
            "Missing runtime/version"
        );
        semver::Version::parse(&self.version)
            .context("Pack version must use semantic versioning")?;
        let features: BTreeSet<_> = self.features.iter().map(|f| f.id.as_str()).collect();
        ensure!(
            features.len() == self.features.len(),
            "Duplicate feature IDs"
        );
        for f in &self.features {
            for id in &f.requires {
                ensure!(
                    features.contains(id.as_str()),
                    "Unknown feature dependency {id}"
                );
            }
        }
        let mut paths = BTreeSet::new();
        let mut ids = BTreeSet::new();
        for file in &self.mods {
            validate_path(&file.path, "mods/")?;
            ensure!(
                file.path.ends_with(".jar") && file.path.matches('/').count() == 1,
                "Expected top-level JAR"
            );
            ensure!(
                paths.insert(file.path.to_ascii_lowercase()) && ids.insert(&file.id),
                "Duplicate mod path/ID"
            );
            ensure!(
                file.sha512.len() == 128 && hex::decode(&file.sha512).is_ok(),
                "Invalid SHA512"
            );
            ensure!(
                file.size > 0 && file.size <= 512 * 1024 * 1024,
                "Invalid mod size"
            );
            ensure!(!file.urls.is_empty(), "No download URL");
            for url in &file.urls {
                ensure!(
                    reqwest::Url::parse(url)?.scheme() == "https",
                    "Non-HTTPS mod URL"
                );
            }
            if let Some(id) = &file.feature {
                ensure!(features.contains(id.as_str()), "Unknown feature {id}");
            }
        }
        for preset in &self.presets {
            validate_path(&preset.path, "config/")?;
            ensure!(
                paths.insert(preset.path.to_ascii_lowercase()),
                "Duplicate preset path"
            );
            if let Some(id) = &preset.feature {
                ensure!(features.contains(id.as_str()), "Unknown preset feature");
            }
        }
        Ok(())
    }

    pub fn choices(&self, previous: Option<&State>) -> BTreeMap<String, bool> {
        self.features
            .iter()
            .map(|f| {
                let value = previous
                    .and_then(|s| s.choices.get(&f.id))
                    .copied()
                    .unwrap_or(f.default);
                (f.id.clone(), value)
            })
            .collect()
    }

    pub fn selected(&self, choices: &BTreeMap<String, bool>) -> Result<Vec<ModFile>> {
        let mut enabled: BTreeSet<String> = choices
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, _)| k.clone())
            .collect();
        loop {
            let count = enabled.len();
            for f in &self.features {
                if enabled.contains(&f.id) {
                    enabled.extend(f.requires.iter().cloned());
                }
            }
            if count == enabled.len() {
                break;
            }
        }
        ensure!(
            enabled
                .iter()
                .all(|id| self.features.iter().any(|f| &f.id == id)),
            "Unknown feature choice"
        );
        Ok(self
            .mods
            .iter()
            .filter(|m| m.feature.as_ref().is_none_or(|id| enabled.contains(id)))
            .cloned()
            .collect())
    }
}

pub fn access_request(name: &str) -> Result<String> {
    ensure!(
        (3..=16).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "Use an exact Minecraft nickname: 3–16 letters, digits or underscores"
    );
    let mut digest: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    digest[6] = (digest[6] & 0x0f) | 0x30;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    let uuid = uuid::Uuid::from_bytes(digest);
    Ok(format!(
        "Forever SMP — access request / заявка\nName / Ник: {name}\nOffline UUID: {uuid}\n\n{{\"uuid\":\"{uuid}\",\"name\":\"{name}\"}}\n\nOwner approval is required. No passwords belong in this request.\nНужно одобрение владельца. Не добавляйте пароли в эту заявку.\n"
    ))
}

pub fn github_feed(repository: &str) -> Result<String> {
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || *s == "."
                || *s == ".."
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
    {
        bail!("Expected GitHub owner/repository");
    }
    Ok(format!(
        "https://github.com/{repository}/releases/latest/download/release.signed.json"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    #[test]
    fn signatures_reject_changed_payload_and_wrong_key() -> Result<()> {
        let key = SigningKey::from_bytes(&[17; 32]);
        let release = Release {
            schema: 1,
            pack_id: PACK_ID.into(),
            version: "0.1.12".into(),
            minecraft: "26.3".into(),
            fabric: "0.19.5".into(),
            java: 25,
            notes_en: "Public".into(),
            notes_ru: "Публичное".into(),
            features: vec![],
            mods: vec![],
            presets: vec![],
        };
        let payload = serde_json::to_string(&release)?;
        let mut envelope = SignedRelease {
            signature: hex::encode(key.sign(payload.as_bytes()).to_bytes()),
            payload,
        };
        let public = hex::encode(key.verifying_key().to_bytes());
        let verified = verify_release(&serde_json::to_vec(&envelope)?, &public)?;
        assert_eq!(verified.version, "0.1.12");
        assert_eq!(verified.notes_en, release.notes_en);
        assert_eq!(verified.notes_ru, release.notes_ru);
        envelope.payload = envelope.payload.replace("Public", "Changed");
        assert!(verify_release(&serde_json::to_vec(&envelope)?, &public).is_err());
        assert!(verify_release(&serde_json::to_vec(&envelope)?, &"00".repeat(32)).is_err());
        assert!(github_feed("owner/repo/../evil").is_err());
        Ok(())
    }
}
