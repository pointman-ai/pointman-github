//! The extension's process: the node starts it (`bin/gh`, from the extension's folder) and talks to it
//! over stdin and stdout. It installs GitHub's CLI where there's none, and `gh.view {url}` answers with
//! what a GitHub link shows.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use gh::{find_gh, install, parse, GitHub, Kept};
use pointman_extension::{Extension, Handle, Job};

fn state(h: &Handle) -> PathBuf {
    PathBuf::from(h.node()["state_dir"].as_str().unwrap_or_default())
}

fn cli(h: &Handle) -> Option<PathBuf> {
    let state = state(h);
    find_gh(h.settings()["cli"].as_str().unwrap_or_default(), state.is_absolute().then_some(state.as_path()))
}

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let h = ext.handle();
    let kept = Arc::new(Kept::default());

    // a machine without gh gets its own, in the state folder (James, Pointman thread 649 [4500])
    ext.on_start(|h: &Handle| {
        let to = state(h);
        if cli(h).is_none() && to.is_absolute() {
            let h = h.clone();
            std::thread::spawn(move || match install::install(&to) {
                Ok((gh, version)) => h.log(format!("installed gh {version} at {}", gh.display())),
                Err(e) => h.log(format!("couldn't install gh: {e:#}")),
            });
        }
    });

    ext.command("gh.view", move |job: &Job| {
        let link = parse(job.params["url"].as_str().context("url is missing")?)?;
        if let Some(v) = kept.get(&link) {
            return Ok(v);
        }
        let gh = cli(&h).context("GitHub's CLI isn't on this machine yet: it's being installed, or set its path")?;
        let v = GitHub::new(gh).view(&link)?;
        kept.put(&link, &v);
        Ok(v)
    });

    ext.run()
}
