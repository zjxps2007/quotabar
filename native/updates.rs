use crate::files::invalid;
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt,
    fs::File,
    io::{self, Read},
    path::Path,
    str::FromStr,
};

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;

const REPOSITORY: &str = "https://github.com/zjxps2007/quotabar";
pub const PUBLIC_KEY: &str = include_str!("../shared/assets/UpdatePublicKey.pem");
pub const MAX_FEED: usize = 512 * 1024;
pub const MAX_MANIFEST: usize = 128 * 1024;
const MAX_ASSET: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Version(pub i32, pub i32, pub i32);

impl FromStr for Version {
    type Err = io::Error;
    fn from_str(input: &str) -> io::Result<Self> {
        let mut parts = input.strip_prefix('v').unwrap_or(input).split('.');
        let mut number = || {
            let field = parts.next().ok_or_else(|| invalid("Invalid version"))?;
            if field.is_empty()
                || field.len() > 10
                || field.len() > 1 && field.starts_with('0')
                || !field.bytes().all(|c| c.is_ascii_digit())
            {
                return Err(invalid("Invalid version"));
            }
            field.parse().map_err(|_| invalid("Invalid version"))
        };
        let parsed = Self(number()?, number()?, number()?);
        if parts.next().is_some() {
            return Err(invalid("Invalid version"));
        }
        Ok(parsed)
    }
}
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[derive(Clone, Copy)]
pub enum Platform {
    MacArm64,
    MacX64,
    WindowsInstaller,
    WindowsPortable,
}
impl Platform {
    pub fn asset_name(self, version: Version) -> String {
        let suffix = match self {
            Self::MacArm64 => "macOS-arm64.zip",
            Self::MacX64 => "macOS-x86_64.zip",
            Self::WindowsInstaller => "Windows-x64-Setup.exe",
            Self::WindowsPortable => "Windows-x64.zip",
        };
        format!("QuotaBar-{version}-{suffix}")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub page: String,
    pub asset: Option<Asset>,
}
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct Verified {
    pub release: Release,
    pub build: i32,
    pub asset: Asset,
}

#[derive(Serialize, Deserialize)]
struct InstallResult {
    version: String,
    success: bool,
    message: String,
}

fn verified_stage(
    update: &Verified,
    platform: Platform,
    current: &str,
    stage: &Path,
) -> io::Result<()> {
    use crate::files::read_file;
    let manifest = stage.join("update-manifest.json");
    let signature = stage.join("update-manifest.sig");
    #[cfg(windows)]
    for path in [&manifest, &signature] {
        crate::files::reject_links(path)?;
    }
    let checked = verify_manifest(
        &update.release,
        platform,
        current,
        &read_file(&manifest, MAX_MANIFEST)?,
        &read_file(&signature, 128)?,
        PUBLIC_KEY,
    )?;
    if checked != *update {
        return Err(invalid("Staged update mismatch"));
    }
    let archive = stage.join(&checked.asset.name);
    #[cfg(windows)]
    crate::files::reject_links(&archive)?;
    verify_file(&archive, &checked.asset)
}

impl Release {
    fn version(&self) -> io::Result<Version> {
        let version: Version = self.tag.parse()?;
        if self.version != version.to_string()
            || self.page != format!("{REPOSITORY}/releases/tag/{}", self.tag)
        {
            return Err(invalid("Unexpected release location or version"));
        }
        Ok(version)
    }
    fn validate_asset(&self, platform: Platform) -> io::Result<&Asset> {
        let name = platform.asset_name(self.version()?);
        let asset = self
            .asset
            .as_ref()
            .ok_or_else(|| invalid("Missing release asset"))?;
        if asset.name != name
            || asset.url != format!("{REPOSITORY}/releases/download/{}/{name}", self.tag)
            || !(1..=MAX_ASSET).contains(&asset.bytes)
            || asset.sha256.len() != 64
            || !asset
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("Invalid release asset"));
        }
        Ok(asset)
    }
}

pub fn parse_feed(bytes: &[u8], platform: Platform) -> io::Result<Option<Release>> {
    use serde_json::Value;
    if bytes.len() > MAX_FEED {
        return Err(invalid("Oversized release feed"));
    }
    let value: Value = serde_json::from_slice(bytes)?;
    for key in ["draft", "prerelease"] {
        if value[key]
            .as_bool()
            .ok_or_else(|| invalid("Invalid release state"))?
        {
            return Ok(None);
        }
    }
    let string = |val: &Value| {
        val.as_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid("Invalid release field"))
    };
    let tag = string(&value["tag_name"])?;
    let version = tag.parse::<Version>()?.to_string();
    let mut release = Release {
        tag,
        version,
        page: string(&value["html_url"])?,
        asset: None,
    };
    let name = platform.asset_name(release.version()?);
    let assets = value["assets"]
        .as_array()
        .ok_or_else(|| invalid("Invalid release assets"))?;
    let mut matches = assets.iter().filter(|a| a["name"].as_str() == Some(&name));
    if let Some(asset) = matches.next() {
        if matches.next().is_some() || asset["state"] != "uploaded" {
            return Err(invalid("Invalid release asset state"));
        }
        let sha256 = asset["digest"]
            .as_str()
            .and_then(|s| s.strip_prefix("sha256:"))
            .ok_or_else(|| invalid("Missing release asset digest"))?
            .to_ascii_lowercase();
        release.asset = Some(Asset {
            name,
            url: string(&asset["browser_download_url"])?,
            bytes: asset["size"]
                .as_u64()
                .ok_or_else(|| invalid("Invalid release asset size"))?,
            sha256,
        });
        release.validate_asset(platform)?;
    }
    Ok(Some(release))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: i32,
    version: String,
    build: i32,
    assets: Vec<SignedAsset>,
}
#[derive(Deserialize)]
struct SignedAsset {
    name: String,
    bytes: u64,
    sha256: String,
}

pub fn verify_manifest(
    release: &Release,
    platform: Platform,
    installed: &str,
    bytes: &[u8],
    signature: &[u8],
    pem: &str,
) -> io::Result<Verified> {
    if bytes.len() > MAX_MANIFEST
        || !(8..=128).contains(&signature.len())
        || release.version()? <= installed.parse()?
    {
        return Err(invalid("Invalid update version or signature"));
    }
    let asset = release.validate_asset(platform)?;
    let key = VerifyingKey::from_public_key_pem(pem).map_err(|_| invalid("Invalid update key"))?;
    let signature =
        Signature::from_der(signature).map_err(|_| invalid("Invalid update signature"))?;
    key.verify(bytes, &signature)
        .map_err(|_| invalid("Update signature mismatch"))?;
    let manifest: Manifest = serde_json::from_slice(bytes)?;
    let mut matches = manifest.assets.iter().filter(|a| a.name == asset.name);
    let signed = matches
        .next()
        .ok_or_else(|| invalid("Missing signed asset"))?;
    if manifest.schema_version != 1
        || manifest.version != release.version
        || manifest.build <= 0
        || matches.next().is_some()
        || signed.bytes != asset.bytes
        || signed.sha256 != asset.sha256
    {
        return Err(invalid("Update manifest mismatch"));
    }
    Ok(Verified {
        release: release.clone(),
        build: manifest.build,
        asset: asset.clone(),
    })
}

pub fn verify_file(path: &Path, asset: &Asset) -> io::Result<()> {
    let file = File::open(path)?;
    if file.metadata()?.len() != asset.bytes {
        return Err(invalid("Update size mismatch"));
    }
    let mut hash = Sha256::new();
    let read = io::copy(&mut file.take(asset.bytes + 1), &mut hash)?;
    if read != asset.bytes || format!("{:x}", hash.finalize()) != asset.sha256 {
        return Err(invalid("Update digest mismatch"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const FEED: &[u8] = include_bytes!("../shared/fixtures/update-release.json");
    const MANIFEST: &[u8] = include_bytes!("../shared/fixtures/update-manifest.json");
    const SIGNATURE: &[u8] = include_bytes!("../shared/fixtures/update-manifest.sig");
    const KEY: &str = include_str!("../shared/fixtures/update-test-public.pem");

    #[test]
    fn signed_release_contract() {
        for platform in [
            Platform::WindowsInstaller,
            Platform::WindowsPortable,
            Platform::MacArm64,
        ] {
            let release = parse_feed(FEED, platform).unwrap().unwrap();
            assert_eq!(
                verify_manifest(&release, platform, "0.1.9", MANIFEST, SIGNATURE, KEY)
                    .unwrap()
                    .build,
                7
            );
            assert!(
                verify_manifest(&release, platform, "0.1.10", MANIFEST, SIGNATURE, KEY).is_err()
            );
            let mut changed = MANIFEST.to_vec();
            changed.push(b' ');
            assert!(
                verify_manifest(&release, platform, "0.1.4", &changed, SIGNATURE, KEY).is_err()
            );
            let mut forged = release.clone();
            forged.asset.as_mut().unwrap().bytes += 1;
            assert!(verify_manifest(&forged, platform, "0.1.4", MANIFEST, SIGNATURE, KEY).is_err());
        }
        assert!(
            parse_feed(FEED, Platform::MacX64)
                .unwrap()
                .unwrap()
                .asset
                .is_none()
        );
    }
}
