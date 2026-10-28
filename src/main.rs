//! The extension's process: the node starts it (`bin/github`, from the extension's folder) and talks to it
//! over stdin and stdout. It installs GitHub's CLI where there's none, and has agents' runs act on GitHub as
//! Pointman App, with tokens from the board kept fresh (`app`); before the board has the App, it signs GitHub
//! accounts in from the apps into the vault and gives runs the vault's token. GitHub's pages open in the apps' web
//! panel (James, Pointman thread 8446).
//!
//! A run's `gh` and git call it back as `bin/github token <state> -- <gh's arguments>` and
//! `bin/github git-credential <state> <get|store|erase>`: those answer from the state folder and exit.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use github::app::{self, App, Kept, Token};
use github::signin::SignIns;
use github::{find_gh, install};
use pointman_extension::convert;
use pointman_extension::protocol::{
    CoreCallRequest, CredentialsCodeRequest, CredentialsCodeResponse, CredentialsSignInRequest, CredentialsWaitRequest,
    CredentialsWaitResponse, SessionEndedRequest, SessionEndedResponse, SessionPrepareRequest, SessionPrepareResponse,
};
use pointman_extension::{Extension, Handle, Job};
use regex::Regex;
use serde_json::{json, Value};

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

// ------------------------------------------------------------------------------------------------ Pointman App

/// How often the board is asked whether it has Pointman App and where it's installed, and how often the tokens are
/// looked at (each is fetched again when it has less than app::RENEW_SECS left).
const APP_EVERY: Duration = Duration::from_secs(300);
const TOKENS_EVERY: Duration = Duration::from_secs(60);

/// The App as the board last said, and when it was asked.
#[derive(Default)]
struct Board {
    asked: Option<Instant>,
    app: Option<App>,
}

/// One call to the board through the node (`core.call`, which lets this extension ask the two GitHub routes): its
/// status and JSON answer.
fn call(h: &Handle, method: &str, path: &str, body: Option<Value>) -> anyhow::Result<(i32, Value)> {
    let mut ask = CoreCallRequest { method: method.into(), path: path.into(), ..Default::default() };
    if let Some(body) = body {
        ask.body = convert::to_proto(&body).into();
    }
    let got = h.request(&ask)?;
    Ok((got.status, got.body.as_option().map(convert::from_proto).unwrap_or(Value::Null)))
}

/// Pointman App, if the board has it installed somewhere; an error when the board couldn't say (the last answer
/// stands). A board from before MB-707 answers 404: no App.
fn ask_app(h: &Handle) -> anyhow::Result<Option<App>> {
    let (status, v) = call(h, "GET", "/v1/node/github/app", None)?;
    match status {
        200 => Ok(App::from_answer(&v)),
        404 | 503 => Ok(None),
        _ => bail!("the board answered {status} ({})", v["error"]["code"].as_str().unwrap_or("?")),
    }
}

/// Asks again when it's time, or `now`. The App's tokens go when the board no longer has it.
fn check_app(h: &Handle, board: &Mutex<Board>, now: bool) -> Option<App> {
    let due = now || board.lock().unwrap_or_else(|e| e.into_inner()).asked.is_none_or(|t| t.elapsed() >= APP_EVERY);
    if due {
        let got = ask_app(h);
        let mut b = board.lock().unwrap_or_else(|e| e.into_inner());
        b.asked = Some(Instant::now());
        match got {
            Ok(app) => {
                if app.is_none() && b.app.is_some() {
                    let _ = std::fs::remove_file(app::tokens_file(&state(h)));
                    h.log("the board has no Pointman App now: runs get the vault's account again");
                }
                if app.is_some() && b.app.is_none() {
                    h.log("the board has Pointman App: runs act on GitHub as it");
                }
                b.app = app;
            }
            Err(e) => h.log(format!("couldn't ask the board for Pointman App: {e:#}")),
        }
    }
    board.lock().unwrap_or_else(|e| e.into_inner()).app.clone()
}

/// The `gh` runs find first, and a fresh token for every account the App is installed on.
fn keep_tokens(h: &Handle, app: &App) {
    let state = state(h);
    if let (Some(gh), Ok(exe)) = (cli(h), std::env::current_exe()) {
        if let Err(e) = app::write_shim(&exe, &state, &gh) {
            h.log(format!("couldn't write the runs' gh: {e:#}"));
        }
    }
    let mut kept = Kept::read(&state);
    let before = kept.clone();
    kept.default = app.owners.first().cloned();
    kept.tokens.retain(|k, _| app.owners.iter().any(|o| o.to_lowercase() == *k));
    for owner in kept.due(&app.owners, app::now()).into_iter().cloned().collect::<Vec<_>>() {
        match call(h, "POST", "/v1/node/github/token", Some(json!({"owner": owner}))) {
            Ok((200, v)) => match (v["token"].as_str(), v["expires_at"].as_str().and_then(app::parse_time)) {
                (Some(token), Some(expires)) => {
                    let owner = v["owner"].as_str().unwrap_or(&owner).to_string();
                    kept.tokens.insert(owner.to_lowercase(), Token { owner, token: token.into(), expires });
                }
                _ => h.log(format!("the board's token for {owner} came without a token or its end")),
            },
            Ok((status, v)) => h.log(format!(
                "no Pointman App token for {owner}: {status} {}",
                v["error"]["code"].as_str().unwrap_or("")
            )),
            Err(e) => h.log(format!("couldn't ask the board for {owner}'s token: {e:#}")),
        }
    }
    if kept != before {
        if let Err(e) = kept.write(&state) {
            h.log(format!("couldn't keep the App's tokens: {e:#}"));
        }
    }
}

/// What a run's environment gets while the board has the App: see app::run_env.
fn app_env(h: &Handle, app: &App) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let state = state(h);
    let exe = std::env::current_exe().context("where this extension's binary is")?;
    let home = PathBuf::from(std::env::var_os("HOME").context("HOME")?);
    let user = std::env::var("USER").ok();
    let path = app::run_path(&app::shim_dir(&state), &home, user.as_deref());
    Ok(app::run_env(app, &exe, &state, path))
}

/// `bin/github token <state> -- <gh's arguments>` and `bin/github git-credential <state> <op>`, from a run's `gh`
/// and git: they answer from the state folder, and the process ends.
fn helper(argv: &[String]) -> Option<i32> {
    let (word, state) = (argv.get(1)?.as_str(), argv.get(2).map(Path::new));
    let answer = match (word, state) {
        ("token", Some(state)) => {
            let rest = &argv[3..];
            let args = rest.strip_prefix(&["--".to_string()][..]).unwrap_or(rest);
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            app::token_command(state, args, &cwd)
        }
        ("git-credential", Some(state)) => {
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            app::git_credential(state, argv.get(3).map(String::as_str).unwrap_or("get"), &input)
        }
        ("token" | "git-credential", None) => Err(anyhow::anyhow!("usage: github {word} <state folder> …")),
        _ => return None,
    };
    Some(match answer {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(e) => {
            eprintln!("github: {e:#}");
            1
        }
    })
}

fn main() -> anyhow::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    if let Some(code) = helper(&argv) {
        std::process::exit(code);
    }
    let mut ext = Extension::new()?;
    let h = ext.handle();
    let vault = Arc::new(Mutex::new(Vault::default()));
    let signins = Arc::new(SignIns::default());
    let board = Arc::new(Mutex::new(Board::default()));

    // a machine without gh gets its own, in the state folder (James, Pointman thread 649 [4500])
    let b = board.clone();
    ext.on_start(move |h: &Handle| {
        let to = state(h);
        if cli(h).is_none() && to.is_absolute() {
            let h = h.clone();
            std::thread::spawn(move || match install::install(&to) {
                Ok((gh, version)) => h.log(format!("installed gh {version} at {}", gh.display())),
                Err(e) => h.log(format!("couldn't install gh: {e:#}")),
            });
        }
        // Pointman App's tokens, kept fresh for runs to read (MB-707)
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || loop {
            if let Some(app) = check_app(&h, &b, false) {
                keep_tokens(&h, &app);
            }
            std::thread::sleep(TOKENS_EVERY);
        });
    });

    // agents' runs act on GitHub as Pointman App once the board has it; before, they get the vault's token
    // ([sessions] in extension.toml)
    let (hh, v, b) = (h.clone(), vault.clone(), board.clone());
    ext.on(move |_: SessionPrepareRequest| {
        let mut answer = SessionPrepareResponse::default();
        // asked here only before the first time behind: a run's start never waits on it otherwise
        let known = {
            let board = b.lock().unwrap_or_else(|e| e.into_inner());
            board.asked.map(|_| board.app.clone())
        };
        if let Some(app) = known.unwrap_or_else(|| check_app(&hh, &b, true)) {
            answer.env.extend(app_env(&hh, &app)?);
        } else if let Some(token) = v.lock().unwrap_or_else(|e| e.into_inner()).token(&hh) {
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
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        s.start(&gh, session, Duration::from_secs(30))
    });
    let (hh, s) = (h.clone(), signins.clone());
    ext.on(move |p: CredentialsWaitRequest| {
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine")?;
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
        let value = s.wait(&gh, session, github::signin::WAIT)?;
        Ok(CredentialsWaitResponse { value, ..Default::default() })
    });
    let (hh, s) = (h.clone(), signins);
    ext.on(move |p: CredentialsCodeRequest| {
        let gh = cli(&hh).context("GitHub's CLI isn't on this machine")?;
        let session = p.session.as_option().map(|s| s.id.as_str()).unwrap_or_default();
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
