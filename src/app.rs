//! Pointman App for agents' runs (MB-707; James, Pointman thread 8446 [26244], [26267]): what agents do on GitHub
//! goes out as the App's bot, not as a person's account. The board gives this machine hour-long installation tokens
//! (`GET /v1/node/github/app`, `POST /v1/node/github/token`), one for each account the App is installed on; the
//! App's key never leaves api.pointman.ai. The extension keeps them fresh in its state folder, and a run's `gh` and
//! git pick the one for the repo's owner:
//!
//! - `gh` is a small script first on the run's PATH (`app/bin/gh`), which asks `bin/github token` for the token and
//!   runs GitHub's CLI with it as GH_TOKEN.
//! - git asks `bin/github git-credential` for https://github.com (GIT_CONFIG_*), pushes to ssh remotes go over https
//!   (`pushInsteadOf`), and commits are the bot's (GIT_AUTHOR_*, GIT_COMMITTER_*).
//!
//! There's no other account to fall back on (MB-707 exists so runs stop acting as a person, thread 8446 [18054]):
//! until the board has the App, both say "Pointman App isn't set up on this board yet".

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
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

    /// The token for `owner` (any case), or the first installation's when there's no owner, if it has a minute left.
    pub fn token_for(&self, owner: Option<&str>, at: i64) -> Result<&Token, String> {
        if self.app.is_none() {
            return Err("Pointman App isn't set up on this board yet".into());
        }
        let key = match owner {
            Some(o) if !self.installed_on(o) => return Err(format!("Pointman App isn't installed on {o}")),
            Some(o) => o.to_lowercase(),
            None => match self.owners.first() {
                Some(o) => o.to_lowercase(),
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

/// The run's PATH the node gives every agent (the node's wakes::session_path), with `first` before it.
pub fn run_path(first: &Path, home: &Path, user: Option<&str>) -> String {
    let mut dirs = vec![first.display().to_string(), home.join(".local/bin").display().to_string()];
    if cfg!(target_os = "linux") {
        let mut nix = vec!["/run/wrappers/bin".to_string(), home.join(".nix-profile/bin").display().to_string()];
        nix.extend(nix_user(home, user).map(|u| format!("/etc/profiles/per-user/{u}/bin")));
        nix.push("/run/current-system/sw/bin".into());
        dirs.extend(nix.into_iter().filter(|d| Path::new(d).is_dir()));
    }
    dirs.push("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into());
    dirs.join(":")
}

/// The user whose NixOS profile a run's PATH has: USER, else the home folder's name, as the node finds it.
fn nix_user(home: &Path, user: Option<&str>) -> Option<String> {
    user.filter(|u| !u.is_empty() && !u.contains('/'))
        .map(str::to_string)
        .or_else(|| home.file_name().map(|n| n.to_string_lossy().into_owned()))
}

/// `s` quoted for sh, whatever it holds.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The folder whose `gh` a run finds first.
pub fn shim_dir(state: &Path) -> PathBuf {
    state.join("app").join("bin")
}

/// The `gh` a run finds first: the App's token for the repo's owner, then GitHub's own CLI with it, and why it
/// failed when that's the App's reach. With no CLI on the machine yet, it says so, and never lets a run find
/// another `gh` signed in as a person.
pub fn shim(exe: &Path, state: &Path, gh: Option<&Path>) -> String {
    let head = "#!/bin/sh\n\
                # GitHub's CLI as Pointman App, for agents' runs: the github extension's token for the repo's owner (MB-707)\n";
    let Some(gh) = gh else {
        return format!(
            "{head}echo \"gh: GitHub's CLI isn't on this machine yet: the github extension is installing it\" >&2\n\
             exit 1\n"
        );
    };
    let (exe, state, gh) = (
        sh_quote(&exe.display().to_string()),
        sh_quote(&state.display().to_string()),
        sh_quote(&gh.display().to_string()),
    );
    format!(
        "{head}token=$({exe} token {state} -- \"$@\") || exit 1\n\
         GH_TOKEN=$token {gh} \"$@\"\n\
         code=$?\n\
         [ \"$code\" = 0 ] || {exe} why {state} -- \"$@\" >&2\n\
         exit \"$code\"\n"
    )
}

pub fn write_shim(exe: &Path, state: &Path, gh: Option<&Path>) -> anyhow::Result<PathBuf> {
    let dir = shim_dir(state);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("gh");
    let text = shim(exe, state, gh);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        let mut f = tempfile::NamedTempFile::new_in(&dir)?;
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

/// What a run's environment gets: `gh` first on its PATH, git asking this extension for GitHub's password (never a
/// person's keychain, and never a prompt), pushes over https, and, once the board has the App, commits as its bot.
pub fn run_env(app: Option<&App>, exe: &Path, state: &Path, path: String) -> BTreeMap<String, String> {
    let helper =
        format!("!{} git-credential {}", sh_quote(&exe.display().to_string()), sh_quote(&state.display().to_string()));
    let config = [
        // an empty helper first, so a person's own (the keychain's) isn't asked before this one
        ("credential.https://github.com.helper", String::new()),
        ("credential.https://github.com.helper", helper),
        ("credential.https://github.com.useHttpPath", "true".into()),
        ("url.https://github.com/.pushInsteadOf", "git@github.com:".into()),
        ("url.https://github.com/.pushInsteadOf", "ssh://git@github.com/".into()),
    ];
    let mut env = BTreeMap::from([
        ("PATH".to_string(), path),
        ("GIT_TERMINAL_PROMPT".into(), "0".into()),
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

/// `bin/github token <state> -- <gh's arguments>`: the token for the account the command is about. For an account
/// the App isn't installed on, the first installation's, which reads public repos there (`why` says so if gh fails).
pub fn token_command(state: &Path, args: &[String], cwd: &Path) -> anyhow::Result<String> {
    let kept = Kept::read(state).context(NOT_YET)?;
    let owner = command_owner(args, cwd).filter(|o| kept.installed_on(o));
    kept.token_for(owner.as_deref(), now()).map(|t| t.token.clone()).map_err(anyhow::Error::msg)
}

/// `bin/github why <state> -- <gh's arguments>`, after gh failed: a line when the account it was about is one the
/// App isn't installed on, else nothing.
pub fn why_command(state: &Path, args: &[String], cwd: &Path) -> String {
    let Some(kept) = Kept::read(state) else { return String::new() };
    match (command_owner(args, cwd), kept.owners.first()) {
        (Some(o), Some(first)) if !kept.installed_on(&o) => format!(
            "github: Pointman App isn't installed on {o}: gh asked with {first}'s token, which can only read its public repos\n"
        ),
        _ => String::new(),
    }
}

/// `bin/github git-credential <state> get`: git's credential helper for https://github.com. It answers `get` with
/// the token for the path's owner (`useHttpPath`), and fails for an account the App isn't installed on; `store` and
/// `erase` do nothing.
pub fn git_credential(state: &Path, op: &str, input: &str) -> anyhow::Result<String> {
    if op != "get" {
        return Ok(String::new());
    }
    let fields: BTreeMap<&str, &str> = input.lines().filter_map(|l| l.split_once('=')).collect();
    if fields.get("host").copied() != Some("github.com") {
        return Ok(String::new());
    }
    let owner = fields.get("path").and_then(|p| owner_of(p));
    let kept = Kept::read(state).context(NOT_YET).context("no GitHub password for git")?;
    let t =
        kept.token_for(owner.as_deref(), now()).map_err(anyhow::Error::msg).context("no GitHub password for git")?;
    Ok(format!("username=x-access-token\npassword={}\n", t.token))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = token_command(dir.path(), &args("pr list"), dir.path()).unwrap_err().to_string();
        assert!(err.contains("aren't on this machine yet"), "{err}");
        no_app.write(dir.path()).unwrap();
        let err = token_command(dir.path(), &args("pr list"), dir.path()).unwrap_err().to_string();
        assert_eq!(err, "Pointman App isn't set up on this board yet");
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

    #[test]
    fn gh_reads_with_the_first_installations_token_where_the_app_isnt_and_says_so_when_it_fails() {
        let dir = tempfile::tempdir().unwrap();
        kept(now()).write(dir.path()).unwrap();
        let other = args("pr view 1 -R rust-lang/rust");
        assert_eq!(token_command(dir.path(), &other, dir.path()).unwrap(), "ghs_pointman-ai");
        assert_eq!(
            why_command(dir.path(), &other, dir.path()),
            "github: Pointman App isn't installed on rust-lang: gh asked with pointman-ai's token, which can only read its public repos\n"
        );
        let ours = args("pr view 1 -R jamescalam/pointman-cve");
        assert_eq!(token_command(dir.path(), &ours, dir.path()).unwrap(), "ghs_jamescalam");
        assert_eq!(why_command(dir.path(), &ours, dir.path()), "");
    }

    #[test]
    fn git_gets_the_paths_owners_token_and_nothing_for_other_hosts() {
        let dir = tempfile::tempdir().unwrap();
        kept(now()).write(dir.path()).unwrap();
        let got =
            git_credential(dir.path(), "get", "protocol=https\nhost=github.com\npath=jamescalam/pointman-cve.git\n\n")
                .unwrap();
        assert_eq!(got, "username=x-access-token\npassword=ghs_jamescalam\n");
        assert_eq!(git_credential(dir.path(), "get", "protocol=https\nhost=gitlab.com\npath=a/b\n").unwrap(), "");
        assert_eq!(git_credential(dir.path(), "store", "host=github.com\n").unwrap(), "");
        let elsewhere = git_credential(dir.path(), "get", "host=github.com\npath=rust-lang/rust.git\n").unwrap_err();
        assert!(format!("{elsewhere:#}").contains("isn't installed on rust-lang"), "{elsewhere:#}");
        let none = tempfile::tempdir().unwrap();
        assert!(git_credential(none.path(), "get", "host=github.com\npath=a/b\n").is_err());
        Kept::default().write(none.path()).unwrap();
        let no_app = git_credential(none.path(), "get", "host=github.com\npath=a/b\n").unwrap_err();
        assert!(format!("{no_app:#}").contains("isn't set up on this board yet"), "{no_app:#}");
    }

    #[test]
    fn the_shim_runs_gh_with_the_token_and_survives_odd_paths() {
        let gh = Path::new("/opt/homebrew/bin/gh");
        let s = shim(Path::new("/x/it's here/github"), Path::new("/state"), Some(gh));
        assert!(s.contains(r"token=$('/x/it'\''s here/github' token '/state' -- "));
        assert!(s.contains("GH_TOKEN=$token '/opt/homebrew/bin/gh' \"$@\"\n"));
        assert!(s.contains(r#"|| '/x/it'\''s here/github' why '/state' -- "$@" >&2"#));
        assert!(s.ends_with("exit \"$code\"\n"));
        let none = shim(Path::new("/e/github"), Path::new("/state"), None);
        assert!(none.ends_with("installing it\" >&2\nexit 1\n"), "{none}");
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
        let env = run_env(Some(&app()), Path::new("/e/bin/github"), Path::new("/s"), "/s/app/bin:/usr/bin".into());
        assert_eq!(env["PATH"], "/s/app/bin:/usr/bin");
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
    fn without_the_app_a_run_still_gets_the_shim_and_the_helper_but_no_bot() {
        let with = run_env(Some(&app()), Path::new("/e/bin/github"), Path::new("/s"), "/s/app/bin:/usr/bin".into());
        let without = run_env(None, Path::new("/e/bin/github"), Path::new("/s"), "/s/app/bin:/usr/bin".into());
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

    #[test]
    fn the_runs_path_starts_with_the_shim_then_the_nodes() {
        let p = run_path(Path::new("/s/app/bin"), Path::new("/Users/j"), Some("j"));
        assert!(p.starts_with("/s/app/bin:/Users/j/.local/bin:"));
        assert!(p.ends_with("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"));
    }

    #[test]
    fn the_nix_profile_is_users_else_the_home_folders_as_the_node_finds_it() {
        let home = Path::new("/home/james");
        assert_eq!(nix_user(home, Some("j")).as_deref(), Some("j"));
        assert_eq!(nix_user(home, None).as_deref(), Some("james"));
        assert_eq!(nix_user(home, Some("")).as_deref(), Some("james"));
        assert_eq!(nix_user(home, Some("a/b")).as_deref(), Some("james"));
    }
}
