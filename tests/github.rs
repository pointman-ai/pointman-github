//! The extension against a stand-in node (pointman-extension's testing::StandIn), started as a node starts
//! it, with a stand-in `gh`.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use pointman_extension::testing::StandIn;
use serde_json::json;

/// A `gh` that's there and does nothing: the extension only needs one so it doesn't install its own.
/// Written once: one test writing it while another runs it is "text file busy" on Linux.
fn fake_gh() -> PathBuf {
    static GH: OnceLock<PathBuf> = OnceLock::new();
    GH.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("pointman-github-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gh = dir.join("gh");
        std::fs::write(&gh, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        gh
    })
    .clone()
}

fn node() -> StandIn {
    StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_github"))
        .settings(json!({"cli": fake_gh().to_string_lossy()}))
        .start()
        .unwrap()
}

/// GitHub's pages open in the apps' web panel (James, Pointman thread 8446), and accounts aren't added here any
/// more (MB-707): no viewer, no `/github add`.
#[test]
fn the_viewer_and_add_are_gone() {
    let node = node();
    assert!(node.run("github.view", json!({"url": "https://github.com/a/b/pull/42"})).is_err());
    assert!(node.open_pane("view", json!({"url": "https://github.com/a/b/pull/42"})).is_err());
    assert!(node.run("github.add", json!({})).is_err());
}

/// Waits for the extension to write the runs' gh and what the board said.
fn settled(state: &std::path::Path) {
    let tokens = github::app::tokens_file(state);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(tokens.exists() && state.join("app/bin/gh").exists()) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Before the board has Pointman App, runs get no GitHub token, not even with an account in the vault: their `gh`
/// runs as this machine's own login and says why, git never asks the keychain, and commits aren't the bot's.
#[test]
fn before_the_board_has_the_app_runs_use_the_machines_own_login() {
    let dir = tempfile::tempdir().unwrap();
    let gh = telling_gh(dir.path());
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_github"))
        .settings(json!({"cli": gh.to_string_lossy()}))
        .secret("github.work", format!("ghp_{}", "a1".repeat(18)))
        .method("core.call", |_| Ok(json!({"status": 503, "body": {"error": {"code": "no_app"}}})))
        .start()
        .unwrap();
    let state = node.state_dir_path().to_path_buf();
    settled(&state);

    let got = node.call("session.prepare", json!({"client": "claude", "handle": {"handle": "scene"}})).unwrap();
    let env = &got["env"];
    assert!(env.get("GH_TOKEN").is_none(), "{got}");
    assert!(env.get("GIT_AUTHOR_NAME").is_none(), "{got}");
    let shims = state.join("app/bin");
    assert!(env.get("PATH").is_none(), "the manifest names the folder, not PATH: {got}");
    assert_eq!(env["GIT_CONFIG_VALUE_0"], "", "the keychain's helper is reset");

    let out = std::process::Command::new(shims.join("gh"))
        .args(["pr", "list"])
        .env("GH_TOKEN", "ghp_from_somewhere_else")
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), " pr list\n", "GitHub's CLI ran with no token");
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "github: Pointman App isn't set up on this board yet, so gh uses this machine's own login\n"
    );
    let asked: Vec<String> =
        node.asked().into_iter().map(|(method, _)| method).filter(|m| m.starts_with("secret.")).collect();
    assert!(asked.is_empty(), "the vault isn't opened: {asked:?}");
}

/// A `gh` that says which token it was given and what for, fails when it's asked to, and refuses the App's token (any
/// token) the way GitHub does when it's asked to.
fn telling_gh(dir: &std::path::Path) -> PathBuf {
    let gh = dir.join("gh");
    let script = "#!/bin/sh\necho \"$GH_TOKEN $*\"\n\
                  case \"$*\" in *refuse*) [ -n \"$GH_TOKEN\" ] && { echo 'HTTP 403: Resource not accessible by integration' >&2; exit 1; };; esac\n\
                  case \"$*\" in *fail*) exit 4;; esac\n";
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    gh
}

/// The board as it answers once it has Pointman App (MB-707), installed on two accounts.
fn board(params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let in_an_hour = "2099-01-01T00:00:00Z";
    Ok(match (params["method"].as_str(), params["path"].as_str()) {
        (Some("GET"), Some("/v1/node/github/app")) => json!({"status": 200, "body": {
            "slug": "pointman-app",
            "bot": {"name": "pointman-app[bot]", "email": "9+pointman-app[bot]@users.noreply.github.com"},
            "installations": [{"owner": "pointman-ai", "id": 1}, {"owner": "Alai-Studios", "id": 2}]}}),
        (Some("POST"), Some("/v1/node/github/token")) => {
            let owner = params["body"]["owner"].as_str().unwrap_or_default().to_string();
            json!({"status": 200, "body": {"owner": owner, "token": format!("ghs_{owner}"), "expires_at": in_an_hour}})
        }
        _ => json!({"status": 404, "body": {"error": {"code": "not_found"}}}),
    })
}

/// Once the board has Pointman App, runs act as it: `gh` first on their PATH gets the token for the repo's owner,
/// git gets it as GitHub's password, and commits are the bot's.
#[test]
fn runs_act_as_pointman_app_once_the_board_has_it() {
    let dir = tempfile::tempdir().unwrap();
    let gh = telling_gh(dir.path());
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_github"))
        .settings(json!({"cli": gh.to_string_lossy()}))
        .secret("github.work", format!("ghp_{}", "a1".repeat(18)))
        .method("core.call", board)
        .start()
        .unwrap();
    let state = node.state_dir_path().to_path_buf();
    settled(&state);

    let got = node.call("session.prepare", json!({"client": "claude", "handle": {"handle": "scene"}})).unwrap();
    let env = &got["env"];
    assert!(env.get("GH_TOKEN").is_none(), "{got}");
    let shims = state.join("app/bin");
    assert!(env.get("PATH").is_none(), "the manifest names the folder, not PATH: {got}");
    assert_eq!(env["GIT_AUTHOR_NAME"], "pointman-app[bot]");
    assert_eq!(env["GIT_COMMITTER_EMAIL"], "9+pointman-app[bot]@users.noreply.github.com");
    assert_eq!(env["GIT_CONFIG_COUNT"], "5");

    // the runs' gh: the token for the account the command names, else the first installation's
    let run = |args: &[&str]| {
        let out = std::process::Command::new(shims.join("gh")).args(args).current_dir(dir.path()).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(run(&["pr", "view", "-R", "alai-studios/spark"]), "ghs_Alai-Studios pr view -R alai-studios/spark\n");
    assert_eq!(run(&["api", "user"]), "ghs_pointman-ai api user\n");
    let body =
        ["pr", "create", "-R", "alai-studios/spark", "--body", "Fixes https://github.com/pointman-ai/x/issues/3"];
    assert!(run(&body).starts_with("ghs_Alai-Studios "), "a link in the body isn't the repo");

    // an account the App isn't on: gh runs as this machine's own login, and says so
    let gh_in = |args: &[&str]| {
        std::process::Command::new(shims.join("gh")).args(args).current_dir(dir.path()).output().unwrap()
    };
    let out = gh_in(&["pr", "create", "-R", "rust-lang/rust", "--title", "fail"]);
    assert_eq!(out.status.code(), Some(4), "gh's own exit code");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), " pr create -R rust-lang/rust --title fail\n");
    let why = String::from_utf8(out.stderr).unwrap();
    assert_eq!(why, "github: Pointman App isn't installed on rust-lang, so gh uses this machine's own login\n");

    // GitHub refuses the App's token: once more as this machine's own login, unless it read stdin
    let out = gh_in(&["pr", "create", "-R", "pointman-ai/x", "--title", "refuse"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let ran = "pr create -R pointman-ai/x --title refuse";
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("ghs_pointman-ai {ran}\n {ran}\n"));
    let why = String::from_utf8(out.stderr).unwrap();
    assert!(why.starts_with("HTTP 403: Resource not accessible by integration\n"), "gh's own words first: {why}");
    assert!(why.ends_with("so gh runs it again as this machine's own login\n"), "{why}");
    let out = gh_in(&["pr", "create", "-R", "pointman-ai/x", "--title", "refuse", "--body-file", "-"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 1, "run once");
    assert!(String::from_utf8(out.stderr).unwrap().contains("reads stdin, so it isn't run again"));
    let out = gh_in(&["pr", "view", "-R", "pointman-ai/x", "--title", "fail"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 1, "a failure that isn't a refusal runs once");

    // git's helper, as git calls it
    let mut git = std::process::Command::new(env!("CARGO_BIN_EXE_github"))
        .args(["git-credential", &state.to_string_lossy(), "get"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    git.stdin.take().unwrap().write_all(b"protocol=https\nhost=github.com\npath=Alai-Studios/spark.git\n\n").unwrap();
    let out = git.wait_with_output().unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "username=x-access-token\npassword=ghs_Alai-Studios\n");
}

/// A push GitHub refuses the App's token for runs once more, with git's helper told to answer as this machine's own
/// login; a refused command that doesn't talk to GitHub, and a push that fails otherwise, run once.
#[test]
fn a_push_github_refuses_the_app_for_runs_again_as_the_machines_login() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let git = bin.join("git");
    let script = "#!/bin/sh\necho \"ran ${POINTMAN_GITHUB_MACHINE_LOGIN:-app}\" >>\"$LOG\"\n\
                  [ -n \"$POINTMAN_GITHUB_MACHINE_LOGIN\" ] && exit 0\n\
                  case \"$*\" in *other*) echo 'error: failed to push some refs' >&2; exit 1;; esac\n\
                  echo \"remote: Permission to a/b.git denied to pointman-app[bot].\" >&2\n\
                  echo \"fatal: unable to access 'https://github.com/a/b.git/': The requested URL returned error: 403\" >&2\n\
                  exit 128\n";
    std::fs::write(&git, script).unwrap();
    std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
    let state = dir.path().join("state");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let log = dir.path().join("log");
    let run = |args: &[&str]| {
        let _ = std::fs::remove_file(&log);
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_github"))
            .args(["git", &state.to_string_lossy(), "--"])
            .args(args)
            .env("PATH", &path)
            .env("LOG", &log)
            .env_remove("POINTMAN_GITHUB_MACHINE_LOGIN")
            .output()
            .unwrap();
        (out.status.code(), std::fs::read_to_string(&log).unwrap_or_default(), String::from_utf8(out.stderr).unwrap())
    };

    let (code, ran, said) = run(&["push", "origin", "x"]);
    assert_eq!((code, ran.as_str()), (Some(0), "ran app\nran 1\n"), "{said}");
    assert!(said.contains("returned error: 403\n"), "git's own words are passed on: {said}");
    assert!(said.ends_with("so git runs it again with this machine's own gh login\n"), "{said}");

    let (code, ran, _) = run(&["push", "other", "x"]);
    assert_eq!((code, ran.as_str()), (Some(1), "ran app\n"));
    let (code, ran, _) = run(&["commit", "-m", "x"]);
    assert_eq!((code, ran.as_str()), (Some(128), "ran app\n"));
}
