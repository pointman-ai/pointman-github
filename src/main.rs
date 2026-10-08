//! The extension's process: the node starts it (`bin/gitview`, from the extension's folder) and talks to
//! it over stdin and stdout. Its one command, `gitview.view {url}`, answers with what the link shows.

use std::sync::Arc;

use anyhow::Context;
use gitview::{find_gh, parse, GitHub, Kept};
use pointman_extension::{Extension, Job};

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let h = ext.handle();
    let kept = Arc::new(Kept::default());

    ext.command("gitview.view", move |job: &Job| {
        let link = parse(job.params["url"].as_str().context("url is missing")?)?;
        if let Some(v) = kept.get(&link) {
            return Ok(v);
        }
        let setting = h.settings()["gh"].as_str().unwrap_or_default().to_string();
        let gh =
            find_gh(&setting).context("GitHub's CLI (gh) isn't on this machine: install it and run gh auth login")?;
        let v = GitHub::new(gh).view(&link)?;
        kept.put(&link, &v);
        Ok(v)
    });

    ext.run()
}
