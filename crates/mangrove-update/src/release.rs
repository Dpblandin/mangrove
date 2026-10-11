//! The latest release on GitHub and the archive this platform installs.
use crate::{http, Result, UpdateError, Version};
use serde::Deserialize;
use url::Url;

const LATEST_RELEASE: &str = "https://api.github.com/repos/Dpblandin/mangrove/releases/latest";
const MAX_RELEASE_BYTES: u64 = 1024 * 1024;
/// Far above the current archives, from 20 to 30 Mio, to refuse an absurd size.
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

/// A platform the release workflow builds an archive for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacosArm64,
    MacosX86_64,
    LinuxX86_64,
    WindowsX86_64,
}

impl Platform {
    /// The platform of this build, when releases include it.
    pub const fn current() -> Option<Self> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(Self::MacosArm64)
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some(Self::MacosX86_64)
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Some(Self::LinuxX86_64)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(Self::WindowsX86_64)
        } else {
            None
        }
    }

    /// The archive name given by the release workflow.
    pub fn archive_name(self, version: Version) -> String {
        let suffix = match self {
            Self::MacosArm64 => "macos-arm64.dmg",
            Self::MacosX86_64 => "macos-x86_64.dmg",
            Self::LinuxX86_64 => "linux-x86_64.tar.gz",
            Self::WindowsX86_64 => "windows-x86_64.zip",
        };
        format!("mangrove-{version}-{suffix}")
    }
}

/// The archive of a platform and its detached minisign signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: Url,
    pub size: u64,
    pub signature: Url,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release page, with its notes and every archive.
    pub page: Url,
    /// Missing when the release has no signed archive for this platform.
    pub asset: Option<Asset>,
}

/// The latest published release; GitHub leaves drafts and pre-releases out.
pub fn latest_release(platform: Option<Platform>) -> Result<Release> {
    let url = Url::parse(LATEST_RELEASE).map_err(|_| UpdateError::InvalidResponse)?;
    let response = http::get(&url, "application/vnd.github+json")?;
    parse_release(&http::read_body(response, MAX_RELEASE_BYTES)?, platform)
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn parse_release(body: &[u8], platform: Option<Platform>) -> Result<Release> {
    let release: GitHubRelease =
        serde_json::from_slice(body).map_err(|_| UpdateError::InvalidResponse)?;
    let version = release
        .tag_name
        .parse()
        .map_err(|_| UpdateError::InvalidResponse)?;
    let page = github_url(&release.html_url).ok_or(UpdateError::InvalidResponse)?;
    let asset = platform.and_then(|platform| {
        let name = platform.archive_name(version);
        let find = |wanted: &str| release.assets.iter().find(|asset| asset.name == wanted);
        let archive = find(&name)?;
        let signature = find(&format!("{name}.minisig"))?;
        Some(Asset {
            url: github_url(&archive.browser_download_url)?,
            size: Some(archive.size).filter(|size| (1..=MAX_ARCHIVE_BYTES).contains(size))?,
            signature: github_url(&signature.browser_download_url)?,
            name,
        })
    });
    Ok(Release {
        version,
        page,
        asset,
    })
}

fn github_url(text: &str) -> Option<Url> {
    Url::parse(text).ok().filter(http::is_github)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOWNLOAD: &str = "https://github.com/Dpblandin/mangrove/releases/download/v0.4.0";

    fn release_json(assets: &[(&str, u64)]) -> Vec<u8> {
        serde_json::json!({
            "tag_name": "v0.4.0",
            "html_url": "https://github.com/Dpblandin/mangrove/releases/tag/v0.4.0",
            "draft": false,
            "prerelease": false,
            "assets": assets
                .iter()
                .map(|(name, size)| serde_json::json!({
                    "name": name,
                    "size": size,
                    "browser_download_url": format!("{DOWNLOAD}/{name}"),
                }))
                .collect::<Vec<_>>(),
        })
        .to_string()
        .into_bytes()
    }

    fn version() -> Version {
        "0.4.0".parse().unwrap()
    }

    #[test]
    fn archive_names_follow_the_release_workflow() {
        assert_eq!(
            [
                Platform::MacosArm64,
                Platform::MacosX86_64,
                Platform::LinuxX86_64,
                Platform::WindowsX86_64,
            ]
            .map(|platform| platform.archive_name(version())),
            [
                "mangrove-0.4.0-macos-arm64.dmg",
                "mangrove-0.4.0-macos-x86_64.dmg",
                "mangrove-0.4.0-linux-x86_64.tar.gz",
                "mangrove-0.4.0-windows-x86_64.zip",
            ]
        );
    }

    #[test]
    fn a_release_offers_the_signed_archive_of_its_platform() {
        let body = release_json(&[
            ("mangrove-0.4.0-macos-arm64.dmg", 19_811_765),
            ("mangrove-0.4.0-macos-arm64.dmg.minisig", 312),
            ("mangrove-0.4.0-linux-x86_64.tar.gz", 26_315_569),
            ("mangrove-0.4.0-linux-x86_64.tar.gz.minisig", 316),
            ("SHA256SUMS.txt", 396),
        ]);
        let release = parse_release(&body, Some(Platform::MacosArm64)).unwrap();
        assert_eq!(release.version, version());
        assert_eq!(
            release.page.as_str(),
            "https://github.com/Dpblandin/mangrove/releases/tag/v0.4.0"
        );
        let asset = release.asset.unwrap();
        assert_eq!(asset.name, "mangrove-0.4.0-macos-arm64.dmg");
        assert_eq!(asset.size, 19_811_765);
        assert_eq!(
            asset.url.as_str(),
            format!("{DOWNLOAD}/mangrove-0.4.0-macos-arm64.dmg")
        );
        assert_eq!(
            asset.signature.as_str(),
            format!("{DOWNLOAD}/mangrove-0.4.0-macos-arm64.dmg.minisig")
        );
        let linux = parse_release(&body, Some(Platform::LinuxX86_64)).unwrap();
        assert_eq!(linux.asset.unwrap().size, 26_315_569);
    }

    #[test]
    fn unsigned_missing_or_doubtful_archives_are_not_offered() {
        let unsigned = release_json(&[("mangrove-0.4.0-windows-x86_64.zip", 18_744_037)]);
        let empty = release_json(&[
            ("mangrove-0.4.0-windows-x86_64.zip", 0),
            ("mangrove-0.4.0-windows-x86_64.zip.minisig", 312),
        ]);
        let huge = release_json(&[
            ("mangrove-0.4.0-windows-x86_64.zip", MAX_ARCHIVE_BYTES + 1),
            ("mangrove-0.4.0-windows-x86_64.zip.minisig", 312),
        ]);
        for body in [unsigned, empty, huge] {
            let release = parse_release(&body, Some(Platform::WindowsX86_64)).unwrap();
            assert_eq!(release.version, version());
            assert_eq!(release.asset, None);
        }
        let other_platform = release_json(&[
            ("mangrove-0.4.0-macos-arm64.dmg", 1),
            ("mangrove-0.4.0-macos-arm64.dmg.minisig", 1),
        ]);
        for platform in [Some(Platform::MacosX86_64), None] {
            assert_eq!(
                parse_release(&other_platform, platform).unwrap().asset,
                None
            );
        }
        let elsewhere = serde_json::json!({
            "tag_name": "v0.4.0",
            "html_url": "https://github.com/Dpblandin/mangrove/releases/tag/v0.4.0",
            "assets": [
                {"name": "mangrove-0.4.0-macos-arm64.dmg", "size": 1,
                 "browser_download_url": "https://example.com/mangrove-0.4.0-macos-arm64.dmg"},
                {"name": "mangrove-0.4.0-macos-arm64.dmg.minisig", "size": 1,
                 "browser_download_url": format!("{DOWNLOAD}/mangrove-0.4.0-macos-arm64.dmg.minisig")},
            ],
        });
        let release =
            parse_release(elsewhere.to_string().as_bytes(), Some(Platform::MacosArm64)).unwrap();
        assert_eq!(release.asset, None);
    }

    #[test]
    fn malformed_releases_are_refused() {
        for body in [
            "not json".to_string(),
            serde_json::json!({"html_url": "https://github.com/Dpblandin/mangrove"}).to_string(),
            serde_json::json!({"tag_name": "nightly", "html_url": "https://github.com/x"})
                .to_string(),
            serde_json::json!({"tag_name": "v0.4.0", "html_url": "https://example.com/x"})
                .to_string(),
        ] {
            assert!(matches!(
                parse_release(body.as_bytes(), Platform::current()),
                Err(UpdateError::InvalidResponse)
            ));
        }
    }
}
