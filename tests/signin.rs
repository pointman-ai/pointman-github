//! Signing in through GitHub's device flow (src/signin.rs), with a stand-in `gh`: the sign-in waits (EXT-69), so
//! the person only enters the code at GitHub, and a node from before that can still have the same code pasted back.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use github::signin::SignIns;

const TOKEN: &str = "gho_0123456789abcdefghijklmnopqrstuvwxyz";

/// A `gh` whose `auth login` gives a device code and, a moment later, GitHub's yes; `auth token` the token.
fn fake_gh(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pointman-github-signin-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let gh = dir.join("gh");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\ncase \"$2\" in\n  login) echo '! First copy your one-time code: ABCD-1234' >&2; /bin/sleep 0.3; exit 0;;\n  \
             token) echo {TOKEN};;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    gh
}

#[test]
fn a_sign_in_shows_its_code_and_waits_for_githubs_yes() {
    let gh = fake_gh("waits");
    let s = SignIns::default();
    let started = s.start(&gh, "sgn_1", Duration::from_secs(5)).unwrap();
    assert_eq!(started["code"], "ABCD-1234");
    assert_eq!(started["waits"], true);
    assert_eq!(started["url"], "https://github.com/login/device");
    let prompt = started["prompt"].as_str().unwrap();
    assert!(prompt.contains("ABCD-1234") && !prompt.contains("paste"), "{prompt}");
    let got = s.wait(&gh, "sgn_1", Duration::from_secs(5)).unwrap();
    assert_eq!(got["value"].as_str(), Some(TOKEN), "the token alone, as text, as the node takes it");
    // once it's out, the sign-in is gone
    assert!(s.wait(&gh, "sgn_1", Duration::from_secs(1)).unwrap_err().to_string().contains("has ended"));
}

#[test]
fn an_older_node_can_still_paste_the_same_code_back() {
    let gh = fake_gh("paste");
    let s = SignIns::default();
    s.start(&gh, "sgn_2", Duration::from_secs(5)).unwrap();
    let wrong = s.code(&gh, "sgn_2", "WXYZ-0000", Duration::from_secs(5)).unwrap_err().to_string();
    assert!(wrong.contains("ABCD-1234"), "{wrong}");
    let got = s.code(&gh, "sgn_2", "abcd-1234", Duration::from_secs(5)).unwrap();
    assert_eq!(got["value"].as_str(), Some(TOKEN), "the token alone, as text, as the node takes it");
}
