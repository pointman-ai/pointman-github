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
    // PRs 43–47, open, for the pane's buttons and review form: 44 is the login's own, 45 merges (and is
    // merged after), 46 can't merge, 47 is a draft; what's sent is logged in calls.log beside gh
    let merge = |n: u64, authored: bool, draft: bool| {
        json!({"data": {"viewer": {"login": "james"}, "repository": {"mergeCommitAllowed": true,
               "squashMergeAllowed": false, "rebaseMergeAllowed": true, "viewerPermission": "ADMIN",
               "pullRequest": {"id": format!("PR_{n}"), "mergeable": "MERGEABLE",
                               "mergeStateStatus": if draft { "DRAFT" } else { "CLEAN" }, "reviewDecision": "REVIEW_REQUIRED",
                               "isDraft": draft, "viewerDidAuthor": authored, "baseRefName": "main", "viewerLatestReview": null,
                               "commits": {"nodes": [{"commit": {"oid": format!("d{n}"), "statusCheckRollup": {"state": "SUCCESS",
                                   "contexts": {"nodes": [{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
                                                          {"__typename": "StatusContext", "state": "PENDING"}]}}}}]}}}}})
    };
    let open_prs = format!(
        r#"  graphql)
    case "$*" in
      *markPullRequestReadyForReview*)
        printf 'READY %s\n' "$(printf '%s\n' "$@" | sed -n 's/^id=//p')" >> "$here/calls.log"
        printf '%s\n' '{{"data": {{"markPullRequestReadyForReview": {{"pullRequest": {{"isDraft": false}}}}}}}}'
        exit 0 ;;
    esac
    n=$(printf '%s\n' "$@" | sed -n 's/^number=//p')
    case "$n" in
      44) printf '%s\n' '{own}' ;;
      47) printf '%s\n' '{draft}' ;;
      *) printf '%s\n' '{other}' | sed "s/\"d43\"/\"d$n\"/; s/\"PR_43\"/\"PR_$n\"/" ;;
    esac ;;
  repos/a/b/pulls/43|repos/a/b/pulls/44|repos/a/b/pulls/45|repos/a/b/pulls/46|repos/a/b/pulls/47)
    n=${{2##*/}}; merged=false; state=open
    [ -f "$here/merged-$n" ] && merged=true && state=closed
    printf '{{"number": %s, "title": "Ship %s", "state": "%s", "draft": false, "merged": %s, "head": {{"sha": "d%s"}}, "user": {{"login": "patchbay"}}, "body": "", "created_at": "2026-10-09T00:00:00Z", "html_url": "https://github.com/a/b/pull/%s"}}\n' "$n" "$n" "$state" "$merged" "$n" "$n" ;;
  repos/a/b/pulls/4[3-7]/files\?*|repos/a/b/issues/4[3-7]/comments\?*|repos/a/b/pulls/4[3-7]/reviews\?*|repos/a/b/pulls/4[3-7]/comments\?*)
    printf '[]\n' ;;
  repos/a/b/pulls/4[3-7]/reviews|repos/a/b/pulls/4[3-7]/merge)
    body=$(cat); printf '%s %s %s\n' "$4" "$2" "$body" >> "$here/calls.log"
    case "$2:$body" in
      */44/reviews:*APPROVE*)
        printf '%s\n' '{{"message":"Unprocessable Entity","errors":["Review Can not approve your own pull request"]}}'
        echo "gh: Unprocessable Entity (HTTP 422)" >&2; exit 1 ;;
      */46/merge:*)
        printf '%s\n' '{{"message":"Pull Request is not mergeable"}}'
        echo "gh: Pull Request is not mergeable (HTTP 405)" >&2; exit 1 ;;
      */merge:*) n=${{2#repos/a/b/pulls/}}; touch "$here/merged-${{n%/merge}}"; printf '%s\n' '{{"merged": true}}' ;;
      *) printf '%s\n' '{{"id": 1}}' ;;
    esac ;;
"#,
        own = merge(44, true, false),
        other = merge(43, false, false),
        draft = merge(47, true, true)
    );
    let script = script.replace("case \"$2\" in\n", &format!("here=$(dirname \"$0\")\ncase \"$2\" in\n{open_prs}"));
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
    assert_eq!(got["url"], "https://github.com/login/device");
    let prompt = got["prompt"].as_str().unwrap();
    assert!(regex::Regex::new(r"^Enter [A-Z0-9]{4}-[A-Z0-9]{4} at ").unwrap().is_match(prompt), "{prompt}");
    let wrong = signins.code(&gh, "s1", "AAAA-AAAA", std::time::Duration::from_secs(1)).unwrap_err().to_string();
    assert!(wrong.starts_with("that isn't this sign-in's code"), "{wrong}");
}

fn node() -> StandIn {
    StandIn::new(env!("CARGO_MANIFEST_DIR"))
        .binary(env!("CARGO_BIN_EXE_github"))
        .settings(json!({"cli": fake_gh().to_string_lossy()}))
        .start()
        .unwrap()
}

#[test]
fn a_pr_opens_as_a_pane_of_its_conversation_and_files() {
    let node = node();
    let view = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/42"})).unwrap();
    assert_eq!(view.format().as_deref(), Some("blocks"));
    assert_eq!(view.title().as_deref(), Some("PR 42 · Fix the thing"));
    assert_eq!(view.block("head").unwrap()["subtitle"], "a/b · PR #42 · merged · 2 files, +2 −1");
    let events = view.block("timeline").unwrap()["events"].clone();
    let who: Vec<&str> = events.as_array().unwrap().iter().map(|e| e["who"].as_str().unwrap()).collect();
    assert_eq!(who, ["patchbay", "james", "loupe", "loupe"]);
    let diff = view.block("diff").unwrap();
    assert_eq!(diff["files"][0]["path"], "src/my file.py");
    assert_eq!(diff["files"][1]["previous_path"], "old.png");
    // a tab is the app's to show; anything else has nothing to do here
    view.input(json!({"kind": "select", "block": "tabs", "value": "files"})).unwrap();
    let refused = view.input(json!({"kind": "tap", "block": "diff"})).unwrap_err();
    assert_eq!(refused.message, "the GitHub view has nothing to do for tap on diff");
    // a merged PR says nothing more and has no buttons
    assert!(view.block("pr-status").is_none() && view.block("pr-actions").is_none());
    assert!(view.problems().is_empty(), "{:?}", view.problems());
}

#[test]
fn a_file_and_an_issue_open_as_panes() {
    let node = node();
    let file =
        node.open_pane("view", json!({"url": "https://github.com/a/b/blob/c8b3559/src/my%20file.py#L2"})).unwrap();
    assert_eq!(file.title().as_deref(), Some("my file.py @ c8b3559"));
    let code = file.block("code").unwrap();
    assert_eq!((code["text"].as_str(), code["lines"].clone()), (Some("line one\nline two\n"), json!([2, 2])));
    assert_eq!(file.block("changed").unwrap()["text"], "How it changed in PR 42");
    let issue = node.open_pane("view", json!({"url": "https://github.com/a/b/issues/9"})).unwrap();
    assert_eq!(issue.title().as_deref(), Some("Issue 9 · It broke"));
    assert_eq!(issue.block("timeline").unwrap()["events"][0]["markdown"], "Steps");
    assert!(file.problems().is_empty() && issue.problems().is_empty());
}

#[test]
fn a_pane_that_cant_open_says_why() {
    let node = node();
    let missing = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/7"})).err().unwrap();
    assert_eq!(missing.message, "GitHub doesn't show that PR to this machine's gh login");
    let no_url = node.open_pane("view", json!({"item": "MB-286"})).err().unwrap();
    assert_eq!(no_url.message, "the GitHub view opens a GitHub link: its target has no url");
}

/// What the stand-in gh was sent for a PR: `<method> <path> <body>`, oldest first.
fn sent(pr: u64) -> Vec<String> {
    let log = fake_gh().parent().unwrap().join("calls.log");
    let text = std::fs::read_to_string(log).unwrap_or_default();
    text.lines().filter(|l| l.contains(&format!("/pulls/{pr}/"))).map(String::from).collect()
}

#[test]
fn an_open_pr_is_approved_and_reviewed_from_its_pane() {
    let node = node();
    let view = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/43"})).unwrap();
    assert_eq!(
        view.block("pr-status").unwrap()["markdown"],
        "**Checks** 1 passed, 1 running · **Review** needed · ready to merge into `main`. Acting as **james**."
    );
    let buttons = view.block("pr-actions").unwrap()["buttons"].clone();
    let ids: Vec<&str> = buttons.as_array().unwrap().iter().map(|b| b["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["approve", "merge:merge", "merge:rebase"]); // the repo doesn't allow squash
    assert_eq!(buttons[1]["confirm"], "Merge PR #43 into main, with a merge commit, as james?");
    view.input(json!({"kind": "tap", "block": "pr-actions", "value": "approve"})).unwrap();
    view.input(json!({"kind": "submit", "block": "pr-review",
                      "value": {"event": "REQUEST_CHANGES", "body": "  Rename it  "}}))
        .unwrap();
    assert_eq!(
        sent(43),
        [
            r#"POST repos/a/b/pulls/43/reviews {"commit_id":"d43","event":"APPROVE"}"#,
            r#"POST repos/a/b/pulls/43/reviews {"body":"Rename it","commit_id":"d43","event":"REQUEST_CHANGES"}"#
        ]
    );
    let empty = view.input(json!({"kind": "submit", "block": "pr-review", "value": {"event": "COMMENT", "body": " "}}));
    assert_eq!(empty.unwrap_err().message, "a comment or a change request needs words");
    let odd = view.input(json!({"kind": "tap", "block": "pr-actions", "value": "close"})).unwrap_err();
    assert_eq!(odd.message, "\"close\" isn't one of this PR's buttons");
    assert!(view.problems().is_empty(), "{:?}", view.problems());
}

#[test]
fn what_github_refuses_comes_back_with_its_reason() {
    let node = node();
    // their own PR: no Approve, and the review form only comments
    let own = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/44"})).unwrap();
    let ids: Vec<String> = own.block("pr-actions").unwrap()["buttons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, ["merge:merge", "merge:rebase"]);
    assert_eq!(
        own.block("pr-review").unwrap()["fields"][0]["choices"],
        json!([{"value": "COMMENT", "title": "Comment"}])
    );
    // an older app's tap anyway: GitHub's reason
    let refused = own.input(json!({"kind": "tap", "block": "pr-actions", "value": "approve"})).unwrap_err();
    assert_eq!(refused.message, "Review Can not approve your own pull request"); // GitHub's own words
    let stuck = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/46"})).unwrap();
    let refused = stuck.input(json!({"kind": "tap", "block": "pr-actions", "value": "merge:merge"})).unwrap_err();
    assert_eq!(refused.message, "Pull Request is not mergeable");
}

#[test]
fn a_merge_goes_to_github_and_the_pane_shows_it_merged() {
    let node = node();
    let view = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/45"})).unwrap();
    view.input(json!({"kind": "tap", "block": "pr-actions", "value": "merge:rebase"})).unwrap();
    assert_eq!(sent(45), [r#"PUT repos/a/b/pulls/45/merge {"merge_method":"rebase","sha":"d45"}"#]);
    // GitHub's answer at once: merged, and nothing left to press
    assert_eq!(view.block("head").unwrap()["subtitle"], "a/b · PR #45 · merged · 0 files, +0 −0");
    assert!(view.block("pr-actions").is_none() && view.block("pr-review").is_none());
    let again = view.input(json!({"kind": "tap", "block": "pr-actions", "value": "merge:rebase"})).unwrap_err();
    assert_eq!(again.message, "PR #45 isn't open");
    assert!(view.problems().is_empty(), "{:?}", view.problems());
}

/// The pane's buttons against the real GitHub, with this machine's own `gh`: it reviews and MERGES the PR
/// you name, so name a throwaway one, opened by this login, into a throwaway base. By hand:
/// `GITHUB_PANE_TEST_PR=https://github.com/<owner>/<repo>/pull/<n> cargo test --test github real_pr_buttons -- --ignored`.
#[test]
#[ignore]
fn real_pr_buttons() {
    let url = std::env::var("GITHUB_PANE_TEST_PR").expect("GITHUB_PANE_TEST_PR: a throwaway PR");
    let node = StandIn::new(env!("CARGO_MANIFEST_DIR")).binary(env!("CARGO_BIN_EXE_github")).start().unwrap();
    let view = node.open_pane("view", json!({"url": url})).unwrap();
    let status = view.block("pr-status").unwrap()["markdown"].as_str().unwrap().to_string();
    println!("status: {status}");
    assert!(status.contains("Acting as **"), "{status}");
    let buttons = view.block("pr-actions").unwrap()["buttons"].clone();
    println!("buttons: {buttons}");
    // this login opened it: GitHub takes no approval from its author, and says so
    let own = view.input(json!({"kind": "tap", "block": "pr-actions", "value": "approve"})).unwrap_err();
    println!("approve: {}", own.message);
    assert_eq!(own.message, "Review Can not approve your own pull request");
    view.input(json!({"kind": "submit", "block": "pr-review", "value": {"event": "COMMENT", "body": "Reviewed from Pointman's GitHub pane (live test)."}}))
        .unwrap();
    let said = view.block("timeline").unwrap()["events"].clone();
    assert!(said.to_string().contains("Reviewed from Pointman's GitHub pane"), "{said}");
    view.input(json!({"kind": "tap", "block": "pr-actions", "value": "merge:merge"})).unwrap();
    let head = view.block("head").unwrap()["subtitle"].as_str().unwrap().to_string();
    println!("after merge: {head}");
    assert!(head.contains("· merged ·"), "{head}");
    assert!(view.block("pr-actions").is_none());
    assert!(view.problems().is_empty(), "{:?}", view.problems());
}

#[test]
fn a_draft_is_force_merged_marked_ready_first() {
    let node = node();
    let view = node.open_pane("view", json!({"url": "https://github.com/a/b/pull/47"})).unwrap();
    let buttons = view.block("pr-actions").unwrap()["buttons"].clone();
    assert_eq!(buttons[0]["title"], "Force merge");
    assert_eq!(buttons[0]["tone"], "bad");
    assert_eq!(
        buttons[0]["confirm"],
        "Force-merge PR #47 into main, with a merge commit, as james, though it's a draft (it's marked ready first)?"
    );
    view.input(json!({"kind": "tap", "block": "pr-actions", "value": "merge:merge"})).unwrap();
    let log = std::fs::read_to_string(fake_gh().parent().unwrap().join("calls.log")).unwrap();
    let ready = log.lines().position(|l| l == "READY PR_47").expect("marked ready");
    let merged = log.lines().position(|l| l.starts_with("PUT repos/a/b/pulls/47/merge")).expect("merged");
    assert!(ready < merged, "{log}");
    assert_eq!(view.block("head").unwrap()["subtitle"], "a/b · PR #47 · merged · 0 files, +0 −0");
}
