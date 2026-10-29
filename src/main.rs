//! The extension's process: the node starts it (`bin/github`, from the extension's folder) and talks to it
//! over stdin and stdout. It installs GitHub's CLI where there's none, and has agents' runs act on GitHub as
//! Pointman App, with tokens from the board kept fresh (`app`). There's no other account: until the board has the
//! App, runs' `gh` and git pushes say so (MB-707). GitHub's pages open in the apps' web panel (James, Pointman
//! thread 8446).
//!
//! A run's `gh` and git call it back as `bin/github token <state> -- <gh's arguments>`, `bin/github why <state> --
//! <gh's arguments>` and `bin/github git-credential <state> <get|store|erase>`: those answer from the state folder
//! and exit.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use github::app::{self, App, Kept, Token};
use github::{find_gh, install};
use pointman_extension::convert;
use pointman_extension::protocol::{
    CoreCallRequest, SessionEndedRequest, SessionEndedResponse, SessionPrepareRequest, SessionPrepareResponse,
};
use pointman_extension::{Extension, Handle};
use serde_json::{json, Value};

fn state(h: &Handle) -> PathBuf {
    h.state_dir()
}

fn cli(h: &Handle) -> Option<PathBuf> {
    let state = state(h);
    find_gh(h.settings()["cli"].as_str().unwrap_or_default(), state.is_absolute().then_some(state.as_path()))
}

// ------------------------------------------------------------------------------------------------ Pointman App

/// How often the board is asked whether it has Pointman App and where it's installed, and how often the tokens are
/// looked at (each is fetched again when it has less than app::RENEW_SECS left).
const APP_EVERY: Duration = Duration::from_secs(300);
const TOKENS_EVERY: Duration = Duration::from_secs(60);

/// When the board was last asked, and what it last said: None until it has answered, then its App, if it has one.
#[derive(Default)]
struct Board {
    asked: Option<Instant>,
    said: Option<Option<App>>,
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

/// What the board last said (see Board), asking again when it's time, or `now`.
fn check_app(h: &Handle, board: &Mutex<Board>, now: bool) -> Option<Option<App>> {
    let due = now || board.lock().unwrap_or_else(|e| e.into_inner()).asked.is_none_or(|t| t.elapsed() >= APP_EVERY);
    if due {
        let got = ask_app(h);
        let mut b = board.lock().unwrap_or_else(|e| e.into_inner());
        b.asked = Some(Instant::now());
        match got {
            Ok(app) => {
                let had = b.said.as_ref().map(Option::is_some);
                match (had, app.is_some()) {
                    (Some(true), false) => h.log("the board has no Pointman App now: runs' gh and git pushes stop"),
                    (Some(false) | None, true) => h.log("the board has Pointman App: runs act on GitHub as it"),
                    _ => {}
                }
                b.said = Some(app);
            }
            Err(e) => h.log(format!("couldn't ask the board for Pointman App: {e:#}")),
        }
    }
    board.lock().unwrap_or_else(|e| e.into_inner()).said.clone()
}

/// The `gh` runs find first: GitHub's CLI with the App's token, or, while there's no CLI here, one that says so.
fn write_shim(h: &Handle) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("where this extension's binary is")?;
    app::write_shim(&exe, &state(h), cli(h).as_deref()).context("couldn't write the runs' gh")?;
    Ok(())
}

/// What the board said, for runs to read: its App and where it's installed, and a fresh token for each of those.
fn keep_tokens(h: &Handle, app: Option<&App>) {
    let state = state(h);
    let before = Kept::read(&state);
    let mut kept = Kept {
        app: app.map(|a| a.slug.clone()),
        owners: app.map(|a| a.owners.clone()).unwrap_or_default(),
        tokens: before.clone().map(|k| k.tokens).unwrap_or_default(),
    };
    kept.tokens.retain(|k, _| kept.owners.iter().any(|o| o.to_lowercase() == *k));
    for owner in kept.due(app::now()) {
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
    if before.as_ref() != Some(&kept) {
        if let Err(e) = kept.write(&state) {
            h.log(format!("couldn't keep the App's tokens: {e:#}"));
        }
    }
}

/// What a run's environment gets: see app::run_env. The runs' `gh` is written first; the node puts its folder (`[sessions] path`) first on PATH.
fn app_env(h: &Handle, app: Option<&App>) -> anyhow::Result<BTreeMap<String, String>> {
    write_shim(h)?;
    let state = state(h);
    let exe = std::env::current_exe().context("where this extension's binary is")?;
    Ok(app::run_env(app, &exe, &state))
}

/// `bin/github token|why <state> -- <gh's arguments>` and `bin/github git-credential <state> <op>`, from a run's
/// `gh` and git: they answer from the state folder, and the process ends.
fn helper(argv: &[String]) -> Option<i32> {
    let (word, state) = (argv.get(1)?.as_str(), argv.get(2).map(Path::new));
    let answer = match (word, state) {
        (word @ ("token" | "why"), Some(state)) => {
            let rest = &argv[3..];
            let args = rest.strip_prefix(&["--".to_string()][..]).unwrap_or(rest);
            let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            match word {
                "token" => app::token_command(state, args, &cwd),
                _ => Ok(app::why_command(state, args, &cwd)),
            }
        }
        ("git-credential", Some(state)) => {
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            app::git_credential(state, argv.get(3).map(String::as_str).unwrap_or("get"), &input)
        }
        ("token" | "why" | "git-credential", None) => Err(anyhow::anyhow!("usage: github {word} <state folder> …")),
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
        // the runs' gh, and what the board says about Pointman App with its tokens, kept fresh for runs (MB-707)
        let (h, b) = (h.clone(), b.clone());
        std::thread::spawn(move || loop {
            if let Err(e) = write_shim(&h) {
                h.log(format!("{e:#}"));
            }
            if let Some(said) = check_app(&h, &b, false) {
                keep_tokens(&h, said.as_ref());
            }
            std::thread::sleep(TOKENS_EVERY);
        });
    });

    // agents' runs act on GitHub as Pointman App, and only as it ([sessions] in extension.toml)
    let (hh, b) = (h.clone(), board.clone());
    ext.on(move |_: SessionPrepareRequest| {
        // asked here only before the first time behind: a run's start never waits on it otherwise
        let known = {
            let board = b.lock().unwrap_or_else(|e| e.into_inner());
            board.asked.map(|_| board.said.clone())
        };
        let said = known.unwrap_or_else(|| check_app(&hh, &b, true)).flatten();
        let mut answer = SessionPrepareResponse::default();
        answer.env.extend(app_env(&hh, said.as_ref())?);
        Ok(answer)
    });
    ext.on(|_: SessionEndedRequest| Ok(SessionEndedResponse::default()));

    ext.run()
}
