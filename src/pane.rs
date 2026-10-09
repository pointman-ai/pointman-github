//! The `view` pane (Pointman's docs/dev/panes.md, MB-343): what a GitHub link shows, as blocks the apps
//! draw for every extension alike, in place of the Mac's own GitHub viewer (MB-300). A PR is its
//! conversation and its files in tabs, a commit its files, a file at a commit its code around the lines
//! with how it changed in its PR, and an issue its conversation. An open PR also says where it stands
//! (checks, review, whether it can merge, and as whom), with buttons to approve and merge and a form to
//! review it: `actions`, `tap` and `submit` in main.rs.

use serde_json::{json, Value};

/// What a pane's content may come to: the stream's 1 MiB a frame, less room for the frame around it.
pub const CONTENT_MAX: usize = 1000 * 1024;

/// The pane's title: `PR 42 · Fix the keeper`.
pub fn title(view: &Value) -> String {
    let title = view["title"].as_str().unwrap_or_default();
    let what = match view["kind"].as_str() {
        Some("pull") => format!("PR {}", view["pull"]["number"]),
        Some("issue") => format!("Issue {}", view["issue"]["number"]),
        Some("commit") => short(&view["ref"]),
        _ => return format!("{title} @ {}", short(&view["ref"])),
    };
    format!("{what} · {title}")
}

/// `github.view`'s answer as blocks, cut to fit a frame.
pub fn blocks(view: &Value) -> Value {
    let files = view["files"].as_array().map(Vec::as_slice).unwrap_or_default();
    let mut out = vec![json!({"type": "heading", "id": "head", "level": 1,
                              "text": view["title"].as_str().unwrap_or_default(), "subtitle": subtitle(view)})];
    match view["kind"].as_str() {
        Some("pull") => {
            out.extend(actions(view));
            out.push(json!({"type": "tabs", "id": "tabs", "selected": "conversation", "tabs": [
            {"id": "conversation", "title": "Conversation", "children": [timeline(&view["thread"])]},
            {"id": "files", "title": format!("Files {}", files.len()), "children": [diff(files, Value::Null)]}]}));
        }
        Some("issue") => out.push(timeline(&view["thread"])),
        Some("commit") => out.push(diff(files, Value::Null)),
        _ => {
            let file = &view["file"];
            match file["text"].as_str() {
                Some(text) => out
                    .push(json!({"type": "code", "id": "code", "text": text, "language": file["language"],
                                              "path": file["path"], "lines": view["lines"], "start": 1})),
                None => out.push(json!({"type": "empty", "id": "code", "title": "A binary file",
                                        "note": "GitHub's page shows it."})),
            }
            if file["truncated"] == true {
                out.push(json!({"type": "text", "id": "cut", "markdown": "_Only its first 512 KB is shown._"}));
            }
            if !files.is_empty() {
                let pr = &view["pull"];
                out.push(json!({"type": "heading", "id": "changed", "level": 2,
                                "text": format!("How it changed in PR {}", pr["number"]),
                                "subtitle": format!("{} · {}", pr["title"].as_str().unwrap_or_default(),
                                                    pr["state"].as_str().unwrap_or_default())}));
                out.push(diff(files, Value::Null));
            }
        }
    }
    fit(Value::Array(out), CONTENT_MAX)
}

/// An open PR's standing and what this login can do with it: a line saying its checks, review, whether it
/// can merge and as whom; Approve and a button per merge method the repo allows (merging asks first); and
/// a review with words. Nothing for a closed or merged PR.
pub fn actions(view: &Value) -> Vec<Value> {
    let m = &view["merge"];
    if !m.is_object() {
        return vec![];
    }
    if let Some(e) = m["error"].as_str() {
        return vec![
            json!({"type": "text", "id": "pr-status", "markdown": format!("_GitHub didn't say where this PR stands: {e}_")}),
        ];
    }
    let number = &view["pull"]["number"];
    let base = m["base"].as_str().unwrap_or("its base");
    let c = &m["checks"];
    let count = |k: &str| c[k].as_u64().unwrap_or(0);
    let checks = match (count("passed"), count("failed"), count("pending")) {
        (0, 0, 0) => "none".to_string(),
        (p, f, w) => [(p, "passed"), (f, "failing"), (w, "running")]
            .iter()
            .filter(|(n, _)| *n > 0)
            .map(|(n, what)| format!("{n} {what}"))
            .collect::<Vec<_>>()
            .join(", "),
    };
    let review = match m["review"].as_str() {
        Some("APPROVED") => "approved",
        Some("CHANGES_REQUESTED") => "changes requested",
        Some("REVIEW_REQUIRED") => "needed",
        _ => "none needed",
    };
    let state = m["state"].as_str().unwrap_or("UNKNOWN");
    let can = match state {
        "CLEAN" | "HAS_HOOKS" => format!("ready to merge into `{base}`"),
        "UNSTABLE" => format!("can merge into `{base}`, though not every check passed"),
        "BLOCKED" => format!("blocked from `{base}` by a required review or check"),
        "BEHIND" => format!("behind `{base}`: update the branch first"),
        "DIRTY" => format!("conflicts with `{base}`: resolve them on GitHub"),
        "DRAFT" => "a draft".to_string(),
        _ => "GitHub is still working out whether it can merge".to_string(),
    };
    let viewer = m["viewer"].as_str().unwrap_or("this machine's login");
    let mut out = vec![json!({"type": "text", "id": "pr-status", "markdown":
        format!("**Checks** {checks} · **Review** {review} · {can}. Acting as **{viewer}**.")})];

    let authored = m["authored"] == true;
    let mut buttons = vec![];
    if !authored && m["my_review"] != "APPROVED" {
        buttons.push(json!({"id": "approve", "title": "Approve", "tone": "good"}));
    }
    // a merge for a login that may (GitHub has the last word on what it may bypass), unless it conflicts,
    // which nothing merges; a forced one overrides what stands in its way, and says what, first
    if m["can_merge"] == true && state != "DIRTY" {
        let draft = m["draft"] == true;
        let mut against = vec![];
        if draft {
            against.push("it's a draft (it's marked ready first)".to_string());
        }
        match state {
            "BLOCKED" => against.push("a required review or check is missing".to_string()),
            "BEHIND" => against.push(format!("it's behind {base}")),
            _ => {}
        }
        if count("failed") > 0 {
            against.push(format!("{} check{} failed", count("failed"), if count("failed") == 1 { "" } else { "s" }));
        }
        let forced = !against.is_empty();
        for method in m["methods"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let (title, forced_title, how) = match method {
                "squash" => ("Squash and merge", "Force squash", "squashed into one commit"),
                "rebase" => ("Rebase and merge", "Force rebase", "rebased"),
                _ => ("Merge", "Force merge", "with a merge commit"),
            };
            let button = if forced {
                json!({"id": format!("merge:{method}"), "title": forced_title, "tone": "bad",
                       "confirm": format!("Force-merge PR #{number} into {base}, {how}, as {viewer}, though {}?",
                                          against.join(" and "))})
            } else {
                json!({"id": format!("merge:{method}"), "title": title,
                       "tone": if matches!(state, "CLEAN" | "HAS_HOOKS") { "good" } else { "warn" },
                       "confirm": format!("Merge PR #{number} into {base}, {how}, as {viewer}?")})
            };
            buttons.push(button);
        }
    }
    if !buttons.is_empty() {
        out.push(json!({"type": "actions", "id": "pr-actions", "buttons": buttons}));
    }
    // GitHub takes no approval or change request from a PR's own author
    let mut kinds = vec![json!({"value": "COMMENT", "title": "Comment"})];
    if !authored {
        kinds.push(json!({"value": "APPROVE", "title": "Approve"}));
        kinds.push(json!({"value": "REQUEST_CHANGES", "title": "Request changes"}));
    }
    out.push(json!({"type": "form", "id": "pr-review", "submit": "Submit review", "fields": [
        {"id": "event", "kind": "select", "label": "Review", "value": "COMMENT", "choices": kinds},
        {"id": "body", "kind": "text", "label": "Comment", "value": ""}]}));
    out
}

fn short(sha: &Value) -> String {
    sha.as_str().unwrap_or_default().chars().take(7).collect()
}

fn subtitle(view: &Value) -> String {
    let repo = view["repo"].as_str().unwrap_or_default();
    let files = view["files"].as_array().map(Vec::as_slice).unwrap_or_default();
    let count = |key: &str| files.iter().filter_map(|f| f[key].as_u64()).sum::<u64>();
    let changed = format!(
        "{} file{}, +{} −{}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        count("additions"),
        count("deletions")
    );
    let state = |v: &Value| v["state"].as_str().unwrap_or_default().to_string();
    match view["kind"].as_str() {
        Some("pull") => format!("{repo} · PR #{} · {} · {changed}", view["pull"]["number"], state(&view["pull"])),
        Some("issue") => format!("{repo} · issue #{} · {}", view["issue"]["number"], state(&view["issue"])),
        Some("commit") => format!("{repo} · commit {} · {changed}", short(&view["ref"])),
        _ => {
            let path = view["file"]["path"].as_str().unwrap_or_default();
            match view["pull"]["number"].as_u64() {
                Some(n) => format!("{repo} · {path} @ {} · in PR #{n}", short(&view["ref"])),
                None => format!("{repo} · {path} @ {}", short(&view["ref"])),
            }
        }
    }
}

fn diff(files: &[Value], lines: Value) -> Value {
    if files.is_empty() {
        return json!({"type": "empty", "id": "diff", "title": "No files changed", "note": ""});
    }
    json!({"type": "diff", "id": "diff", "files": files, "lines": lines})
}

/// A PR's or an issue's conversation, oldest first: its description, comments, reviews and comments on
/// lines. Each event's id is GitHub's own anchor (`issuecomment-1`), so it stays put as the thread grows.
fn timeline(thread: &Value) -> Value {
    let text = |v: &Value| v.as_str().unwrap_or_default().trim().to_string();
    let anchor = |v: &Value, otherwise: String| {
        v["url"].as_str().and_then(|u| u.split_once('#')).map(|(_, a)| a.to_string()).unwrap_or(otherwise)
    };
    let mut events = Vec::new();
    if thread.is_object() {
        let body = text(&thread["body"]);
        events.push(json!({"id": "opened", "at": thread["created_at"], "who": thread["author"], "kind": "comment",
                           "markdown": if body.is_empty() { "_No description._".to_string() } else { body }}));
    }
    let list = |key: &str| thread[key].as_array().cloned().unwrap_or_default();
    for (i, c) in list("comments").iter().enumerate() {
        events.push(json!({"id": anchor(c, format!("comment-{i}")), "at": c["created_at"], "who": c["author"],
                           "kind": "comment", "markdown": text(&c["body"])}));
    }
    for (i, r) in list("reviews").iter().enumerate() {
        let said = match r["state"].as_str().unwrap_or_default() {
            "APPROVED" => "**Approved**",
            "CHANGES_REQUESTED" => "**Changes requested**",
            "DISMISSED" => "**Dismissed**",
            "PENDING" => continue,
            _ => "**Reviewed**",
        };
        let body = text(&r["body"]);
        let markdown = if body.is_empty() { said.to_string() } else { format!("{said}\n\n{body}") };
        events.push(json!({"id": anchor(r, format!("review-{i}")), "at": r["submitted_at"], "who": r["author"],
                           "kind": "review", "markdown": markdown}));
    }
    for (i, c) in list("review_comments").iter().enumerate() {
        let place = match c["line"].as_u64() {
            Some(line) => format!("`{}` line {line}", c["path"].as_str().unwrap_or_default()),
            None => format!("`{}`", c["path"].as_str().unwrap_or_default()),
        };
        events.push(json!({"id": anchor(c, format!("line-{i}")), "at": c["created_at"], "who": c["author"],
                           "kind": "line", "markdown": format!("{place}\n\n{}", text(&c["body"]))}));
    }
    events.sort_by(|a, b| a["at"].as_str().unwrap_or_default().cmp(b["at"].as_str().unwrap_or_default()));
    json!({"type": "timeline", "id": "timeline", "events": events})
}

/// Blocks cut to `max` bytes: the biggest files' diffs go first, and a line says how many GitHub's page
/// has instead.
pub fn fit(mut blocks: Value, max: usize) -> Value {
    let mut size = serde_json::to_vec(&blocks).map(|b| b.len()).unwrap_or(0);
    if size <= max {
        return blocks;
    }
    let mut patches = Vec::new();
    find_patches(&blocks, String::new(), &mut patches);
    patches.sort_by_key(|p| std::cmp::Reverse(p.1));
    let mut dropped = 0;
    for (at, len) in patches {
        if size <= max {
            break;
        }
        if let Some(patch) = blocks.pointer_mut(&at) {
            *patch = Value::Null;
            size = size.saturating_sub(len.saturating_sub(4));
            dropped += 1;
        }
    }
    if dropped > 0 {
        let s = if dropped == 1 { " is" } else { "s are" };
        if let Some(list) = blocks.as_array_mut() {
            list.push(json!({"type": "text", "id": "too-big",
                             "markdown": format!("_{dropped} file{s} too big to show here: GitHub's page has them._")}));
        }
    }
    blocks
}

/// Where each diff's patches are (a JSON Pointer from the top) and how long each is.
fn find_patches(v: &Value, at: String, out: &mut Vec<(String, usize)>) {
    match v {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                find_patches(item, format!("{at}/{i}"), out);
            }
        }
        Value::Object(block) if block.get("type") == Some(&json!("diff")) => {
            for (i, file) in block["files"].as_array().into_iter().flatten().enumerate() {
                if let Some(patch) = file["patch"].as_str() {
                    out.push((format!("{at}/files/{i}/patch"), serde_json::to_string(patch).map_or(0, |s| s.len())));
                }
            }
        }
        Value::Object(block) => {
            if let Some(children) = block.get("children") {
                find_patches(children, format!("{at}/children"), out);
            }
            if let Some(tabs) = block.get("tabs").and_then(Value::as_array) {
                for (t, tab) in tabs.iter().enumerate() {
                    find_patches(&tab["children"], format!("{at}/tabs/{t}/children"), out);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pull() -> Value {
        json!({"kind": "pull", "repo": "a/b", "url": "https://github.com/a/b/pull/42", "title": "Fix the thing",
               "ref": "c8b3559aaaa", "lines": null, "pull": {"number": 42, "title": "Fix the thing", "state": "open"},
               "issue": null, "file": null,
               "files": [{"path": "src/a.py", "previous_path": null, "status": "modified", "additions": 2,
                          "deletions": 1, "patch": "@@ -1 +1,2 @@\n-a\n+b\n+c"}],
               "thread": {"author": "patchbay", "body": "It fixes it.", "created_at": "2026-10-08T00:00:00Z",
                          "url": "https://github.com/a/b/pull/42",
                          "comments": [{"author": "james", "body": "Looks good", "created_at": "2026-10-08T01:00:00Z",
                                        "url": "https://github.com/a/b/pull/42#issuecomment-1"}],
                          "reviews": [{"author": "loupe", "state": "APPROVED", "body": "",
                                       "submitted_at": "2026-10-08T02:00:00Z",
                                       "url": "https://github.com/a/b/pull/42#pullrequestreview-2"}],
                          "review_comments": [{"author": "loupe", "body": "Why this?", "created_at": "2026-10-08T01:30:00Z",
                                               "path": "src/a.py", "line": 3,
                                               "url": "https://github.com/a/b/pull/42#discussion_r3"}]}})
    }

    #[test]
    fn a_pr_is_its_conversation_and_its_files() {
        let v = pull();
        assert_eq!(title(&v), "PR 42 · Fix the thing");
        let b = blocks(&v);
        assert_eq!(b[0]["subtitle"], "a/b · PR #42 · open · 1 file, +2 −1");
        let tabs = &b[1]["tabs"];
        let events = tabs[0]["children"][0]["events"].as_array().unwrap();
        let ids: Vec<&str> = events.iter().map(|e| e["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["opened", "issuecomment-1", "discussion_r3", "pullrequestreview-2"]);
        assert_eq!(events[2]["markdown"], "`src/a.py` line 3\n\nWhy this?");
        assert_eq!(events[3]["markdown"], "**Approved**");
        assert_eq!(tabs[1]["title"], "Files 1");
        assert_eq!(tabs[1]["children"][0]["files"], v["files"]);
    }

    #[test]
    fn a_file_is_its_code_and_how_it_changed() {
        let v = json!({"kind": "file", "repo": "a/b", "title": "a.py", "ref": "c8b3559aaaa", "lines": [3, 5],
                       "file": {"path": "src/a.py", "text": "x\n", "language": "python", "truncated": true},
                       "pull": {"number": 42, "title": "Fix", "state": "merged"},
                       "files": [{"path": "src/a.py", "status": "modified", "additions": 1, "deletions": 0, "patch": "+x"}]});
        assert_eq!(title(&v), "a.py @ c8b3559");
        let b = blocks(&v);
        let ids: Vec<&str> = b.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["head", "code", "cut", "changed", "diff"]);
        assert_eq!(
            b[1],
            json!({"type": "code", "id": "code", "text": "x\n", "language": "python",
                                "path": "src/a.py", "lines": [3, 5], "start": 1})
        );
        assert_eq!(b[0]["subtitle"], "a/b · src/a.py @ c8b3559 · in PR #42");
        let binary = blocks(&json!({"kind": "file", "file": {"text": null}, "files": []}));
        assert_eq!(binary[1]["type"], "empty");
    }

    #[test]
    fn an_issue_and_a_commit() {
        let issue = json!({"kind": "issue", "repo": "a/b", "title": "It broke", "issue": {"number": 9, "state": "open"},
                           "files": [], "thread": {"author": "james", "body": "", "created_at": "2026-10-07T00:00:00Z",
                                                   "comments": [], "reviews": [], "review_comments": []}});
        assert_eq!(title(&issue), "Issue 9 · It broke");
        let b = blocks(&issue);
        assert_eq!(b[1]["events"][0]["markdown"], "_No description._");
        let commit = json!({"kind": "commit", "repo": "a/b", "title": "Revert it", "ref": "6946239ffff", "files": []});
        assert_eq!(title(&commit), "6946239 · Revert it");
        assert_eq!(blocks(&commit)[1]["type"], "empty");
    }

    #[test]
    fn big_diffs_give_way_biggest_first() {
        let file = |name: &str, n: usize| json!({"path": name, "status": "modified", "patch": "x".repeat(n)});
        let b = json!([{"type": "tabs", "id": "tabs", "tabs": [{"id": "files", "children": [
            {"type": "diff", "id": "diff", "files": [file("small", 100), file("big", 5000), file("mid", 3000)]}]}]}]);
        let cut = fit(b.clone(), 4000);
        let files = &cut[0]["tabs"][0]["children"][0]["files"];
        assert!(files[1]["patch"].is_null() && files[0]["patch"].is_string() && files[2]["patch"].is_string());
        assert_eq!(cut[1]["markdown"], "_1 file is too big to show here: GitHub's page has them._");
        assert_eq!(fit(b.clone(), 1_000_000), b);
    }

    #[test]
    fn an_open_prs_buttons_follow_where_it_stands() {
        let mut v = pull();
        let merge = |state: &str| {
            json!({"viewer": "james", "methods": ["squash"], "can_merge": true, "mergeable": "MERGEABLE", "state": state,
                   "review": "APPROVED", "draft": false, "authored": false, "my_review": "APPROVED", "base": "production",
                   "checks": {"state": "FAILURE", "passed": 3, "failed": 1, "pending": 0}})
        };
        v["merge"] = merge("UNSTABLE");
        let b = blocks(&v);
        let ids: Vec<&str> = b.as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["head", "pr-status", "pr-actions", "pr-review", "tabs"]);
        assert_eq!(
            b[1]["markdown"],
            "**Checks** 3 passed, 1 failing · **Review** approved · can merge into `production`, though not every \
             check passed. Acting as **james**."
        );
        // already approved by this login: only the merge, forced past the failing check, which it names
        assert_eq!(
            b[2]["buttons"],
            json!([{"id": "merge:squash", "title": "Force squash", "tone": "bad",
                    "confirm": "Force-merge PR #42 into production, squashed into one commit, as james, though 1 check failed?"}])
        );
        // a draft that's blocked: forced too, saying both
        v["merge"] = merge("BLOCKED");
        v["merge"]["draft"] = json!(true);
        v["merge"]["checks"]["failed"] = json!(0);
        assert_eq!(
            actions(&v)[1]["buttons"][0]["confirm"],
            "Force-merge PR #42 into production, squashed into one commit, as james, though it's a draft (it's marked \
             ready first) and a required review or check is missing?"
        );
        // ready, with nothing in its way: a plain merge
        v["merge"] = merge("CLEAN");
        v["merge"]["checks"]["failed"] = json!(0);
        assert_eq!(
            actions(&v)[1]["buttons"][0],
            json!({"id": "merge:squash", "title": "Squash and merge", "tone": "good",
                   "confirm": "Merge PR #42 into production, squashed into one commit, as james?"})
        );
        // conflicts, or no right to merge: no merge buttons
        for (field, value) in [("state", json!("DIRTY")), ("can_merge", json!(false))] {
            v["merge"] = merge("CLEAN");
            v["merge"][field] = value;
            assert!(actions(&v).iter().all(|x| x["id"] != "pr-actions"), "{field}");
        }
        v["merge"] = merge("DIRTY");
        assert!(actions(&v)[0]["markdown"]
            .as_str()
            .unwrap()
            .contains("conflicts with `production`: resolve them on GitHub"));
        v["merge"] = json!({"error": "Resource not accessible by integration"});
        assert_eq!(
            actions(&v),
            [json!({"type": "text", "id": "pr-status",
                    "markdown": "_GitHub didn't say where this PR stands: Resource not accessible by integration_"})]
        );
    }

    /// What the SDK's show sends when the thread grows: the new comment and the subtitle, in place.
    #[test]
    fn a_grown_thread_patches_in_place() {
        use pointman_extension::changes;
        let old = blocks(&pull());
        let mut v = pull();
        v["thread"]["comments"].as_array_mut().unwrap().push(json!({"author": "cairn", "body": "Merging",
            "created_at": "2026-10-08T03:00:00Z", "url": "https://github.com/a/b/pull/42#issuecomment-9"}));
        v["pull"]["state"] = json!("merged");
        let ops = changes(&old, &blocks(&v)).unwrap();
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert_eq!(ops[0]["path"], "/b/head/subtitle");
        assert_eq!(ops[1]["path"], "/b/timeline/events/-");
        assert_eq!(ops[1]["value"]["id"], "issuecomment-9");
        assert_eq!(changes(&old, &old), Some(vec![]));
        // a file more: the tab's title and the diff's files change, in place
        v["files"].as_array_mut().unwrap().push(json!({"path": "b.rs", "status": "added", "patch": "+y"}));
        let ops = changes(&old, &blocks(&v)).unwrap();
        let paths: Vec<&str> = ops.iter().map(|o| o["path"].as_str().unwrap()).collect();
        assert!(paths.contains(&"/b/tabs/tabs/1/title") && paths.contains(&"/b/diff/files/-"), "{paths:?}");
        // a commit has no tabs: blocks of another shape are sent whole
        let commit = blocks(&json!({"kind": "commit", "files": []}));
        assert_eq!(changes(&old, &commit), None);
    }
}
