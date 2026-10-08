//! GitHub's CLI on a machine that hasn't got it (James, Pointman thread 649 [4500]: the extension installs
//! gh into the nodes): the latest release from github.com/cli/cli for this platform, checked against the
//! release's own checksums, unpacked into the extension's state folder. No Homebrew, no sudo, nothing
//! outside that folder.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{bail, Context};
use serde_json::Value;
use sha2::{Digest, Sha256};

const RELEASES: &str = "https://api.github.com/repos/cli/cli/releases/latest";
/// gh's archives are about 15 MB; anything much bigger isn't one.
const ARCHIVE_MAX: u64 = 200 * 1024 * 1024;

/// This platform's archive, as gh's releases name it: `gh_<version>_<this>`.
pub fn asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macOS_arm64.zip"),
        ("macos", "x86_64") => Some("macOS_amd64.zip"),
        ("linux", "x86_64") => Some("linux_amd64.tar.gz"),
        ("linux", "aarch64") => Some("linux_arm64.tar.gz"),
        _ => None,
    }
}

/// The gh this extension installed, if it has: `<state>/cli/current/bin/gh`.
pub fn installed(state: &Path) -> Option<PathBuf> {
    let gh = state.join("cli").join("current").join("bin").join("gh");
    gh.is_file().then_some(gh)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(300)).user_agent("pointman-gh").build()
}

fn bytes(url: &str, max: u64) -> anyhow::Result<Vec<u8>> {
    let r = agent().get(url).call().with_context(|| format!("downloading {url}"))?;
    let mut out = Vec::new();
    r.into_reader().take(max + 1).read_to_end(&mut out)?;
    if out.len() as u64 > max {
        bail!("{url} is bigger than it should be");
    }
    Ok(out)
}

/// The checksum `checksums` gives for `name` (sha256sum's lines: `<hex>  <name>`).
pub fn checksum_for<'a>(checksums: &'a str, name: &str) -> Option<&'a str> {
    checksums.lines().find_map(|l| {
        let (sum, file) = l.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then_some(sum)
    })
}

/// Install the latest gh into `<state>/cli/<version>`, and point `<state>/cli/current` at it. Returns its
/// path and version.
pub fn install(state: &Path) -> anyhow::Result<(PathBuf, String)> {
    let suffix = asset_suffix().context("gh has no build for this platform")?;
    let release: Value = agent().get(RELEASES).call().context("asking GitHub for gh's latest release")?.into_json()?;
    let tag = release["tag_name"].as_str().context("gh's latest release has no tag")?;
    let version = tag.trim_start_matches('v').to_string();
    if !version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        bail!("gh's latest release has an odd tag: {tag}");
    }
    let assets = release["assets"].as_array().cloned().unwrap_or_default();
    let url_of = |name: &str| {
        assets.iter().find(|a| a["name"] == name).and_then(|a| a["browser_download_url"].as_str().map(String::from))
    };
    let archive_name = format!("gh_{version}_{suffix}");
    let sums_name = format!("gh_{version}_checksums.txt");
    let (Some(archive_url), Some(sums_url)) = (url_of(&archive_name), url_of(&sums_name)) else {
        bail!("gh {version} has no {archive_name} with its checksums");
    };
    let sums = String::from_utf8(bytes(&sums_url, 1024 * 1024)?)?;
    let want = checksum_for(&sums, &archive_name).context("gh's checksums don't list its archive")?.to_lowercase();
    let archive = bytes(&archive_url, ARCHIVE_MAX)?;
    let got = format!("{:x}", Sha256::digest(&archive));
    if got != want {
        bail!("gh {version}'s archive doesn't match its checksum: not installed");
    }

    let cli = state.join("cli");
    let unpack = cli.join(format!(".unpack-{version}"));
    let _ = fs::remove_dir_all(&unpack);
    fs::create_dir_all(&unpack)?;
    let file = unpack.join(&archive_name);
    fs::write(&file, &archive)?;
    // macOS's tar (bsdtar) reads zip too; Linux's archives are tar.gz
    let ok = Command::new("tar").arg("-xf").arg(&file).arg("-C").arg(&unpack).status().context("running tar")?;
    if !ok.success() {
        bail!("couldn't unpack {archive_name}");
    }
    let stem = archive_name.trim_end_matches(".zip").trim_end_matches(".tar.gz");
    let bin = unpack.join(stem).join("bin").join("gh");
    if !bin.is_file() {
        bail!("{archive_name} has no bin/gh");
    }
    let to = cli.join(&version);
    let _ = fs::remove_dir_all(&to);
    fs::create_dir_all(to.join("bin"))?;
    fs::rename(&bin, to.join("bin").join("gh"))?;
    let _ = fs::remove_dir_all(&unpack);
    let current = cli.join("current");
    let link = cli.join(".current.new");
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&version, &link)?;
    fs::rename(&link, &current)?;
    Ok((current.join("bin").join("gh"), version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        let sums = "abc  gh_2.62.0_linux_amd64.tar.gz\ndef  gh_2.62.0_macOS_arm64.zip\n";
        assert_eq!(checksum_for(sums, "gh_2.62.0_macOS_arm64.zip"), Some("def"));
        assert_eq!(checksum_for(sums, "gh_2.62.0_macOS_amd64.zip"), None);
        assert!(asset_suffix().is_some());
    }
}

#[cfg(test)]
mod real {
    /// The real download: `cargo test -- --ignored` (online).
    #[test]
    #[ignore]
    fn installs_the_latest_gh() {
        let dir = std::env::temp_dir().join(format!("pointman-gh-install-{}", std::process::id()));
        let (gh, version) = super::install(&dir).unwrap();
        let out = std::process::Command::new(&gh).arg("--version").output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains(&version), "{out:?}");
        assert_eq!(super::installed(&dir), Some(gh));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
