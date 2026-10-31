//! GitHub on Pointman's machines (James, Pointman thread 649 [4443], [4500]): GitHub's CLI on each node,
//! installed where there's none (`install`), and agents' runs acting on GitHub as Pointman App (`app`), else as this
//! machine's own `gh` login (MB-707, EXT-116: the vault's accounts and their sign-in went in 0.10.0). GitHub's pages
//! open in the apps' web panel (James, Pointman thread 8446): the viewer and its pane that were here went in 0.6.0.

pub mod app;
pub mod install;

use std::path::{Path, PathBuf};

/// Where `gh` is: the setting, else on the PATH, else where Homebrew puts it, else the one this extension
/// installed in its state folder.
pub fn find_gh(setting: &str, state: Option<&Path>) -> Option<PathBuf> {
    if !setting.trim().is_empty() {
        return Some(PathBuf::from(setting.trim()));
    }
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("gh")).collect::<Vec<_>>())
        .unwrap_or_default();
    on_path
        .into_iter()
        .chain(["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"].map(PathBuf::from))
        .find(|p| p.is_file())
        .or_else(|| state.and_then(install::installed))
}
