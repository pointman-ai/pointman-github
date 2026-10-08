//! The extension's process: the node starts it (`bin/github`, from the extension's folder) and talks to it
//! over stdin and stdout. It installs GitHub's CLI where there's none, signs GitHub accounts in from the
//! apps into the vault, gives agents' runs the vault's token, and `github.view {url}` answers with what a
//! GitHub link shows. The `view` pane shows the same as blocks, and keeps it current.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use github::signin::SignIns;
use github::{find_gh, install, pane, parse, GitHub, Kept, Link};
use pointman_extension::{Extension, Handle, Job, Pane, Refused};
use regex::Regex;
use serde_json::{json, Value};

/// How often the vault's github.* names are looked at, and how long an opened token is kept.
const LIST_EVERY: Duration = Duration::from_secs(60);
const OPEN_EVERY: Duration = Duration::from_secs(1800);
/// How often an open `view` pane asks GitHub again.
const REFRESH_EVERY: Duration = Duration::from_secs(60);

fn state(h: &Handle) -> PathBuf {
    PathBuf::from(h.node()["state_dir"].as_str().unwrap_or_default())
}

fn cli(h: &Handle) -> Option<PathBuf> {
    let state = state(h);
    find_gh(h.settings()["cli"].as_str().unwrap_or_default(), state.is_absolute().then_some(state.as_path()))
}

/// The vault's GitHub token for this machine (`github.<label>`, the first by label), as the node lets this
/// extension see github.* only. None while there's none, and the machine's own gh login is used.
#[derive(Default)]
struct Vault {
    listed: Option<Instant>,
    names: Vec<String>,
    opened: Option<(Instant, String, Option<String>)>,
}

impl Vault {
    fn token(&mut self, h: &Handle) -> Option<String> {
        if self.listed.is_none_or(|t| t.elapsed() > LIST_EVERY) {
            self.listed = Some(Instant::now());
            if let Ok(got) = h.request("secret.list", json!({"prefix": "github."})) {
                let mut names: Vec<String> = got["names"]
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                    .unwrap_or_default();
                names.sort();
                self.names = names;
            }
        }
        let name = self.names.first()?.clone();
        if let Some((at, n, token)) = &self.opened {
            if *n == name && at.elapsed() < OPEN_EVERY {
                return token.clone();
            }
        }
        let found = Regex::new(r"(gh[pousr]_[A-Za-z0-9]{36,251}|github_pat_[A-Za-z0-9_]{22,255})").expect("compiles");
        let token = match h.secret(&name) {
            Ok(value) => found.find(&value).map(|m| m.as_str().to_string()),
            Err(e) => {
                h.log(format!("couldn't open {name}: {e}"));
                None
            }
        };
        self.opened = Some((Instant::now(), name, token.clone()));
        token
    }
}

/// What a link shows: kept for a minute unless `fresh`, asked with the vault's account, else this
/// machine's own gh login.
fn view(h: &Handle, vault: &Mutex<Vault>, kept: &Kept, link: &Link, fresh: bool) -> anyhow::Result<Value> {
    if let Some(got) = kept.get(link).filter(|_| !fresh) {
        return Ok(got);
    }
    let gh = cli(h).context("GitHub's CLI isn't on this machine yet: it's being installed, or set its path")?;
    let token = vault.lock().unwrap_or_else(|e| e.into_inner()).token(h);
    let got = GitHub::new(gh).token(token).view(link)?;
    kept.put(link, &got);
    Ok(got)
}

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let h = ext.handle();
    let kept = Arc::new(Kept::default());
    let vault = Arc::new(Mutex::new(Vault::default()));
    let signins = Arc::new(SignIns::default());

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

    // agents' runs get the vault's token, so `gh` works for them on every node ([sessions] in extension.toml)
    let (hh, v) = (h.clone(), vault.clone());
    ext.method("session.prepare", move |_: Value| {
        let token = v.lock().unwrap_or_else(|e| e.into_inner()).token(&hh);
        Ok(match token {
            Some(t) => json!({"env": {"GH_TOKEN": t}}),
            None => json!({}),
        })
    });
    ext.method("session.ended", |_: Value| Ok(json!({})));

    // Sign in from the apps (MB-216's sheet): GitHub's device flow
    let (hh, s) = (h.clone(), signins.clone());
    ext.method("credentials.sign_in", move |p: Value| {
        if p["form"] != "account" {
            bail!("the GitHub extension signs in GitHub accounts only (form: account)");
        }
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine yet: it's being installed")?;
        s.start(&gh, p["session"].as_str().unwrap_or_default(), Duration::from_secs(30))
    });
    let (hh, s) = (h.clone(), signins);
    ext.method("credentials.code", move |p: Value| {
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine")?;
        s.code(
            &gh,
            p["session"].as_str().unwrap_or_default(),
            p["code"].as_str().unwrap_or_default(),
            Duration::from_secs(120),
        )
    });

    // `/github add` posted (an agent, an older app): the apps open the account form for it
    ext.command("github.add", |_: &Job| {
        Ok(json!({"text": "A GitHub account is added in the Pointman app, so its token never goes in a post: \
                           type `/github add` there and tap Sign in."}))
    });

    let (hh, v, k) = (h.clone(), vault.clone(), kept.clone());
    ext.command("github.view", move |job: &Job| {
        let link = parse(job.params["url"].as_str().context("url is missing")?)?;
        view(&hh, &v, &k, &link, false)
    });

    // the `view` pane: the same, as blocks (Pointman's docs/dev/panes.md), each open one's last content
    // kept so a refresh sends only what changed
    let shown: Arc<Mutex<HashMap<String, Value>>> = Arc::default();
    let (hh, v, k, s) = (h.clone(), vault.clone(), kept.clone(), shown.clone());
    ext.pane("view", move |p: &Pane| {
        let url = p.target()["url"].as_str().context("the GitHub view opens a GitHub link: its target has no url")?;
        let got = view(&hh, &v, &k, &parse(url)?, false)?;
        let content = pane::blocks(&got);
        s.lock().unwrap_or_else(|e| e.into_inner()).insert(p.id().to_string(), content.clone());
        p.on_input(|_: &Pane, event: &Value| -> anyhow::Result<()> {
            match event["kind"].as_str().unwrap_or_default() {
                "select" => Ok(()), // a tab: the app shows it
                kind => Err(Refused::new(format!("the GitHub view has nothing to do for {kind}")).into()),
            }
        });
        let s = s.clone();
        p.on_close(move |p: &Pane, _: &str| {
            s.lock().unwrap_or_else(|e| e.into_inner()).remove(p.id());
        });
        Ok(json!({"title": pane::title(&got), "blocks": content}))
    });
    let (v, k) = (vault, kept);
    ext.every(
        REFRESH_EVERY,
        move |h: &Handle| {
            for p in h.panes("view") {
                let Ok(link) = parse(p.target()["url"].as_str().unwrap_or_default()) else { continue };
                let got = match view(h, &v, &k, &link, true) {
                    Ok(got) => got,
                    Err(e) => {
                        h.log(format!("pane {}: couldn't ask GitHub again: {e:#}", p.id()));
                        continue;
                    }
                };
                let new = pane::blocks(&got);
                let mut shown = shown.lock().unwrap_or_else(|e| e.into_inner());
                let Some(old) = shown.get(p.id()) else { continue }; // still opening, or closed
                if *old == new {
                    continue;
                }
                match pane::changes(old, &new) {
                    Some(ops) => p.patch(ops),
                    None => p.set(&new),
                }
                if p.closed().is_none() {
                    shown.insert(p.id().to_string(), new);
                }
            }
        },
        "panes",
    );

    ext.run()
}
