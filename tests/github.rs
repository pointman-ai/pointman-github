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
    let asked: Vec<String> = node.asked().into_iter().map(|(method, _)| method).collect();
    assert_eq!(asked, ["secret.list", "secret.get"], "listed and opened once");
}
