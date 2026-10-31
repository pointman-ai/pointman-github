//! Pointman App for agents' runs (MB-707; James, Pointman thread 8446 [26244], [26267]): what agents do on GitHub
//! goes out as the App's bot, not as a person's account. The board gives this machine hour-long installation tokens
//! (`GET /v1/node/github/app`, `POST /v1/node/github/token`), one for each account the App is installed on; the
//! App's key never leaves api.pointman.ai. The extension keeps them fresh in its state folder, and a run's `gh` and
//! git pick the one for the repo's owner:
//!
//! - `gh` is a small script first on the run's PATH (`app/bin/gh`), which hands the command to `bin/github gh`: it
//!   runs GitHub's CLI with the token as GH_TOKEN.
//! - git asks `bin/github git-credential` for https://github.com (GIT_CONFIG_*), pushes to ssh remotes go over https
//!   (`pushInsteadOf`), and commits are the bot's (GIT_AUTHOR_*, GIT_COMMITTER_*).
//!
//! Where the App can't act (no App on the board, an account it isn't installed on, or GitHub refusing its token),
//! both use this machine's own `gh` login instead, and say so (James, Pointman thread 25602 [29623]: "allow agents
//! to use gh as a fallback if app permissions aren't working for them").

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde_json::{json, Value};

/// A token is fetched again once it has less than this left, and never handed out with less than USABLE.
pub const RENEW_SECS: i64 = 10 * 60;
pub const USABLE_SECS: i64 = 60;

/// The App as the board has it: its bot, and the accounts it's installed on (their logins as GitHub has them).
#[derive(Clone, Debug, PartialEq)]
pub struct App {
    pub slug: String,
    pub bot_name: String,
    pub bot_email: String,
    pub owners: Vec<String>,
}

impl App {
    /// `GET /v1/node/github/app`'s answer. None without a bot: no App.
    pub fn from_answer(v: &Value) -> Option<App> {
        let s = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let owners: Vec<String> = v["installations"]
            .as_array()
            .map(|a| a.iter().filter_map(|i| s(&i["owner"])).collect())
            .unwrap_or_default();
        Some(App { slug: s(&v["slug"])?, bot_name: s(&v["bot"]["name"])?, bot_email: s(&v["bot"]["email"])?, owners })
    }
}

/// One installation's token, as `POST /v1/node/github/token` gave it.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub owner: String,
    pub token: String,
    /// Seconds since the epoch.
    pub expires: i64,
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or_default()
}

/// RFC 3339 in UTC, as GitHub and the board write it (`2026-10-29T01:02:03Z`, fractions and offsets allowed), as
/// seconds since the epoch.
pub fn parse_time(s: &str) -> Option<i64> {
    let (date, rest) = s.trim().split_once(['T', 't', ' '])?;
    let d: Vec<i64> = date.split('-').map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let (&[y, mo, da], clock) = (d.as_slice(), rest) else { return None };
    let (clock, offset) = match clock.find(['Z', 'z', '+', '-']) {
        Some(i) => (&clock[..i], &clock[i..]),
        None => (clock, "Z"),
    };
    let hms: Vec<&str> = clock.split(':').collect();
    let [h, mi, se] = hms.as_slice() else { return None };
    let (h, mi): (i64, i64) = (h.parse().ok()?, mi.parse().ok()?);
    let se: i64 = se.split('.').next()?.parse().ok()?;
    let off = match offset {
        "Z" | "z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let (oh, om) = o[1..].split_once(':').unwrap_or((&o[1..3.min(o.len())], "0"));
            sign * (oh.parse::<i64>().ok()? * 3600 + om.parse::<i64>().ok()? * 60)
        }
    };
    // days from the civil date (Howard Hinnant's days_from_civil)
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (mo + if mo > 2 { -3 } else { 9 }) + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + h * 3600 + mi * 60 + se - off)
}

// ------------------------------------------------------------------------------------------- the tokens' file

/// The tokens' file in the state folder: only this machine's user may read it, and each token lasts an hour at most.
pub fn tokens_file(state: &Path) -> PathBuf {
    state.join("app").join("tokens.json")
}

/// What the board last said, as runs read it: its App, if it has one, the accounts it's installed on (the first is
/// used when a command names no repo), and their tokens.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    pub app: Option<String>,
    pub owners: Vec<String>,
    pub tokens: BTreeMap<String, Token>,
}

/// What a run's `gh` and git say before the extension has asked the board.
const NOT_YET: &str = "Pointman App's tokens aren't on this machine yet: is the github extension running?";

impl Kept {
    /// None before the extension has written it.
    pub fn read(state: &Path) -> Option<Kept> {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(tokens_file(state)).ok()?).ok()?;
        let tokens = v["tokens"]
            .as_object()?
            .values()
            .filter_map(|t| {
                Some(Token {
                    owner: t["owner"].as_str()?.to_string(),
                    token: t["token"].as_str()?.to_string(),
                    expires: t["expires"].as_i64()?,
                })
            })
            .map(|t| (t.owner.to_lowercase(), t))
            .collect();
        let owners = v["owners"].as_array()?.iter().filter_map(|o| o.as_str().map(str::to_string)).collect();
        Some(Kept { app: v["app"].as_str().map(str::to_string), owners, tokens })
    }

    /// Written whole to a new file, then moved over the old: a reader never sees half of it.
    pub fn write(&self, state: &Path) -> anyhow::Result<()> {
        let path = tokens_file(state);
        let dir = path.parent().expect("in app/");
        std::fs::create_dir_all(dir)?;
        let tokens: serde_json::Map<String, Value> = self
            .tokens
            .iter()
            .map(|(k, t)| (k.clone(), json!({"owner": t.owner, "token": t.token, "expires": t.expires})))
            .collect();
        let body = json!({"app": self.app, "owners": self.owners, "tokens": tokens}).to_string();
        let mut f = tempfile::NamedTempFile::new_in(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.as_file().set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(body.as_bytes())?;
        f.persist(&path)?;
        Ok(())
    }

    /// Whether the App is installed on `owner` (any case).
    pub fn installed_on(&self, owner: &str) -> bool {
        self.owners.iter().any(|o| o.eq_ignore_ascii_case(owner))
    }

    /// The token for `owner` (any case), or with no owner the first installation's that has one with a minute left
    /// (an installation the board lists but has no token for, such as an old row, is passed over).
    pub fn token_for(&self, owner: Option<&str>, at: i64) -> Result<&Token, String> {
        if self.app.is_none() {
            return Err("Pointman App isn't set up on this board yet".into());
        }
        let key = match owner {
            Some(o) if !self.installed_on(o) => return Err(format!("Pointman App isn't installed on {o}")),
            Some(o) => o.to_lowercase(),
            None => match self.owners.first() {
                Some(first) => self
                    .owners
                    .iter()
                    .map(|o| o.to_lowercase())
                    .find(|k| self.tokens.get(k).is_some_and(|t| t.expires - at >= USABLE_SECS))
                    .unwrap_or_else(|| first.to_lowercase()),
                None => return Err("Pointman App isn't installed on any GitHub account yet".into()),
            },
        };
        match self.tokens.get(&key) {
            Some(t) if t.expires - at >= USABLE_SECS => Ok(t),
            Some(t) => Err(format!(
                "Pointman App's token for {} has run out: is the github extension running on this machine?",
                t.owner
            )),
            None => Err(format!("there's no Pointman App token for {key} yet: try again in a minute")),
        }
    }

    /// The owners whose token is missing or runs out within RENEW_SECS.
    pub fn due(&self, at: i64) -> Vec<String> {
        self.owners
            .iter()
            .filter(|o| self.tokens.get(&o.to_lowercase()).is_none_or(|t| t.expires - at < RENEW_SECS))
            .cloned()
            .collect()
    }
}

// ----------------------------------------------------------------------------------- which account a command is for

/// The owner in a GitHub repo's address: `git@github.com:o/r.git`, `ssh://git@github.com/o/r`,
/// `https://[user@]github.com/o/r[.git]`, or the short `o/r` and `github.com/o/r`. The whole text is the address:
/// a link inside other words isn't.
pub fn owner_of(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = if let Some(r) = s.strip_prefix("git@github.com:") {
        r
    } else if let Some(i) = s.find("github.com") {
        // the host itself, not a repo of that name, a path that only mentions it, or another host
        if !address_start(&s[..i]) {
            return None;
        }
        s[i + "github.com".len()..].strip_prefix(['/', ':'])?
    } else if s.contains("://") || s.starts_with('-') {
        return None;
    } else {
        s
    };
    let mut parts = rest.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    let ok = |p: &str| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    (ok(owner) && ok(repo.trim_end_matches(".git")) && !owner.starts_with('.')).then(|| owner.to_string())
}

/// What may come before the host in an address: nothing, a scheme (`https://`), a user (`git@`,
/// `x-access-token:…@`), or both. Never words, a path or another host.
fn address_start(before: &str) -> bool {
    let user = match before.split_once("://") {
        Some((scheme, user)) => {
            let mut c = scheme.chars();
            if !(c.next().is_some_and(|c| c.is_ascii_alphabetic())
                && c.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)))
            {
                return false;
            }
            user
        }
        None => before,
    };
    user.is_empty()
        || user
            .strip_suffix('@')
            .is_some_and(|u| !u.is_empty() && !u.contains(|c: char| c.is_whitespace() || "/@".contains(c)))
}

/// Options whose value is free text (a title, a body, notes, a jq filter or template): a link in one isn't the repo
/// the command is about (`gh pr create --body "Fixes https://github.com/o/x/issues/3"`).
const TEXT_OPTIONS: [&str; 11] =
    ["-b", "--body", "-t", "--title", "-n", "--notes", "--subject", "--description", "-q", "--jq", "--template"];

/// A command's words without its options' free text.
fn words_of(args: &[String]) -> Vec<&str> {
    let mut words = Vec::new();
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        if TEXT_OPTIONS.contains(&a) {
            it.next();
        } else if !TEXT_OPTIONS
            .iter()
            .any(|o| o.starts_with("--") && a.strip_prefix(o).is_some_and(|v| v.starts_with('=')))
        {
            words.push(a);
        }
    }
    words
}

/// The account a `gh` command is about, from its words: `-R`/`--repo`, a github.com link, an API path
/// (`repos/o/…`, `orgs/o`, `users/o`), or the `o/r` a `gh repo …` names. None when they don't say.
pub fn owner_in_args(args: &[String]) -> Option<String> {
    let words = words_of(args);
    let mut i = 0;
    while i < words.len() {
        let a = words[i];
        if a == "-R" || a == "--repo" {
            if let Some(o) = words.get(i + 1).and_then(|v| owner_of(v)) {
                return Some(o);
            }
            i += 2;
            continue;
        }
        if let Some(v) = a.strip_prefix("--repo=").or_else(|| a.strip_prefix("-R").filter(|v| !v.is_empty())) {
            if let Some(o) = owner_of(v) {
                return Some(o);
            }
        }
        i += 1;
    }
    if let Some(o) = words.iter().filter(|a| a.contains("github.com/")).find_map(|a| owner_of(a)) {
        return Some(o);
    }
    if words.first() == Some(&"api") {
        for a in &words[1..] {
            let p = a.trim_start_matches('/');
            for prefix in ["repos/", "orgs/", "users/"] {
                if let Some(rest) = p.strip_prefix(prefix) {
                    let owner = rest.split(['/', '?']).next().unwrap_or_default();
                    if !owner.is_empty() && !owner.starts_with('{') {
                        return Some(owner.to_string());
                    }
                }
            }
        }
    }
    if words.first() == Some(&"repo") {
        if let Some(o) = words[1..].iter().filter(|a| !a.starts_with('-')).skip(1).find_map(|a| owner_of(a)) {
            return Some(o);
        }
    }
    None
}

/// The account a `gh` command run in `cwd` is about: its words, else GH_REPO, else the folder's repo.
pub fn command_owner(args: &[String], cwd: &Path) -> Option<String> {
    owner_in_args(args)
        .or_else(|| std::env::var("GH_REPO").ok().and_then(|r| owner_of(&r)))
        .or_else(|| owner_of_folder(cwd))
}

/// The owner of the repo a folder is in, as `gh` finds it: its `upstream`, `github` or `origin` remote, in that order.
pub fn owner_of_folder(dir: &Path) -> Option<String> {
    ["upstream", "github", "origin"].iter().find_map(|remote| {
        let out = std::process::Command::new("git")
            .args(["config", "--get", &format!("remote.{remote}.url")])
            .current_dir(dir)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        out.status.success().then(|| owner_of(&String::from_utf8_lossy(&out.stdout))).flatten()
    })
}

// ------------------------------------------------------------------------------------------- what a run is given

/// `s` quoted for sh, whatever it holds.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The folder whose `gh` a run finds first.
pub fn shim_dir(state: &Path) -> PathBuf {
    state.join("app").join("bin")
}

/// The `gh` a run finds first: `bin/github gh` (see `gh_command`). With no CLI on the machine yet, it says so, and
/// never lets a run find another `gh`.
pub fn shim(exe: &Path, state: &Path, gh: Option<&Path>) -> String {
    let head = "#!/bin/sh\n\
                # GitHub's CLI for agents' runs: as Pointman App, else this machine's own login (MB-707, EXT-116)\n";
    if gh.is_none() {
        return format!(
            "{head}echo \"gh: GitHub's CLI isn't on this machine yet: the github extension is installing it\" >&2\n\
             exit 1\n"
        );
    }
    format!(
        "{head}exec {} gh {} -- \"$@\"\n",
        sh_quote(&exe.display().to_string()),
        sh_quote(&state.display().to_string())
    )
}

/// The file that names GitHub's CLI on this machine, for `bin/github gh` and git's helper.
fn cli_file(state: &Path) -> PathBuf {
    state.join("app").join("cli")
}

/// GitHub's CLI on this machine, as the extension last found it.
pub fn machine_cli(state: &Path) -> Option<PathBuf> {
    let path = PathBuf::from(std::fs::read_to_string(cli_file(state)).ok()?.trim());
    path.is_file().then_some(path)
}

fn write_script(dir: &Path, name: &str, text: &str) -> anyhow::Result<PathBuf> {
    let path = dir.join(name);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
        let mut f = tempfile::NamedTempFile::new_in(dir)?;
        f.write_all(text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.as_file().set_permissions(std::fs::Permissions::from_mode(0o755))?;
        }
        f.persist(&path)?;
    }
    Ok(path)
}

/// Writes the runs' `gh` and `git` (see `shim` and `git_shim`), the people who are credited on their commits, and
/// where GitHub's CLI is.
pub fn write_shim(
    exe: &Path,
    state: &Path,
    gh: Option<&Path>,
    coauthors: &serde_json::Value,
) -> anyhow::Result<PathBuf> {
    let dir = shim_dir(state);
    std::fs::create_dir_all(&dir)?;
    write_script(&dir, "git", &git_shim(exe, state))?;
    write_script(&dir, "coauthors.json", &coauthors.to_string())?;
    match gh {
        Some(gh) => {
            write_script(&state.join("app"), "cli", &gh.display().to_string())?;
        }
        None => {
            let _ = std::fs::remove_file(cli_file(state));
        }
    }
    write_script(&dir, "gh", &shim(exe, state, gh))
}

/// The `git` a run finds first: it is git, but a commit in a run a person's post started is theirs, with Pointman App as co-author (MB-726).
pub fn git_shim(exe: &Path, state: &Path) -> String {
    format!(
        "#!/bin/sh\n# git for agents' runs: a commit in a run a person started is theirs, with Pointman App as co-author (MB-726)\nexec {} git {} -- \"$@\"\n",
        sh_quote(&exe.display().to_string()),
        sh_quote(&state.display().to_string())
    )
}

/// `Name <email>` for a person's handle, as the extension's `coauthors` setting names them.
fn coauthor(state: &Path, person: &str) -> Option<String> {
    let text = std::fs::read_to_string(shim_dir(state).join("coauthors.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let who = v.get(person)?.as_str()?.trim();
    (!person.is_empty() && !who.is_empty() && !who.contains('\n')).then(|| who.to_string())
}

/// Where git's subcommand is in its arguments, past the options that come before it.
fn subcommand_at(args: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--exec-path" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => return Some(i),
        }
    }
    None
}

/// Where `commit` is in git's arguments, when it's the subcommand.
fn commit_at(args: &[String]) -> Option<usize> {
    subcommand_at(args).filter(|&i| args[i] == "commit")
}

/// git's subcommands that talk to GitHub: one GitHub refuses the App's token for runs again with the machine's login.
const NETWORK: [&str; 5] = ["push", "fetch", "pull", "clone", "ls-remote"];

/// Whether git's arguments are one of NETWORK.
pub fn talks_to_github(args: &[String]) -> bool {
    subcommand_at(args).is_some_and(|i| NETWORK.contains(&args[i].as_str()))
}

/// git's arguments with Pointman App credited as co-author, when this is a `commit` in a run a person's post
/// started and they have an entry (they become the author, see `git_command`); else the arguments as they are.
pub fn git_args(state: &Path, person: &str, bot: &str, args: &[String]) -> Vec<String> {
    let mut out = args.to_vec();
    if let (Some(at), Some(_), false) = (commit_at(args), coauthor(state, person), bot.is_empty()) {
        let trailer = format!("Co-authored-by: {bot}");
        if !args[at..].iter().any(|a| a == &format!("--trailer={trailer}")) {
            out.splice(at + 1..at + 1, ["--trailer".to_string(), trailer]);
        }
    }
    out
}

/// `Name <email>` split into its two parts.
fn split_identity(who: &str) -> Option<(&str, &str)> {
    let (name, rest) = who.split_once('<')?;
    let email = rest.strip_suffix('>')?.trim();
    (!name.trim().is_empty() && !email.is_empty()).then(|| (name.trim(), email))
}

/// The person whose post started this run: the handle in the file `MB_RUN_PERSON_FILE` names (a resident session's
/// person changes from one post to the next, and the node rewrites the file before each), else `MB_RUN_PERSON`. A file
/// that can't be read means no person, so the commit stays the App's.
fn run_person() -> String {
    match std::env::var_os("MB_RUN_PERSON_FILE").filter(|f| !f.is_empty()) {
        Some(f) => std::fs::read_to_string(f).map(|t| t.trim().to_string()).unwrap_or_default(),
        None => std::env::var("MB_RUN_PERSON").unwrap_or_default(),
    }
}

/// The first `git` on PATH that isn't the runs' own.
fn real_git(state: &Path) -> Option<PathBuf> {
    let mine = shim_dir(state);
    let mine = mine.canonicalize().unwrap_or(mine);
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|d| d.canonicalize().unwrap_or_else(|_| d.clone()) != mine)
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
}

/// `bin/github git <state> -- <git's arguments>`: git signed in as the App, with the person who started this run
/// credited on a commit. Unless `login` is App-only, a push, fetch or clone GitHub refuses the App's token for runs
/// once more, with git's helper answering with this machine's own `gh` login (`MACHINE_LOGIN`).
pub fn git_command(state: &Path, args: &[String], login: Login) -> anyhow::Result<i32> {
    let git = real_git(state).context("git isn't on this machine's PATH")?;
    let person = run_person();
    let bot = match (std::env::var("GIT_COMMITTER_NAME"), std::env::var("GIT_COMMITTER_EMAIL")) {
        (Ok(n), Ok(e)) if !n.is_empty() && !e.is_empty() => format!("{n} <{e}>"),
        _ => String::new(),
    };
    let args = git_args(state, &person, &bot, args);
    let exe = std::env::current_exe().context("where this extension's binary is")?;
    let mut cmd = Command::new(git);
    if let (Some(at), Some(who)) = (commit_at(&args), coauthor(state, &person)) {
        // the person is the author; Pointman App stays the committer and is the co-author
        if let (true, Some((name, email))) = (at < args.len() && !bot.is_empty(), split_identity(&who)) {
            cmd.env("GIT_AUTHOR_NAME", name).env("GIT_AUTHOR_EMAIL", email);
        }
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").args(signed_in(&exe, state, &args));
    if login != Login::AppThenMachine || !talks_to_github(&args) {
        return Ok(cmd.status()?.code().unwrap_or(1));
    }
    let (code, err) = keeping_stderr(&mut cmd)?;
    let Some(line) = refusal(code, &err) else { return Ok(code) };
    eprintln!(
        "github: GitHub refused Pointman App (\"{line}\"), so git runs it again with this machine's own gh login"
    );
    Ok(cmd.stderr(Stdio::inherit()).env(MACHINE_LOGIN, "1").status()?.code().unwrap_or(1))
}

/// git's arguments after `git_config` as `-c`s: signed in as the App by itself, not only in a run whose environment
/// says so (a workflow's step runs this git too, with none of it).
pub fn signed_in(exe: &Path, state: &Path, args: &[String]) -> Vec<String> {
    let mut out: Vec<String> =
        git_config(exe, state).into_iter().flat_map(|(k, v)| ["-c".to_string(), format!("{k}={v}")]).collect();
    out.extend_from_slice(args);
    out
}

/// git asking this extension for GitHub's password (never a person's keychain or gh), and pushes over https.
pub fn git_config(exe: &Path, state: &Path) -> [(&'static str, String); 5] {
    let helper =
        format!("!{} git-credential {}", sh_quote(&exe.display().to_string()), sh_quote(&state.display().to_string()));
    [
        // an empty helper first, so a person's own (the keychain's) isn't asked before this one
        ("credential.https://github.com.helper", String::new()),
        ("credential.https://github.com.helper", helper),
        ("credential.https://github.com.useHttpPath", "true".into()),
        ("url.https://github.com/.pushInsteadOf", "git@github.com:".into()),
        ("url.https://github.com/.pushInsteadOf", "ssh://git@github.com/".into()),
    ]
}

/// What a run's environment gets: `git_config` (and never a prompt), and, once the board has the App, commits as
/// its bot.
pub fn run_env(app: Option<&App>, exe: &Path, state: &Path) -> BTreeMap<String, String> {
    let config = git_config(exe, state);
    let mut env = BTreeMap::from([
        ("GIT_TERMINAL_PROMPT".to_string(), "0".into()),
        ("GIT_CONFIG_COUNT".into(), config.len().to_string()),
    ]);
    for (i, (k, v)) in config.into_iter().enumerate() {
        env.insert(format!("GIT_CONFIG_KEY_{i}"), k.to_string());
        env.insert(format!("GIT_CONFIG_VALUE_{i}"), v);
    }
    if let Some(app) = app {
        env.insert("GIT_AUTHOR_NAME".into(), app.bot_name.clone());
        env.insert("GIT_AUTHOR_EMAIL".into(), app.bot_email.clone());
        env.insert("GIT_COMMITTER_NAME".into(), app.bot_name.clone());
        env.insert("GIT_COMMITTER_EMAIL".into(), app.bot_email.clone());
    }
    env
}

// ------------------------------------------------------------------------------------- the helpers runs call

/// What GitHub's CLI would use instead of the machine's own login.
const TOKEN_VARS: [&str; 4] = ["GH_TOKEN", "GITHUB_TOKEN", "GH_ENTERPRISE_TOKEN", "GITHUB_ENTERPRISE_TOKEN"];

/// Set, a run's `gh` and git act as this machine's own login straight away: git_command sets it for git's helper
/// once GitHub has refused the App, and gh says to put it in front of a write it didn't run again.
pub const MACHINE_LOGIN: &str = "POINTMAN_GITHUB_MACHINE_LOGIN";

/// Set, `gh` and git act only as the App and never fall back: for workflow steps, so a permission the App lacks
/// shows up rather than going out as a person.
pub const APP_ONLY: &str = "POINTMAN_GITHUB_APP_ONLY";

/// Who a run's `gh` and git act as.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Login {
    /// The App, else this machine's own `gh` login (James, Pointman thread 25602 [29623]).
    AppThenMachine,
    /// Only the App (APP_ONLY).
    AppOnly,
    /// This machine's own login straight away (MACHINE_LOGIN).
    Machine,
}

impl Login {
    /// From the environment: APP_ONLY wins over MACHINE_LOGIN; empty or `0` is unset.
    pub fn from_env() -> Login {
        let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty() && v != "0");
        match (set(APP_ONLY), set(MACHINE_LOGIN)) {
            (true, _) => Login::AppOnly,
            (false, true) => Login::Machine,
            _ => Login::AppThenMachine,
        }
    }
}

/// When gh or git failed because GitHub refused the App's token, the first line that says so: a permission the App
/// wasn't given ("Resource not accessible by integration", 403), or a repo its installation can't see (404, "not
/// found").
pub fn refusal(code: i32, stderr: &str) -> Option<&str> {
    const SAYS: [&str; 8] = [
        "not accessible by integration",
        "HTTP 403",
        "HTTP 404",
        "Could not resolve to a Repository",
        "returned error: 403",
        "returned error: 404",
        "Repository not found",
        "denied to ",
    ];
    (code != 0).then(|| stderr.lines().map(str::trim).find(|l| SAYS.iter().any(|m| l.contains(m)))).flatten()
}

/// gh's verbs that only read.
const READS: [&str; 7] = ["view", "list", "status", "checks", "diff", "watch", "search"];

/// Whether a gh command only reads, so running it again can't write twice: a READS verb (`gh pr view`), `gh search`
/// and `gh status`, and `gh api` sending nothing (no method but GET, no fields without `-X GET`, no `--input`).
pub fn only_reads(args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        Some("search" | "status") => true,
        Some("api") => api_only_reads(&args[1..]),
        Some(_) => args.get(1).is_some_and(|v| READS.contains(&v.as_str())),
        None => false,
    }
}

fn api_only_reads(args: &[String]) -> bool {
    let (mut method, mut fields) = (None, false);
    let mut it = args.iter().map(String::as_str);
    while let Some(a) = it.next() {
        if a == "-X" || a == "--method" {
            method = it.next();
        } else if let Some(m) = a.strip_prefix("--method=").or_else(|| a.strip_prefix("-X")) {
            method = Some(m);
        } else if a == "--input" || a.starts_with("--input=") {
            return false;
        } else if ["-f", "-F", "--field", "--raw-field"].iter().any(|f| a.starts_with(f)) {
            fields = true;
        }
    }
    method.map_or(!fields, |m| m.eq_ignore_ascii_case("GET"))
}

/// Whether a gh command reads its input from stdin (`--body-file -`, `--input=-`, `-F body=@-`): it can't run twice.
pub fn reads_stdin(args: &[String]) -> bool {
    args.iter().any(|a| a == "-" || a.ends_with("=-") || a.ends_with("@-"))
}

/// Runs `cmd` with what it writes to stderr passed on as it comes, and kept: its exit code and that text.
fn keeping_stderr(cmd: &mut Command) -> anyhow::Result<(i32, String)> {
    let mut child = cmd.stderr(Stdio::piped()).spawn()?;
    let mut pipe = child.stderr.take().context("its stderr")?;
    let (mut kept, mut buf, mut out) = (Vec::new(), [0u8; 8192], std::io::stderr());
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let _ = out.write_all(&buf[..n]);
                kept.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let code = child.wait()?.code().unwrap_or(1);
    Ok((code, String::from_utf8_lossy(&kept).into_owned()))
}

/// GitHub's CLI as this machine's own login: none of TOKEN_VARS.
fn as_machine(gh: &Path) -> Command {
    let mut cmd = Command::new(gh);
    for v in TOKEN_VARS {
        cmd.env_remove(v);
    }
    cmd
}

/// The App's token for `owner` (None: the first installation's) and the account it's for, or why there's none.
fn app_token(state: &Path, owner: Option<&str>) -> Result<(String, String), String> {
    let kept = Kept::read(state).ok_or_else(|| NOT_YET.to_string())?;
    kept.token_for(owner, now()).map(|t| (t.owner.clone(), t.token.clone()))
}

/// `bin/github gh <state> -- <gh's arguments>`, a run's `gh`: GitHub's CLI with the App's token for the account the
/// command is about (the first installation's when it names none). Unless `login` is App-only, with no such token (no
/// App on the board, or an account it isn't installed on) it runs as this machine's own login, and a read GitHub
/// refuses the App's token for runs once more that way. A refused write doesn't run again: gh may have written part of
/// it already (a PR, then its labels). Each time, it says so.
pub fn gh_command(state: &Path, args: &[String], cwd: &Path, login: Login) -> anyhow::Result<i32> {
    let gh =
        machine_cli(state).context("GitHub's CLI isn't on this machine yet: the github extension is installing it")?;
    let token = match login {
        Login::Machine => Err(format!("{MACHINE_LOGIN} is set")),
        _ => app_token(state, command_owner(args, cwd).as_deref()),
    };
    let (account, token) = match token {
        Ok(it) => it,
        Err(why) if login == Login::AppOnly => {
            eprintln!("github: {why}");
            return Ok(1);
        }
        Err(why) => {
            eprintln!("github: {why}, so gh uses this machine's own login");
            return Ok(as_machine(&gh).args(args).status()?.code().unwrap_or(1));
        }
    };
    let (code, err) = keeping_stderr(as_machine(&gh).env("GH_TOKEN", token).args(args))?;
    let Some(line) = refusal(code, &err) else { return Ok(code) };
    let refused = format!("GitHub refused Pointman App on {account} (\"{line}\")");
    if login == Login::AppOnly {
        eprintln!("github: {refused}");
        return Ok(code);
    }
    if !only_reads(args) || reads_stdin(args) {
        eprintln!(
            "github: {refused}. It may have written part of this already, so it isn't run again: check what it did, \
             then run what's left with {MACHINE_LOGIN}=1 in front, as this machine's own login"
        );
        return Ok(code);
    }
    eprintln!("github: {refused}, so gh reads it again as this machine's own login");
    Ok(as_machine(&gh).args(args).status()?.code().unwrap_or(1))
}

/// `bin/github git-credential <state> get`: git's credential helper for https://github.com. It answers `get` with
/// the App's token for the path's owner (`useHttpPath`), else, as with `gh` and unless `login` is App-only, with this
/// machine's own `gh` login: for an account the App isn't installed on, or with `Login::Machine` (GitHub refused the
/// App's token, see `git_command`). `store` and `erase` do nothing.
pub fn git_credential(state: &Path, op: &str, input: &str, login: Login) -> anyhow::Result<String> {
    if op != "get" {
        return Ok(String::new());
    }
    let fields: BTreeMap<&str, &str> = input.lines().filter_map(|l| l.split_once('=')).collect();
    if fields.get("host").copied() != Some("github.com") {
        return Ok(String::new());
    }
    let owner = fields.get("path").and_then(|p| owner_of(p));
    let why = match login {
        Login::Machine => format!("{MACHINE_LOGIN} is set"),
        _ => match app_token(state, owner.as_deref()) {
            Ok((_, token)) => return Ok(format!("username=x-access-token\npassword={token}\n")),
            Err(why) if login == Login::AppOnly => anyhow::bail!("no GitHub password for git: {why}"),
            Err(why) => why,
        },
    };
    let answer = machine_credential(state, input)
        .with_context(|| format!("{why}, and this machine's own gh login gave none"))
        .context("no GitHub password for git")?;
    eprintln!("github: {why}, so git uses this machine's own gh login");
    Ok(answer)
}

/// GitHub's password from this machine's own `gh` login (`gh auth git-credential get`).
fn machine_credential(state: &Path, input: &str) -> anyhow::Result<String> {
    let gh = machine_cli(state).context("GitHub's CLI isn't on this machine")?;
    let mut child = as_machine(&gh)
        .args(["auth", "git-credential", "get"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    child.stdin.take().context("its stdin")?.write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    anyhow::ensure!(out.status.success() && text.contains("password="), "it isn't signed in to GitHub");
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(coauthors: serde_json::Value) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(shim_dir(dir.path())).unwrap();
        std::fs::write(shim_dir(dir.path()).join("coauthors.json"), coauthors.to_string()).unwrap();
        dir
    }

    #[test]
    fn a_commit_in_a_persons_run_credits_them_beside_the_app() {
        let who = "James <1+jamescalam@users.noreply.github.com>";
        let dir = state_with(json!({"james": who}));
        let bot = "pointman-app[bot] <1+pointman-app[bot]@users.noreply.github.com>";
        let got = git_args(dir.path(), "james", bot, &args("-C /r commit -m hi"));
        assert_eq!(got, argv(&["-C", "/r", "commit", "--trailer", &format!("Co-authored-by: {bot}"), "-m", "hi"]));
        assert_eq!(split_identity(who), Some(("James", "1+jamescalam@users.noreply.github.com")));
    }

    #[test]
    fn other_runs_and_commands_are_left_alone() {
        let dir = state_with(json!({"james": "James <1+j@users.noreply.github.com>"}));
        for (person, a) in
            [("", "commit -m x"), ("scene", "commit -m x"), ("james", "push origin main"), ("james", "log commit")]
        {
            assert_eq!(git_args(dir.path(), person, "bot <b@x>", &args(a)), args(a), "{person} {a}");
        }
        let none = tempfile::tempdir().unwrap();
        assert_eq!(git_args(none.path(), "james", "bot <b@x>", &args("commit -m x")), args("commit -m x"));
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_repos_owner_from_each_way_its_address_is_written() {
        for (url, owner) in [
            ("git@github.com:pointman-ai/pointman.git", Some("pointman-ai")),
            ("ssh://git@github.com/Alai-Studios/spark.git", Some("Alai-Studios")),
            ("https://github.com/spacesheep-dev/spacesheep", Some("spacesheep-dev")),
            ("https://x-access-token:abc@github.com/jamescalam/pointman-cve.git\n", Some("jamescalam")),
            ("github.com/a/b", Some("a")),
            ("a/b.git", Some("a")),
            ("https://github.com/a/b/pull/42", Some("a")),
            ("https://gitlab.com/a/b", None),
            ("https://notgithub.com/a/b", None),
            ("https://github.company/a/b", None),
            ("https://github.com.evil.dev/a/b", None),
            ("Fixes https://github.com/jamescalam/x/issues/3", None),
            ("body=https://github.com/a/b", None),
            ("-bhttps://github.com/a/b", None),
            ("see me@github.com/a/b", None),
            ("--json", None),
            ("main", None),
        ] {
            assert_eq!(owner_of(url).as_deref(), owner, "{url}");
        }
    }

    #[test]
    fn a_gh_commands_owner_from_its_words() {
        for (cmd, owner) in [
            ("pr view 4906 -R Alai-Studios/spark --json state", Some("Alai-Studios")),
            ("pr list --repo=spacesheep-dev/spacesheep", Some("spacesheep-dev")),
            ("pr view -Rpointman-ai/pointman 19", Some("pointman-ai")),
            ("pr view https://github.com/pointman-ai/pointman-github/pull/8", Some("pointman-ai")),
            ("api repos/jamescalam/pointman-cve/pulls", Some("jamescalam")),
            ("api /orgs/pointman-ai/repos", Some("pointman-ai")),
            ("api repos/{owner}/{repo}/pulls", None),
            ("repo clone Alai-Studios/spark", Some("Alai-Studios")),
            ("repo view", None),
            ("pr create --title x --body y", None),
        ] {
            assert_eq!(owner_in_args(&args(cmd)).as_deref(), owner, "{cmd}");
        }
    }

    #[test]
    fn a_link_in_a_title_body_or_notes_isnt_the_commands_repo() {
        for cmd in [
            argv(&["pr", "create", "--body", "Fixes https://github.com/jamescalam/x/issues/3"]),
            argv(&["pr", "create", "--body", "https://github.com/jamescalam/x/issues/3"]),
            argv(&["pr", "create", "-b", "https://github.com/jamescalam/x/issues/3", "-t", "x"]),
            argv(&["pr", "create", "--body=https://github.com/jamescalam/x/issues/3"]),
            argv(&["issue", "create", "--title", "https://github.com/jamescalam/x/issues/3"]),
            argv(&["release", "create", "v1", "--notes", "https://github.com/jamescalam/x/pull/4"]),
            argv(&["pr", "merge", "4", "--subject", "https://github.com/jamescalam/x/pull/4"]),
        ] {
            assert_eq!(owner_in_args(&cmd), None, "{cmd:?}");
        }
        let both = argv(&[
            "pr",
            "comment",
            "-R",
            "pointman-ai/pointman",
            "--body",
            "see https://github.com/jamescalam/x/pull/4",
        ]);
        assert_eq!(owner_in_args(&both).as_deref(), Some("pointman-ai"));
        let linked = argv(&[
            "pr",
            "comment",
            "https://github.com/Alai-Studios/spark/pull/9",
            "-b",
            "https://github.com/jamescalam/x",
        ]);
        assert_eq!(owner_in_args(&linked).as_deref(), Some("Alai-Studios"));
    }

    #[test]
    fn times_as_github_and_the_board_write_them() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-10-29T01:02:03Z"), Some(1_793_235_723));
        assert_eq!(parse_time("2026-10-29T01:02:03.456Z"), Some(1_793_235_723));
        assert_eq!(parse_time("2026-10-29T09:02:03+08:00"), Some(1_793_235_723));
        assert_eq!(parse_time("soon"), None);
    }

    fn kept(at: i64) -> Kept {
        let t = |o: &str, left: i64| {
            (o.to_lowercase(), Token { owner: o.into(), token: format!("ghs_{o}"), expires: at + left })
        };
        Kept {
            app: Some("pointman-app".into()),
            owners: ["pointman-ai", "Alai-Studios", "jamescalam", "spacesheep-dev"].map(String::from).into(),
            tokens: BTreeMap::from([t("pointman-ai", 3000), t("Alai-Studios", 30), t("jamescalam", 900)]),
        }
    }

    #[test]
    fn a_token_is_the_owners_else_the_first_installations_and_never_one_about_to_run_out() {
        let k = kept(1000);
        assert_eq!(k.token_for(Some("JamesCalam"), 1000).unwrap().token, "ghs_jamescalam");
        assert_eq!(k.token_for(None, 1000).unwrap().token, "ghs_pointman-ai");
        assert_eq!(k.token_for(Some("cli"), 1000).unwrap_err(), "Pointman App isn't installed on cli");
        assert!(k.token_for(Some("alai-studios"), 1000).unwrap_err().contains("run out"));
        assert!(k.token_for(Some("spacesheep-dev"), 1000).unwrap_err().contains("no Pointman App token"));
        assert_eq!(k.due(1000), vec!["Alai-Studios", "spacesheep-dev"]);
        // an installation with no token (an old row listed first) is passed over, not chosen
        let mut k2 = kept(1000);
        k2.owners = ["jamescalam-old", "pointman-ai"].map(String::from).into();
        assert_eq!(k2.token_for(None, 1000).unwrap().token, "ghs_pointman-ai");
        let none = Kept { app: Some("pointman-app".into()), ..Kept::default() };
        assert_eq!(none.token_for(None, 1000).unwrap_err(), "Pointman App isn't installed on any GitHub account yet");
    }

    #[test]
    fn before_the_board_has_the_app_there_is_no_token_at_all() {
        let no_app = Kept::default();
        assert_eq!(no_app.token_for(None, 1000).unwrap_err(), "Pointman App isn't set up on this board yet");
        assert_eq!(
            no_app.token_for(Some("pointman-ai"), 1000).unwrap_err(),
            "Pointman App isn't set up on this board yet"
        );
        let dir = tempfile::tempdir().unwrap();
        assert!(app_token(dir.path(), None).unwrap_err().contains("aren't on this machine yet"));
        no_app.write(dir.path()).unwrap();
        assert_eq!(app_token(dir.path(), None).unwrap_err(), "Pointman App isn't set up on this board yet");
    }

    #[test]
    fn the_tokens_file_is_only_its_users_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Kept::read(dir.path()), None);
        let k = kept(now());
        k.write(dir.path()).unwrap();
        assert_eq!(Kept::read(dir.path()), Some(k));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tokens_file(dir.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// A state folder whose `cli` is a stand-in `gh`: `auth git-credential get` answers as the machine's login.
    fn with_machine_login(dir: &Path, signed_in: bool) {
        let gh = dir.join("gh");
        let answer = if signed_in { "echo username=jamescalam; echo password=gho_machine" } else { "exit 1" };
        std::fs::write(
            &gh,
            format!("#!/bin/sh\n[ \"$*\" = \"auth git-credential get\" ] && {{ cat >/dev/null; {answer}; }}\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(cli_file(dir), gh.display().to_string()).unwrap();
    }

    #[test]
    fn git_gets_the_paths_owners_token_else_the_machines_login() {
        let dir = tempfile::tempdir().unwrap();
        kept(now()).write(dir.path()).unwrap();
        let ours = "protocol=https\nhost=github.com\npath=jamescalam/pointman-cve.git\n\n";
        assert_eq!(
            git_credential(dir.path(), "get", ours, Login::AppThenMachine).unwrap(),
            "username=x-access-token\npassword=ghs_jamescalam\n"
        );
        assert_eq!(
            git_credential(dir.path(), "get", "protocol=https\nhost=gitlab.com\npath=a/b\n", Login::AppThenMachine)
                .unwrap(),
            ""
        );
        assert_eq!(git_credential(dir.path(), "store", "host=github.com\n", Login::AppThenMachine).unwrap(), "");

        // an account the App isn't on, with no gh signed in here: no password, and why
        let elsewhere = "host=github.com\npath=rust-lang/rust.git\n";
        let err = git_credential(dir.path(), "get", elsewhere, Login::AppThenMachine).unwrap_err();
        assert!(
            format!("{err:#}").contains("isn't installed on rust-lang, and this machine's own gh login gave none"),
            "{err:#}"
        );
        with_machine_login(dir.path(), false);
        assert!(git_credential(dir.path(), "get", elsewhere, Login::AppThenMachine).is_err());

        // with gh signed in: the machine's login, for that account, and for any once GitHub refused the App
        with_machine_login(dir.path(), true);
        let machine = "username=jamescalam\npassword=gho_machine\n";
        assert_eq!(git_credential(dir.path(), "get", elsewhere, Login::AppThenMachine).unwrap(), machine);
        assert_eq!(git_credential(dir.path(), "get", ours, Login::Machine).unwrap(), machine);

        // no App on the board, or no tokens yet: the machine's login too
        let none = tempfile::tempdir().unwrap();
        assert!(git_credential(none.path(), "get", "host=github.com\npath=a/b\n", Login::AppThenMachine).is_err());
        with_machine_login(none.path(), true);
        assert_eq!(
            git_credential(none.path(), "get", "host=github.com\npath=a/b\n", Login::AppThenMachine).unwrap(),
            machine
        );
        Kept::default().write(none.path()).unwrap();
        assert_eq!(
            git_credential(none.path(), "get", "host=github.com\npath=a/b\n", Login::AppThenMachine).unwrap(),
            machine
        );

        // App-only (workflow steps): today's error, and the machine's login is never asked
        let err = git_credential(dir.path(), "get", elsewhere, Login::AppOnly).unwrap_err();
        assert_eq!(format!("{err:#}"), "no GitHub password for git: Pointman App isn't installed on rust-lang");
        assert!(git_credential(none.path(), "get", "host=github.com\npath=a/b\n", Login::AppOnly).is_err());
        assert_eq!(
            git_credential(dir.path(), "get", ours, Login::AppOnly).unwrap(),
            "username=x-access-token\npassword=ghs_jamescalam\n"
        );
    }

    #[test]
    fn github_refusing_the_app_is_told_apart_from_other_failures() {
        let push =
            "To https://github.com/a/b.git\nremote: Permission to a/b.git denied to pointman-app[bot].\nfatal: x";
        assert_eq!(refusal(128, push), Some("remote: Permission to a/b.git denied to pointman-app[bot]."));
        assert_eq!(refusal(0, push), None, "only a failure");
        for refusal_text in [
            "GraphQL: Resource not accessible by integration (createPullRequest)",
            "HTTP 403: Resource not accessible by integration (https://api.github.com/repos/a/b/statuses/x)",
            "HTTP 404: Not Found (https://api.github.com/repos/a/b)",
            "GraphQL: Could not resolve to a Repository with the name 'a/b'. (repository)",
            "remote: Repository not found.\nfatal: repository 'https://github.com/a/b.git/' not found",
            "remote: Permission to a/b.git denied to pointman-app[bot].\nfatal: unable to access 'https://github.com/a/b.git/': The requested URL returned error: 403",
        ] {
            assert!(refusal(1, refusal_text).is_some(), "{refusal_text}");
        }
        for other in [
            "GraphQL: Could not resolve to a PullRequest with the number of 9. (repository.pullRequest)",
            "a pull request for branch \"x\" into branch \"main\" already exists",
            "! [rejected]        main -> main (non-fast-forward)",
            "error: pathspec 'x' did not match any file(s) known to git",
        ] {
            assert_eq!(refusal(1, other), None, "{other}");
        }
    }

    #[test]
    fn only_reads_run_again() {
        for cmd in [
            "pr view 4 -R a/b",
            "pr list",
            "pr checks 4",
            "pr diff 4",
            "run watch 9",
            "issue status",
            "search prs is:open",
            "status",
            "api repos/a/b/pulls",
            "api repos/a/b/issues -X GET -f state=open",
            "api --method=get repos/a/b",
        ] {
            assert!(only_reads(&args(cmd)), "{cmd}");
        }
        for cmd in [
            "pr create --title x",
            "pr merge 4 --delete-branch",
            "issue create --label bug",
            "release create v1 a.tgz",
            "pr review 4 --approve",
            "api repos/a/b/issues -f title=x",
            "api repos/a/b/statuses/s -F state=success",
            "api -XPOST repos/a/b/forks",
            "api --method DELETE repos/a/b/git/refs/heads/x",
            "api graphql -f query=x",
            "api repos/a/b/contents/x --input f.json",
            "",
        ] {
            assert!(!only_reads(&args(cmd)), "{cmd}");
        }
    }

    #[test]
    fn a_command_reading_stdin_isnt_run_twice() {
        for cmd in
            ["pr create --body-file -", "api repos/a/b/issues --input=-", "api x -F body=@-", "issue create -F -"]
        {
            assert!(reads_stdin(&args(cmd)), "{cmd}");
        }
        assert!(!reads_stdin(&args("pr create --title a-b --body c")));
    }

    #[test]
    fn only_git_commands_that_talk_to_github_run_again() {
        for cmd in ["push origin x", "-C /r fetch", "-c a=b pull", "clone https://github.com/a/b", "ls-remote origin"] {
            assert!(talks_to_github(&args(cmd)), "{cmd}");
        }
        for cmd in ["commit -m push", "status", "log --all", "-C /r rebase origin/main"] {
            assert!(!talks_to_github(&args(cmd)), "{cmd}");
        }
    }

    #[test]
    fn the_shim_hands_gh_to_the_extension_and_survives_odd_paths() {
        let gh = Path::new("/opt/homebrew/bin/gh");
        let s = shim(Path::new("/x/it's here/github"), Path::new("/state"), Some(gh));
        assert!(s.ends_with("exec '/x/it'\\''s here/github' gh '/state' -- \"$@\"\n"), "{s}");
        let none = shim(Path::new("/e/github"), Path::new("/state"), None);
        assert!(none.ends_with("installing it\" >&2\nexit 1\n"), "{none}");
    }

    #[test]
    fn the_cli_is_kept_beside_the_shims_and_goes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("gh");
        std::fs::write(&gh, "").unwrap();
        write_shim(Path::new("/e/github"), dir.path(), Some(&gh), &json!({})).unwrap();
        assert_eq!(machine_cli(dir.path()), Some(gh));
        write_shim(Path::new("/e/github"), dir.path(), None, &json!({})).unwrap();
        assert_eq!(machine_cli(dir.path()), None);
    }

    fn app() -> App {
        App {
            slug: "pointman-app".into(),
            bot_name: "pointman-app[bot]".into(),
            bot_email: "1+pointman-app[bot]@users.noreply.github.com".into(),
            owners: vec!["pointman-ai".into()],
        }
    }

    #[test]
    fn a_run_gets_gh_first_git_through_the_helper_and_the_bots_name() {
        let env = run_env(Some(&app()), Path::new("/e/bin/github"), Path::new("/s"));
        assert!(!env.contains_key("PATH"), "the manifest names the shim folder");
        assert_eq!(env["GIT_AUTHOR_NAME"], "pointman-app[bot]");
        assert_eq!(env["GIT_COMMITTER_EMAIL"], "1+pointman-app[bot]@users.noreply.github.com");
        assert_eq!(env["GIT_TERMINAL_PROMPT"], "0");
        assert_eq!(env["GIT_CONFIG_COUNT"], "5");
        assert_eq!(env["GIT_CONFIG_VALUE_0"], "");
        assert_eq!(env["GIT_CONFIG_VALUE_1"], "!'/e/bin/github' git-credential '/s'");
        assert_eq!(env["GIT_CONFIG_KEY_3"], "url.https://github.com/.pushInsteadOf");
        assert!(!env.contains_key("GH_TOKEN"), "gh gets its token from the shim, per command");
    }

    #[test]
    fn the_shims_git_signs_in_by_itself_before_its_arguments() {
        let got = signed_in(Path::new("/e/bin/github"), Path::new("/s"), &args("push origin main"));
        assert_eq!(
            got[..4],
            [
                "-c",
                "credential.https://github.com.helper=",
                "-c",
                "credential.https://github.com.helper=!'/e/bin/github' git-credential '/s'"
            ]
        );
        assert_eq!(got.len(), 13);
        assert_eq!(got[10..], args("push origin main")[..]);
        // a commit's place is still found past the -c's, so a person's commit is still credited
        assert_eq!(commit_at(&signed_in(Path::new("/e"), Path::new("/s"), &args("commit -m x"))), Some(10));
    }

    #[test]
    fn without_the_app_a_run_still_gets_the_shim_and_the_helper_but_no_bot() {
        let with = run_env(Some(&app()), Path::new("/e/bin/github"), Path::new("/s"));
        let without = run_env(None, Path::new("/e/bin/github"), Path::new("/s"));
        let bot = ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL"];
        assert!(bot.iter().all(|k| !without.contains_key(*k)));
        assert_eq!(with.into_iter().filter(|(k, _)| !bot.contains(&k.as_str())).collect::<BTreeMap<_, _>>(), without);
    }

    #[test]
    fn the_boards_answer_needs_a_bot() {
        let v = json!({"slug": "pointman-app", "bot": {"name": "pointman-app[bot]", "email": "1+pointman-app[bot]@users.noreply.github.com"},
                       "installations": [{"owner": "pointman-ai", "id": 1}, {"owner": "jamescalam", "id": 2}]});
        assert_eq!(App::from_answer(&v).unwrap().owners, vec!["pointman-ai", "jamescalam"]);
        let mut none = v.clone();
        none["installations"] = json!([]);
        assert_eq!(App::from_answer(&none).unwrap().owners, Vec::<String>::new());
        assert_eq!(App::from_answer(&json!({"error": {"code": "no_app"}})), None);
    }
}
