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

/// GitHub's pages open in the apps' web panel (James, Pointman thread 8446): no viewer here any more, and
/// `/github add` is still the extension's.
#[test]
fn the_viewer_is_gone_and_add_stays() {
    let node = node();
    assert!(node.run("github.view", json!({"url": "https://github.com/a/b/pull/42"})).is_err());
    assert!(node.open_pane("view", json!({"url": "https://github.com/a/b/pull/42"})).is_err());
    let added = node.run("github.add", json!({})).unwrap().result;
    assert!(added["text"].as_str().unwrap().contains("/github add"), "{added}");
}

/// The real `gh` on this machine starts a sign-in: GitHub's link and a one-time code, which nobody enters
/// here, so nothing is signed in. Run by hand: `cargo test --test github real_sign_in_starts -- --ignored`.
#[test]
#[ignore]
fn real_sign_in_starts() {
    let gh =
        ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"].into_iter().map(std::path::PathBuf::from).find(|p| p.exists());
    let gh = gh.expect("gh on this machine");
    let signins = github::signin::SignIns::default();
    let got = signins.start(&gh, "s1", std::time::Duration::from_secs(30)).unwrap();
    assert_eq!(got.url, "https://github.com/login/device");
    let prompt = got.prompt.as_deref().unwrap();
    assert!(regex::Regex::new(r"^Enter [A-Z0-9]{4}-[A-Z0-9]{4} at ").unwrap().is_match(prompt), "{prompt}");
    let wrong = signins.code(&gh, "s1", "AAAA-AAAA", std::time::Duration::from_secs(1)).unwrap_err().to_string();
    assert!(wrong.starts_with("that isn't this sign-in's code"), "{wrong}");
}

/// Agents' runs get the vault's token, and the vault's list is kept: two runs starting together ask for it
/// once (each ask is a round trip to core that a run's start waited on).
#[test]
fn runs_get_the_vaults_token_and_its_list_is_kept() {
    let token = format!("ghp_{}", "a1".repeat(18));
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_github"))
        .settings(json!({"cli": fake_gh().to_string_lossy()}))
        .secret("github.work", format!("token: {token}"))
        .start()
        .unwrap();
    for _ in 0..2 {
        let got = node.call("session.prepare", json!({"client": "claude", "handle": {"handle": "scene"}})).unwrap();
        assert_eq!(got, json!({"env": {"GH_TOKEN": token}}));
    }
    let asked: Vec<String> =
        node.asked().into_iter().map(|(method, _)| method).filter(|m| m.starts_with("secret.")).collect();
    assert_eq!(asked, ["secret.list", "secret.get"], "listed and opened once");
}

/// A `gh` that says which token it was given and what for.
fn telling_gh(dir: &std::path::Path) -> PathBuf {
    let gh = dir.join("gh");
    std::fs::write(&gh, "#!/bin/sh\necho \"$GH_TOKEN $*\"\n").unwrap();
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
/// git gets it as GitHub's password, commits are the bot's, and the vault's account isn't given at all.
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
    let tokens = github::app::tokens_file(&state);
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !(tokens.exists() && state.join("app/bin/gh").exists()) && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let got = node.call("session.prepare", json!({"client": "claude", "handle": {"handle": "scene"}})).unwrap();
    let env = &got["env"];
    assert!(env.get("GH_TOKEN").is_none(), "{got}");
    let shims = state.join("app/bin");
    assert!(env["PATH"].as_str().unwrap().starts_with(&format!("{}:", shims.display())), "{got}");
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
