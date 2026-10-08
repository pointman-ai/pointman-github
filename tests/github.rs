//! The extension against a stand-in node (pointman-extension's testing::StandIn), started as a node starts
//! it, with a stand-in `gh` that answers as GitHub's API would.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;

use pointman_extension::testing::StandIn;
use serde_json::{json, Value};

/// A `gh` that answers `gh api <path>` from a few canned bodies, and 404 for anything else. Written once:
/// one test writing it while another runs it is "text file busy" on Linux.
fn fake_gh() -> PathBuf {
    static GH: OnceLock<PathBuf> = OnceLock::new();
    GH.get_or_init(write_gh).clone()
}

fn write_gh() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pointman-github-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pull = json!({"number": 42, "title": "Fix the thing", "state": "open", "draft": false, "merged": true,
                      "head": {"sha": "c8b3559aaaa"}, "user": {"login": "patchbay"}, "body": "It fixes it.",
                      "created_at": "2026-10-08T00:00:00Z", "html_url": "https://github.com/a/b/pull/42"});
    let said = json!([{"user": {"login": "james"}, "body": "Looks good", "created_at": "2026-10-08T01:00:00Z",
                       "html_url": "https://github.com/a/b/pull/42#issuecomment-1"}]);
    let reviews = json!([{"user": {"login": "loupe"}, "state": "APPROVED", "body": "", "submitted_at": "2026-10-08T02:00:00Z",
                          "html_url": "https://github.com/a/b/pull/42#pullrequestreview-2"}]);
    let on_lines = json!([{"user": {"login": "loupe"}, "body": "Why this?", "created_at": "2026-10-08T01:30:00Z",
                           "path": "src/my file.py", "line": null, "original_line": 3, "in_reply_to_id": 5,
                           "pull_request_review_id": 2,
                           "html_url": "https://github.com/a/b/pull/42#discussion_r3"}]);
    let issue = json!({"number": 9, "title": "It broke", "state": "open", "user": {"login": "james"}, "body": "Steps",
                       "created_at": "2026-10-07T00:00:00Z", "html_url": "https://github.com/a/b/issues/9"});
    let files = json!([
        {"filename": "src/my file.py", "status": "modified", "additions": 2, "deletions": 1, "patch": "@@ -1 +1,2 @@\n-a\n+b\n+c"},
        {"filename": "img.png", "previous_filename": "old.png", "status": "renamed", "additions": 0, "deletions": 0}
    ]);
    let commit = json!({"sha": "6946239ffff", "commit": {"message": "Revert it\n\nWhy, at length"},
                        "files": [{"filename": "a.rs", "status": "added", "additions": 1, "deletions": 0, "patch": "+x"}]});
    let prs = json!([{"number": 42, "title": "Fix the thing", "state": "closed", "merged_at": "2026-10-08T00:00:00Z"}]);
    let script = format!(
        r#"#!/bin/sh
[ "$1" = api ] || exit 2
case "$2" in
  repos/a/b/pulls/42) printf '%s\n' '{pull}' ;;
  'repos/a/b/pulls/42/files?per_page=100&page=1') printf '%s\n' '{files}' ;;
  'repos/a/b/issues/42/comments?per_page=100&page=1') printf '%s\n' '{said}' ;;
  'repos/a/b/pulls/42/reviews?per_page=100&page=1') printf '%s\n' '{reviews}' ;;
  'repos/a/b/pulls/42/comments?per_page=100&page=1') printf '%s\n' '{on_lines}' ;;
  repos/a/b/issues/9) printf '%s\n' '{issue}' ;;
  'repos/a/b/issues/9/comments?per_page=100&page=1') printf '%s\n' '[]' ;;
  repos/a/b/commits/6946239) printf '%s\n' '{commit}' ;;
  'repos/a/b/contents/src/my%20file.py?ref=c8b3559')
    [ "$4" = "Accept: application/vnd.github.raw" ] || exit 3
    printf 'line one\nline two\n' ;;
  repos/a/b/commits/c8b3559/pulls) printf '%s\n' '{prs}' ;;
  *) echo "gh: Not Found (HTTP 404)" >&2; exit 1 ;;
esac
"#
    );
    let gh = dir.join("gh");
    std::fs::write(&gh, script).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    gh
}

fn view(url: &str) -> Result<Value, String> {
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_github"))
        .settings(json!({"cli": fake_gh().to_string_lossy()}))
        .start()
        .unwrap();
    node.run("github.view", json!({"url": url})).map(|d| d.result).map_err(|f| f.message)
}

#[test]
fn a_pull_request_with_its_files() {
    let v = view("https://github.com/a/b/pull/42/files").unwrap();
    assert_eq!(
        (v["kind"].as_str(), v["title"].as_str(), v["ref"].as_str()),
        (Some("pull"), Some("Fix the thing"), Some("c8b3559aaaa"))
    );
    assert_eq!(v["pull"], json!({"number": 42, "title": "Fix the thing", "state": "merged"}));
    assert_eq!(
        v["files"][0],
        json!({"path": "src/my file.py", "previous_path": null, "status": "modified",
                                     "additions": 2, "deletions": 1, "patch": "@@ -1 +1,2 @@\n-a\n+b\n+c"})
    );
    assert_eq!((v["files"][1]["previous_path"].as_str(), &v["files"][1]["patch"]), (Some("old.png"), &Value::Null));
    assert_eq!((&v["file"], &v["lines"]), (&Value::Null, &Value::Null));
    // its conversation (James, Pointman thread 649 [4500]): description, comments, reviews, comments on lines
    let t = &v["thread"];
    assert_eq!((t["author"].as_str(), t["body"].as_str()), (Some("patchbay"), Some("It fixes it.")));
    assert_eq!(
        t["comments"][0],
        json!({"author": "james", "body": "Looks good", "created_at": "2026-10-08T01:00:00Z",
                                        "url": "https://github.com/a/b/pull/42#issuecomment-1"})
    );
    assert_eq!(
        (t["reviews"][0]["author"].as_str(), t["reviews"][0]["state"].as_str()),
        (Some("loupe"), Some("APPROVED"))
    );
    let c = &t["review_comments"][0];
    assert_eq!(
        (c["path"].as_str(), c["line"].as_u64(), c["in_reply_to"].as_u64(), c["review_id"].as_u64()),
        (Some("src/my file.py"), Some(3), Some(5), Some(2))
    );
}

#[test]
fn an_issue_and_its_conversation() {
    let v = view("https://github.com/a/b/issues/9").unwrap();
    assert_eq!((v["kind"].as_str(), v["title"].as_str()), (Some("issue"), Some("It broke")));
    assert_eq!(v["issue"], json!({"number": 9, "title": "It broke", "state": "open"}));
    assert_eq!((v["thread"]["body"].as_str(), v["thread"]["comments"].clone()), (Some("Steps"), json!([])));
    assert_eq!(
        (v["thread"]["reviews"].clone(), v["files"].clone(), v["pull"].clone()),
        (json!([]), json!([]), Value::Null)
    );
}

#[test]
fn a_commit() {
    let v = view("https://github.com/a/b/commit/6946239").unwrap();
    assert_eq!((v["kind"].as_str(), v["title"].as_str()), (Some("commit"), Some("Revert it")));
    assert_eq!(v["files"][0]["path"], "a.rs");
}

#[test]
fn a_file_at_its_lines_with_its_change_in_the_pr() {
    let v = view("https://github.com/a/b/blob/c8b3559/src/my%20file.py#L2-L1").unwrap();
    assert_eq!(v["kind"], "file");
    assert_eq!(
        v["file"],
        json!({"path": "src/my file.py", "text": "line one\nline two\n", "language": "python",
                                 "truncated": false})
    );
    assert_eq!((v["lines"].clone(), v["title"].as_str()), (json!([1, 2]), Some("my file.py")));
    assert_eq!(v["pull"]["state"], "merged");
    assert_eq!(v["files"].as_array().unwrap().len(), 1); // only that file's change
}

#[test]
fn what_github_doesnt_show_fails_saying_so() {
    let failed = view("https://github.com/a/b/pull/7").unwrap_err();
    assert_eq!(failed, "GitHub doesn't show that PR to this machine's gh login");
    assert_eq!(
        view("https://github.com/a/b/issues/1").unwrap_err(),
        "GitHub doesn't show that issue to this machine's gh login"
    );
    assert!(view("https://github.com/a/b/discussions/1").unwrap_err().contains("isn't a link"));
}

/// With this machine's own `gh`: `cargo test -- --ignored` (signed in, online).
#[test]
#[ignore]
fn the_real_github() {
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR")).binary(env!("CARGO_BIN_EXE_github")).start().unwrap();
    let run = |url: &str| node.run("github.view", json!({"url": url})).map(|d| d.result).map_err(|f| f.message);
    let v = run("https://github.com/pointman-ai/pointman/commit/6946239").unwrap();
    assert_eq!(v["files"].as_array().unwrap().len(), 3, "{v}");
    let v = run("https://github.com/pointman-ai/pointman/blob/6946239/src/messageboard/server/v1/people.py#L345-L350")
        .unwrap();
    assert!(v["file"]["text"].as_str().unwrap().contains("def node_extensions"), "{v}");
    assert_eq!(v["lines"], json!([345, 350]));
    let v = run("https://github.com/pointman-ai/pointman/pull/8").unwrap();
    assert_eq!((v["kind"].as_str(), v["pull"]["number"].as_u64()), (Some("pull"), Some(8)), "{v}");
    assert!(v["thread"]["author"].is_string() && v["thread"]["comments"].is_array(), "{v}");
}
