//! The extension's process: the node starts it (`bin/github`, from the extension's folder) and talks to it
//! over stdin and stdout. It installs GitHub's CLI where there's none, signs GitHub accounts in from the
//! apps into the vault, and gives agents' runs the vault's token. GitHub's pages open in the apps' web panel
//! (James, Pointman thread 8446).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use github::signin::SignIns;
use github::{find_gh, install};
use pointman_extension::protocol::{
    CredentialsCodeRequest, CredentialsCodeResponse, CredentialsSignInRequest, CredentialsWaitRequest,
    CredentialsWaitResponse, SessionEndedRequest, SessionEndedResponse, SessionPrepareRequest, SessionPrepareResponse,
};
use pointman_extension::{Extension, Handle, Job};
use regex::Regex;
use serde_json::json;

/// How often the vault's github.* names are looked at, and how long an opened token is kept. A list older
/// than LIST_EVERY is still used while it's asked for again behind (each ask is a round trip to core, 0.2 s
/// from the Mac, that a run's start or a pane's open waited on: Pointman thread 6216); one older than
/// OPEN_EVERY is asked for first.
const LIST_EVERY: Duration = Duration::from_secs(60);
const OPEN_EVERY: Duration = Duration::from_secs(1800);

fn state(h: &Handle) -> PathBuf {
    h.state_dir()
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
    let mut names = h.secrets("github.").ok()?;
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

fn main() -> anyhow::Result<()> {
    let mut ext = Extension::new()?;
    let h = ext.handle();
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
    ext.on(move |_: SessionPrepareRequest| {
        let mut answer = SessionPrepareResponse::default();
        if let Some(token) = v.lock().unwrap_or_else(|e| e.into_inner()).token(&hh) {
            answer.env.insert("GH_TOKEN".to_string(), token);
        }
        Ok(answer)
    });
    ext.on(|_: SessionEndedRequest| Ok(SessionEndedResponse::default()));

    // Sign in from the apps (MB-216's sheet): GitHub's device flow
    let (hh, s) = (h.clone(), signins.clone());
    ext.on(move |p: CredentialsSignInRequest| {
        if p.form != "account" {
            bail!("the GitHub extension signs in GitHub accounts only (form: account)");
        }
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine yet: it's being installed")?;
        let session = p.session.as_option().map(|s| s.value.as_str()).unwrap_or_default();
        s.start(&gh, session, Duration::from_secs(30))
    });
    let (hh, s) = (h.clone(), signins.clone());
    ext.on(move |p: CredentialsWaitRequest| {
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine")?;
        let session = p.session.as_option().map(|s| s.value.as_str()).unwrap_or_default();
        let value = s.wait(&gh, session, github::signin::WAIT)?;
        Ok(CredentialsWaitResponse { value, ..Default::default() })
    });
    let (hh, s) = (h.clone(), signins);
    ext.on(move |p: CredentialsCodeRequest| {
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine")?;
        let session = p.session.as_option().map(|s| s.value.as_str()).unwrap_or_default();
        let value = s.code(&gh, session, &p.code, Duration::from_secs(120))?;
        Ok(CredentialsCodeResponse { value, ..Default::default() })
    });

    // `/github add` posted (an agent, an older app): the apps open the account form for it
    ext.command("github.add", |_: &Job| {
        Ok(json!({"text": "A GitHub account is added in the Pointman app, so its token never goes in a post: \
                           type `/github add` there and tap Sign in."}))
    });

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
