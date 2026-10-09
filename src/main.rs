//! The extension's process: the node starts it (`bin/github`, from the extension's folder) and talks to it
//! over stdin and stdout. It installs GitHub's CLI where there's none, signs GitHub accounts in from the
//! apps into the vault, gives agents' runs the vault's token, and `github.view {url}` answers with what a
//! GitHub link shows. The `view` pane shows the same as blocks, and keeps it current; on an open PR its
//! buttons and review form approve, merge and review it on GitHub, as this machine's login.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use github::signin::SignIns;
use github::{find_gh, install, pane, parse, said, GitHub, Kept, Link};
use pointman_extension::{Extension, Handle, Job, Pane, Refused};
use regex::Regex;
use serde_json::{json, Value};

/// How often the vault's github.* names are looked at, and how long an opened token is kept. A list older
/// than LIST_EVERY is still used while it's asked for again behind (each ask is a round trip to core, 0.2 s
/// from the Mac, that a run's start or a pane's open waited on: Pointman thread 6216); one older than
/// OPEN_EVERY is asked for first.
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
    /// The list being asked for behind, None in it if the node couldn't say.
    asking: Option<Receiver<Option<Vec<String>>>>,
    opened: Option<(Instant, String, Option<String>)>,
}

/// What's done about the vault's list, from its age and whether it's being asked for already.
#[derive(Debug, PartialEq)]
enum Relist {
    Now,
    Behind,
    Kept,
}

fn relist(age: Option<Duration>, asking: bool) -> Relist {
    match age {
        None => Relist::Now,
        Some(age) if age > OPEN_EVERY => Relist::Now,
        Some(age) if age > LIST_EVERY && !asking => Relist::Behind,
        Some(_) => Relist::Kept,
    }
}

fn list(h: &Handle) -> Option<Vec<String>> {
    let got = h.request("secret.list", json!({"prefix": "github."})).ok()?;
    let mut names: Vec<String> = got["names"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
        .unwrap_or_default();
    names.sort();
    Some(names)
}

impl Vault {
    fn token(&mut self, h: &Handle) -> Option<String> {
        if let Some(asking) = &self.asking {
            match asking.try_recv() {
                Ok(got) => {
                    self.names = got.unwrap_or_else(|| self.names.clone());
                    self.asking = None;
                }
                Err(TryRecvError::Disconnected) => self.asking = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        match relist(self.listed.map(|t| t.elapsed()), self.asking.is_some()) {
            Relist::Now => {
                self.listed = Some(Instant::now());
                self.asking = None;
                if let Some(names) = list(h) {
                    self.names = names;
                }
            }
            Relist::Behind => {
                self.listed = Some(Instant::now());
                let (tx, rx) = mpsc::channel();
                self.asking = Some(rx);
                let h = h.clone();
                std::thread::spawn(move || tx.send(list(&h)));
            }
            Relist::Kept => {}
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

type Titles = Arc<Mutex<HashMap<String, String>>>;

/// A view on its pane: the blocks (show sends what changed) and the title, when it changed.
fn shown(p: &Pane, got: &Value, titles: &Titles) {
    p.show(pane::blocks(got));
    let title = pane::title(got);
    let mut titles = titles.lock().unwrap_or_else(|e| e.into_inner());
    if titles.get(p.id()) != Some(&title) && p.is_open() {
        p.set_title(&title);
        titles.insert(p.id().to_string(), title);
    }
}

/// A button or a form on a PR's or an issue's pane, as its page on GitHub does it: approve, merge or review
/// an open PR (on the head the pane showed); close or reopen, draft or ready, update or delete the branch;
/// its title, labels, assignees and review requests; a comment. GitHub does it as this machine's login (the
/// vault's account, else its own gh login), and the pane shows GitHub's answer at once. What GitHub refuses
/// comes back with its reason.
fn act(h: &Handle, vault: &Mutex<Vault>, kept: &Kept, titles: &Titles, p: &Pane, event: &Value) -> anyhow::Result<()> {
    let refuse = |why: String| -> anyhow::Result<()> { Err(Refused::new(why).into()) };
    let (kind, block) = (event["kind"].as_str().unwrap_or_default(), event["block"].as_str().unwrap_or_default());
    let pr_only = matches!((kind, block), ("tap", "pr-actions") | ("submit", "pr-review"));
    let page = matches!((kind, block), ("tap", "state") | ("submit", "meta") | ("submit", "comment"));
    if !pr_only && !page {
        return refuse(format!("the GitHub view has nothing to do for {kind} on {block}"));
    }
    let link = match parse(p.target()["url"].as_str().unwrap_or_default()) {
        Ok(link) if link.kind == "pull" || (page && link.kind == "issue") => link,
        _ if pr_only => return refuse("only a PR is reviewed or merged here".into()),
        _ => return refuse("only a PR or an issue is changed here".into()),
    };
    let shown_now = match view(h, vault, kept, &link, false) {
        Ok(v) => v,
        Err(e) => return refuse(format!("{e:#}")),
    };
    if pr_only && !shown_now["merge"].is_object() {
        return refuse(format!("PR #{} isn't open", link.reference));
    }
    let pull = link.kind == "pull";
    let sha = shown_now["ref"].as_str().unwrap_or_default().to_string();
    let number: u64 = link.reference.parse()?;
    let Some(gh) = cli(h) else { return refuse("GitHub's CLI isn't on this machine yet".into()) };
    let github = GitHub::new(gh).token(vault.lock().unwrap_or_else(|e| e.into_inner()).token(h));
    let value = &event["value"];
    let meta = &shown_now["meta"];
    let pr_id = meta["id"].as_str().unwrap_or_default();
    let done = match (block, value.as_str()) {
        ("pr-actions", Some("approve")) => github.review(&link.repo, number, "APPROVE", "", &sha),
        ("pr-actions", Some(b)) if b.starts_with("merge:") => {
            // a forced merge of a draft marks it ready first: GitHub merges no draft
            let merge = &shown_now["merge"];
            let ready = match merge["id"].as_str() {
                Some(id) if merge["draft"] == true => github.ready(id),
                _ => Ok(()),
            };
            ready.and_then(|_| github.merge(&link.repo, number, &b["merge:".len()..], &sha))
        }
        ("pr-review", _) => {
            let kind = value["event"].as_str().unwrap_or("COMMENT");
            let body = value["body"].as_str().unwrap_or_default();
            if kind != "APPROVE" && body.trim().is_empty() {
                return refuse("a comment or a change request needs words".into());
            }
            github.review(&link.repo, number, kind, body, &sha)
        }
        ("state", Some("close")) => github.set_state(&link.repo, number, pull, false, None),
        ("state", Some(b)) if b.starts_with("close:") && !pull => {
            github.set_state(&link.repo, number, false, false, Some(&b["close:".len()..]))
        }
        ("state", Some("reopen")) => github.set_state(&link.repo, number, pull, true, None),
        ("state", Some("draft")) if pull => github.draft(pr_id),
        ("state", Some("ready")) if pull => github.ready(pr_id),
        ("state", Some("update-branch")) if pull => github.update_branch(&link.repo, number, &sha),
        ("state", Some("delete-branch")) if pull && meta["head"]["same_repo"] == true => {
            github.delete_branch(&link.repo, meta["head"]["branch"].as_str().unwrap_or_default())
        }
        ("meta", _) => {
            // the title only when it changed; the rest as chosen
            let mut changes = value.clone();
            if changes["title"].as_str().map(str::trim) == meta["title"].as_str().map(str::trim) {
                changes["title"] = Value::Null;
            }
            github.edit_meta(&link.repo, number, &changes)
        }
        ("comment", _) => {
            let body = value["body"].as_str().map(str::trim).unwrap_or_default();
            if body.is_empty() {
                return refuse("a comment needs words".into());
            }
            github.comment(&link.repo, number, body)
        }
        _ => return refuse(format!("{value} isn't one of this page's buttons")),
    };
    if let Err(e) = done {
        return refuse(said(&e));
    }
    match view(h, vault, kept, &link, true) {
        Ok(got) => shown(p, &got, titles),
        Err(e) => h.log(format!("pane {}: couldn't ask GitHub again: {e:#}", p.id())),
    }
    Ok(())
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

    // the `view` pane: the same, as blocks (Pointman's docs/dev/panes.md), asked again each minute; show
    // sends only what changed, so a new comment slides in and the open tab stays put
    let titles: Titles = Arc::default();
    let (hh, v, k, t) = (h.clone(), vault.clone(), kept.clone(), titles.clone());
    ext.pane("view", move |p: &Pane| {
        let url = p.target()["url"].as_str().context("the GitHub view opens a GitHub link: its target has no url")?;
        let got = view(&hh, &v, &k, &parse(url)?, false)?;
        let title = pane::title(&got);
        t.lock().unwrap_or_else(|e| e.into_inner()).insert(p.id().to_string(), title.clone());
        let (ih, iv, ik, it) = (hh.clone(), v.clone(), k.clone(), t.clone());
        p.on_input(move |p: &Pane, event: &Value| -> anyhow::Result<()> {
            match event["kind"].as_str().unwrap_or_default() {
                "select" => Ok(()), // a tab: the app shows it
                "tap" | "submit" => act(&ih, &iv, &ik, &it, p, event),
                kind => Err(Refused::new(format!("the GitHub view has nothing to do for {kind}")).into()),
            }
        });
        let t = t.clone();
        p.on_close(move |p: &Pane, _: &str| {
            t.lock().unwrap_or_else(|e| e.into_inner()).remove(p.id());
        });
        Ok(json!({"title": title, "blocks": pane::blocks(&got)}))
    });
    let (v, k) = (vault, kept);
    ext.every(
        REFRESH_EVERY,
        move |h: &Handle| {
            for p in h.panes("view") {
                let Ok(link) = parse(p.target()["url"].as_str().unwrap_or_default()) else { continue };
                match view(h, &v, &k, &link, true) {
                    Ok(got) => shown(&p, &got, &titles),
                    Err(e) => h.log(format!("pane {}: couldn't ask GitHub again: {e:#}", p.id())),
                }
            }
        },
        "panes",
    );

    ext.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vaults_list_is_asked_for_first_then_behind_and_first_again_when_too_old() {
        let secs = |s: u64| Some(Duration::from_secs(s));
        assert_eq!(relist(None, false), Relist::Now);
        assert_eq!(relist(secs(10), false), Relist::Kept);
        assert_eq!(relist(secs(61), false), Relist::Behind);
        assert_eq!(relist(secs(61), true), Relist::Kept, "asked for once");
        assert_eq!(relist(secs(1801), true), Relist::Now);
    }
}
