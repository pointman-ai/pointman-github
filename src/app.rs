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

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
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
    /// `GET /v1/node/github/app`'s answer. None without a bot or an installation: nothing to act as.
    pub fn from_answer(v: &Value) -> Option<App> {
        let s = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let owners: Vec<String> = v["installations"]
            .as_array()
            .map(|a| a.iter().filter_map(|i| s(&i["owner"])).collect())
            .unwrap_or_default();
        let app =
            App { slug: s(&v["slug"])?, bot_name: s(&v["bot"]["name"])?, bot_email: s(&v["bot"]["email"])?, owners };
        (!app.owners.is_empty()).then_some(app)
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

/// The tokens kept, and the owner whose token is used when a command names no repo (the first installation).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    pub default: Option<String>,
    pub tokens: BTreeMap<String, Token>,
}

impl Kept {
    pub fn read(state: &Path) -> Kept {
        let v: Value = std::fs::read_to_string(tokens_file(state))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let tokens = v["tokens"]
            .as_object()
            .map(|o| {
                o.values()
                    .filter_map(|t| {
                        Some(Token {
                            owner: t["owner"].as_str()?.to_string(),
                            token: t["token"].as_str()?.to_string(),
                            expires: t["expires"].as_i64()?,
                        })
                    })
                    .map(|t| (t.owner.to_lowercase(), t))
                    .collect()
            })
            .unwrap_or_default();
        Kept { default: v["default"].as_str().map(str::to_string), tokens }
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
        let body = json!({"default": self.default, "tokens": tokens}).to_string();
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

    /// The token for `owner` (any case), else the default owner's, if it has a minute left.
    pub fn token_for(&self, owner: Option<&str>, at: i64) -> Result<&Token, String> {
        let pick = owner
            .map(str::to_lowercase)
            .filter(|o| self.tokens.contains_key(o))
            .or_else(|| self.default.as_deref().map(str::to_lowercase));
        let Some(key) = pick else {
            return Err("Pointman App isn't installed on any GitHub account yet".into());
        };
        match self.tokens.get(&key) {
            Some(t) if t.expires - at >= USABLE_SECS => Ok(t),
            Some(t) => Err(format!(
                "Pointman App's token for {} has run out: is the github extension running on this machine?",
                t.owner
            )),
            None => Err(format!("there's no Pointman App token for {key} yet")),
        }
    }

    /// The owners whose token is missing or runs out within RENEW_SECS.
    pub fn due<'a>(&self, owners: &'a [String], at: i64) -> Vec<&'a String> {
        owners
            .iter()
            .filter(|o| self.tokens.get(&o.to_lowercase()).is_none_or(|t| t.expires - at < RENEW_SECS))
            .collect()
    }
}

// ----------------------------------------------------------------------------------- which account a command is for

/// The owner in a GitHub repo's address: `git@github.com:o/r.git`, `ssh://git@github.com/o/r`,
/// `https://[user@]github.com/o/r[.git]`, or the short `o/r` and `github.com/o/r`.
pub fn owner_of(s: &str) -> Option<String> {
    let s = s.trim();
    let rest = if let Some(r) = s.strip_prefix("git@github.com:") {
        r
    } else if let Some(i) = s.find("github.com") {
        let before = &s[..i];
        // the host itself, not a repo of that name or a path that only mentions it
        if !(before.is_empty() || before.ends_with("://") || before.ends_with('@')) {
            return None;
        }
        s[i + "github.com".len()..].trim_start_matches([':', '/'])
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

/// The account a `gh` command is about, from its words: `-R`/`--repo`, a github.com link, an API path
/// (`repos/o/…`, `orgs/o`, `users/o`), or the `o/r` a `gh repo …` names. None when they don't say.
pub fn owner_in_args(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-R" || a == "--repo" {
            if let Some(o) = args.get(i + 1).and_then(|v| owner_of(v)) {
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
    for a in args {
        if a.contains("github.com/") {
            if let Some(o) = owner_of(a) {
                return Some(o);
            }
        }
    }
    if args.first().map(String::as_str) == Some("api") {
        for a in &args[1..] {
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
    if args.first().map(String::as_str) == Some("repo") {
        if let Some(o) = args[1..].iter().filter(|a| !a.starts_with('-')).skip(1).find_map(|a| owner_of(a)) {
            return Some(o);
        }
    }
    None
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
        nix.extend(
            user.filter(|u| !u.is_empty() && !u.contains('/')).map(|u| format!("/etc/profiles/per-user/{u}/bin")),
        );
        nix.push("/run/current-system/sw/bin".into());
        dirs.extend(nix.into_iter().filter(|d| Path::new(d).is_dir()));
    }
    dirs.push("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".into());
    dirs.join(":")
}

/// `s` quoted for sh, whatever it holds.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The folder whose `gh` a run finds first.
pub fn shim_dir(state: &Path) -> PathBuf {
    state.join("app").join("bin")
}

/// The `gh` a run finds first: the App's token for the repo's owner, then GitHub's own CLI with it.
pub fn shim(exe: &Path, state: &Path, gh: &Path) -> String {
    let (exe, state, gh) = (
        sh_quote(&exe.display().to_string()),
        sh_quote(&state.display().to_string()),
        sh_quote(&gh.display().to_string()),
    );
    format!(
        "#!/bin/sh\n\
         # GitHub's CLI as Pointman App, for agents' runs: the github extension's token for the repo's owner (MB-707)\n\
         token=$({exe} token {state} -- \"$@\") || exit 1\n\
         GH_TOKEN=$token exec {gh} \"$@\"\n"
    )
}

pub fn write_shim(exe: &Path, state: &Path, gh: &Path) -> anyhow::Result<PathBuf> {
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

/// What a run's environment gets in App mode: `gh` first on its PATH, git asking this extension for GitHub's
/// password, pushes over https, and commits as the bot.
pub fn run_env(app: &App, exe: &Path, state: &Path, path: String) -> BTreeMap<String, String> {
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
        ("GIT_AUTHOR_NAME".into(), app.bot_name.clone()),
        ("GIT_AUTHOR_EMAIL".into(), app.bot_email.clone()),
        ("GIT_COMMITTER_NAME".into(), app.bot_name.clone()),
        ("GIT_COMMITTER_EMAIL".into(), app.bot_email.clone()),
        ("GIT_CONFIG_COUNT".into(), config.len().to_string()),
    ]);
    for (i, (k, v)) in config.into_iter().enumerate() {
        env.insert(format!("GIT_CONFIG_KEY_{i}"), k.to_string());
        env.insert(format!("GIT_CONFIG_VALUE_{i}"), v);
    }
    env
}

// ------------------------------------------------------------------------------------- the helpers runs call

/// `bin/github token <state> -- <gh's arguments>`: the token for the account the command is about.
pub fn token_command(state: &Path, args: &[String], cwd: &Path) -> anyhow::Result<String> {
    let owner = owner_in_args(args)
        .or_else(|| std::env::var("GH_REPO").ok().and_then(|r| owner_of(&r)))
        .or_else(|| owner_of_folder(cwd));
    match Kept::read(state).token_for(owner.as_deref(), now()) {
        Ok(t) => Ok(t.token.clone()),
        Err(why) => bail!("{why}"),
    }
}

/// `bin/github git-credential <state> get`: git's credential helper for https://github.com. It answers `get` with
/// the token for the path's owner (`useHttpPath`); `store` and `erase` do nothing.
pub fn git_credential(state: &Path, op: &str, input: &str) -> anyhow::Result<String> {
    if op != "get" {
        return Ok(String::new());
    }
    let fields: BTreeMap<&str, &str> = input.lines().filter_map(|l| l.split_once('=')).collect();
    if fields.get("host").copied() != Some("github.com") {
        return Ok(String::new());
    }
    let owner = fields.get("path").and_then(|p| owner_of(p));
    let kept = Kept::read(state);
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
            default: Some("pointman-ai".into()),
            tokens: BTreeMap::from([t("pointman-ai", 3000), t("Alai-Studios", 30), t("jamescalam", 900)]),
        }
    }

    #[test]
    fn a_token_is_the_owners_else_the_defaults_and_never_one_about_to_run_out() {
        let k = kept(1000);
        assert_eq!(k.token_for(Some("JamesCalam"), 1000).unwrap().token, "ghs_jamescalam");
        assert_eq!(k.token_for(Some("cli"), 1000).unwrap().token, "ghs_pointman-ai", "not installed there");
        assert_eq!(k.token_for(None, 1000).unwrap().token, "ghs_pointman-ai");
        assert!(k.token_for(Some("alai-studios"), 1000).unwrap_err().contains("run out"));
        assert!(Kept::default().token_for(None, 1000).unwrap_err().contains("isn't installed"));
        let owners: Vec<String> =
            ["pointman-ai", "Alai-Studios", "jamescalam", "spacesheep-dev"].map(String::from).into();
        assert_eq!(k.due(&owners, 1000), vec!["Alai-Studios", "spacesheep-dev"]);
    }

    #[test]
    fn the_tokens_file_is_only_its_users_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let k = kept(now());
        k.write(dir.path()).unwrap();
        assert_eq!(Kept::read(dir.path()), k);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(tokens_file(dir.path())).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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
        let none = tempfile::tempdir().unwrap();
        assert!(git_credential(none.path(), "get", "host=github.com\npath=a/b\n").is_err());
    }

    #[test]
    fn the_shim_runs_gh_with_the_token_and_survives_odd_paths() {
        let s = shim(Path::new("/x/it's here/github"), Path::new("/state"), Path::new("/opt/homebrew/bin/gh"));
        assert!(s.contains(r"token=$('/x/it'\''s here/github' token '/state' -- "));
        assert!(s.ends_with("GH_TOKEN=$token exec '/opt/homebrew/bin/gh' \"$@\"\n"));
    }

    #[test]
    fn a_run_gets_gh_first_git_through_the_helper_and_the_bots_name() {
        let app = App {
            slug: "pointman-app".into(),
            bot_name: "pointman-app[bot]".into(),
            bot_email: "1+pointman-app[bot]@users.noreply.github.com".into(),
            owners: vec!["pointman-ai".into()],
        };
        let env = run_env(&app, Path::new("/e/bin/github"), Path::new("/s"), "/s/app/bin:/usr/bin".into());
        assert_eq!(env["PATH"], "/s/app/bin:/usr/bin");
        assert_eq!(env["GIT_AUTHOR_NAME"], "pointman-app[bot]");
        assert_eq!(env["GIT_COMMITTER_EMAIL"], "1+pointman-app[bot]@users.noreply.github.com");
        assert_eq!(env["GIT_CONFIG_COUNT"], "5");
        assert_eq!(env["GIT_CONFIG_VALUE_0"], "");
        assert_eq!(env["GIT_CONFIG_VALUE_1"], "!'/e/bin/github' git-credential '/s'");
        assert_eq!(env["GIT_CONFIG_KEY_3"], "url.https://github.com/.pushInsteadOf");
        assert!(!env.contains_key("GH_TOKEN"), "gh gets its token from the shim, per command");
    }

    #[test]
    fn the_boards_answer_needs_a_bot_and_an_installation() {
        let v = json!({"slug": "pointman-app", "bot": {"name": "pointman-app[bot]", "email": "1+pointman-app[bot]@users.noreply.github.com"},
                       "installations": [{"owner": "pointman-ai", "id": 1}, {"owner": "jamescalam", "id": 2}]});
        assert_eq!(App::from_answer(&v).unwrap().owners, vec!["pointman-ai", "jamescalam"]);
        let mut none = v.clone();
        none["installations"] = json!([]);
        assert_eq!(App::from_answer(&none), None);
        assert_eq!(App::from_answer(&json!({"error": {"code": "no_app"}})), None);
    }

    #[test]
    fn the_runs_path_starts_with_the_shim_then_the_nodes() {
        let p = run_path(Path::new("/s/app/bin"), Path::new("/Users/j"), Some("j"));
        assert!(p.starts_with("/s/app/bin:/Users/j/.local/bin:"));
        assert!(p.ends_with("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"));
    }
}
