//! What a GitHub link shows, for Pointman's viewer panel (James, Pointman thread 649 [4424], [4443], [4500]):
//! a PR's or a commit's changed files with GitHub's unified diff of each, a file at a commit around its
//! lines with how that file changed in its commit's PR, and a PR's or an issue's conversation. GitHub is asked with this machine's `gh` login, so the
//! viewer sees what that login sees.
//!
//! The answer, as the apps draw it:
//!
//! ```text
//! {kind: "file"|"pull"|"commit"|"issue", repo, url, title, ref, lines: [from, to] | null,
//!  pull: {number, title, state} | null,
//!  issue: {number, title, state} | null,
//!  file: {path, text, language, truncated} | null,
//!  files: [{path, previous_path, status, additions, deletions, patch | null}],
//!  thread: {author, body, created_at, url,                      // a PR's or an issue's, else null
//!           comments: [{author, body, created_at, url}],
//!           reviews: [{author, state, body, submitted_at, url}],  // a PR's
//!           review_comments: [{author, body, created_at, path, line, url, in_reply_to, review_id}]} | null,
//!  merge: {viewer, methods, can_merge, mergeable, state, review, draft, authored, my_review, base,
//!          checks: {state, passed, failed, pending}} | {error} | null}   // an open PR's, as this login
//! ```
//!
//! An open PR can be reviewed and merged from the viewer (`review`, `merge`), as the login that asks.

pub mod install;
pub mod pane;
pub mod signin;

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use regex::Regex;
use serde_json::{json, Value};

/// A file's text in the viewer; longer is cut, and says so.
pub const TEXT_MAX: usize = 512 * 1024;
/// A PR's or a commit's files (GitHub lists 3,000 at most), and a thread's comments of each kind.
pub const FILES_MAX: usize = 300;
/// How long an answer is kept, as core keeps link pills.
const KEEP: Duration = Duration::from_secs(60);
/// How long one `gh api` call may take.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

const LANGUAGES: &[(&str, &str)] = &[
    ("py", "python"),
    ("rs", "rust"),
    ("swift", "swift"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("js", "javascript"),
    ("jsx", "jsx"),
    ("mjs", "javascript"),
    ("go", "go"),
    ("rb", "ruby"),
    ("java", "java"),
    ("kt", "kotlin"),
    ("c", "c"),
    ("h", "c"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("hpp", "cpp"),
    ("cs", "csharp"),
    ("m", "objectivec"),
    ("sh", "bash"),
    ("zsh", "bash"),
    ("json", "json"),
    ("toml", "toml"),
    ("yml", "yaml"),
    ("yaml", "yaml"),
    ("md", "markdown"),
    ("html", "html"),
    ("css", "css"),
    ("scss", "scss"),
    ("sql", "sql"),
    ("lua", "lua"),
    ("vue", "vue"),
    ("svelte", "svelte"),
    ("xml", "xml"),
    ("graphql", "graphql"),
    ("proto", "protobuf"),
];

/// A link the viewer takes, picked apart.
#[derive(Debug, PartialEq)]
pub struct Link {
    pub repo: String,
    /// "pull", "commit", "issue" or "file"
    pub kind: &'static str,
    /// The PR's or the issue's number, the commit's sha, or the file's ref.
    pub reference: String,
    /// A file's path in the repo, decoded.
    pub path: Option<String>,
    pub lines: Option<[u64; 2]>,
    /// The link without its #fragment.
    pub url: String,
}

fn link_pattern() -> Regex {
    Regex::new(
        r"^https://github\.com/(?P<owner>[A-Za-z0-9-]{1,39})/(?P<repo>[A-Za-z0-9._-]{1,100})/(?:(?P<kind>pull|commit|issues)/(?P<ref>[0-9]{1,9}|[0-9a-f]{7,40})(?:/[^#?]*)?|blob/(?P<blob_ref>[A-Za-z0-9._-]{1,100})/(?P<path>[^#?]+))(?:\?[^#]*)?(?:#(?P<frag>\S*))?$",
    )
    .expect("the link pattern compiles")
}

/// `%2F` and friends in a path, as GitHub's links write them.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// One path segment for GitHub's API: everything but letters, digits and `-._~` escaped.
fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A GitHub link the viewer shows: a PR, a commit, an issue, or a file at a ref (`#L10-L20` for lines).
pub fn parse(url: &str) -> anyhow::Result<Link> {
    let Some(m) = link_pattern().captures(url.trim()) else {
        bail!("that isn't a link to a GitHub PR, commit, issue or file");
    };
    let repo = format!("{}/{}", &m["owner"], &m["repo"]);
    let lines = m.name("frag").and_then(|f| {
        let lm = Regex::new(r"^L(\d{1,7})(?:-L(\d{1,7}))?$").ok()?.captures(f.as_str())?;
        let a: u64 = lm[1].parse().ok()?;
        let b: u64 = lm.get(2).map_or(Some(a), |b| b.as_str().parse().ok())?;
        Some([a.min(b), a.max(b)])
    });
    let url = url.trim().split('#').next().unwrap_or_default().to_string();
    if let Some(kind) = m.name("kind") {
        let kind = match kind.as_str() {
            "pull" => "pull",
            "issues" => "issue",
            _ => "commit",
        };
        if kind != "commit" && !m["ref"].bytes().all(|b| b.is_ascii_digit()) {
            bail!("that isn't a link to a GitHub PR, commit, issue or file");
        }
        return Ok(Link { repo, kind, reference: m["ref"].to_string(), path: None, lines, url });
    }
    let reference = m["blob_ref"].to_string();
    let path = decode(&m["path"]).map(|p| p.trim_matches('/').to_string());
    let path = path.filter(|p| p.split('/').all(|x| !matches!(x, "" | "." | "..")) && !p.contains(['\\', '\0']));
    if path.is_none() || matches!(reference.as_str(), "." | "..") {
        bail!("that isn't a link to a file on GitHub"); // never another API path
    }
    Ok(Link { repo, kind: "file", reference, path, lines, url })
}

/// A GitHub token and nothing else: a classic or OAuth one (`gho_…`, `ghp_…`) or a fine-grained one.
pub fn is_token(s: &str) -> bool {
    Regex::new(r"^(gh[pousr]_[A-Za-z0-9]{36,251}|github_pat_[A-Za-z0-9_]{22,255})$").expect("compiles").is_match(s)
}

/// What the viewer calls a file's language, from its name.
pub fn language(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    if name == "dockerfile" || name.starts_with("dockerfile.") {
        return Some("dockerfile");
    }
    let (_, ext) = name.rsplit_once('.')?;
    LANGUAGES.iter().find(|(e, _)| *e == ext).map(|(_, l)| *l)
}

/// GitHub as this machine's `gh` login sees it.
pub struct GitHub {
    gh: PathBuf,
    token: Option<String>,
}

/// Where `gh` is: the setting, else on the PATH, else where Homebrew puts it, else the one this extension
/// installed in its state folder.
pub fn find_gh(setting: &str, state: Option<&Path>) -> Option<PathBuf> {
    if !setting.trim().is_empty() {
        return Some(PathBuf::from(setting.trim()));
    }
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("gh")).collect::<Vec<_>>())
        .unwrap_or_default();
    on_path
        .into_iter()
        .chain(["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "/usr/bin/gh"].map(PathBuf::from))
        .find(|p| p.is_file())
        .or_else(|| state.and_then(install::installed))
}

/// An answer from GitHub that isn't a 200: 404 and its kin, as GitHub said.
#[derive(Debug)]
pub struct NotThere(pub String);

impl std::fmt::Display for NotThere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotThere {}

impl GitHub {
    pub fn new(gh: impl AsRef<Path>) -> Self {
        GitHub { gh: gh.as_ref().to_path_buf(), token: None }
    }

    /// Ask as the vault's account (GH_TOKEN) rather than the machine's own gh login.
    pub fn token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    /// `gh api <path>`: the body as bytes, cut at `max`, and whether there was more.
    fn call(&self, path: &str, raw: bool, max: usize) -> anyhow::Result<(Vec<u8>, bool)> {
        self.call_with(path, &[], None, raw, max)
    }

    /// `gh api <path> <args…>`, with `input` on its stdin (`--input -`, a request's JSON body).
    fn call_with(
        &self,
        path: &str,
        args: &[&str],
        input: Option<Vec<u8>>,
        raw: bool,
        max: usize,
    ) -> anyhow::Result<(Vec<u8>, bool)> {
        let mut cmd = Command::new(&self.gh);
        cmd.arg("api").arg(path).args(args);
        if raw {
            cmd.args(["-H", "Accept: application/vnd.github.raw"]);
        }
        if let Some(t) = &self.token {
            cmd.env("GH_TOKEN", t);
        }
        let mut child = cmd
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .spawn()
            .with_context(|| format!("couldn't run {}", self.gh.display()))?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            std::thread::spawn(move || {
                let _ = std::io::Write::write_all(&mut stdin, &input);
            });
        }
        let mut out = child.stdout.take().context("gh's output")?;
        let mut err = child.stderr.take().context("gh's errors")?;
        let reader = std::thread::spawn(move || {
            let mut body = Vec::new();
            let mut more = false;
            let mut buf = [0u8; 65536];
            loop {
                match out.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) if body.len() < max + 1 => body.extend_from_slice(&buf[..n]),
                    Ok(_) => more = true, // read on so gh isn't stuck writing
                }
            }
            if body.len() > max {
                body.truncate(max);
                more = true;
            }
            (body, more)
        });
        let errs = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let started = Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if started.elapsed() > CALL_TIMEOUT {
                let _ = child.kill();
                let _ = child.wait();
                bail!("GitHub took over {} s", CALL_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let (body, more) = reader.join().unwrap_or_default();
        let errs = errs.join().unwrap_or_default();
        if !status.success() {
            let said = errs.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            // GitHub's reason is in the answer's errors ("Can not approve your own pull request"), gh says
            // only its kind ("Unprocessable Entity")
            let said = match (errors_in(&body), Regex::new(r"^gh: .* \(HTTP (\d{3})\)$").ok()) {
                (Some(why), Some(re)) if re.is_match(&said) => {
                    re.replace(&said, format!("gh: {why} (HTTP $1)")).into_owned()
                }
                _ => said,
            };
            if said.contains("HTTP 404") || said.contains("HTTP 403") || said.contains("HTTP 422") {
                return Err(NotThere(said).into());
            }
            bail!("gh api {path}: {}", if said.is_empty() { status.to_string() } else { said });
        }
        Ok((body, more))
    }

    fn json(&self, path: &str) -> anyhow::Result<Value> {
        let (body, more) = self.call(path, false, 64 * 1024 * 1024)?;
        if more {
            bail!("GitHub's answer for {path} is too big");
        }
        serde_json::from_slice(&body).with_context(|| format!("GitHub's answer for {path}"))
    }

    /// A list GitHub pages, up to FILES_MAX.
    fn list(&self, path: &str) -> anyhow::Result<Vec<Value>> {
        let mut out = vec![];
        for page in 1..=FILES_MAX / 100 {
            let items = self.json(&format!("{path}?per_page=100&page={page}"))?;
            let items = items.as_array().cloned().unwrap_or_default();
            out.extend(items.iter().filter(|f| f.is_object()).cloned());
            if items.len() < 100 {
                break;
            }
        }
        Ok(out)
    }

    fn pull_files(&self, repo: &str, number: u64) -> anyhow::Result<Vec<Value>> {
        Ok(self.list(&format!("repos/{repo}/pulls/{number}/files"))?.iter().map(changed).collect())
    }

    /// A PR's or an issue's conversation: its description, comments, and a PR's reviews and their comments
    /// on lines, each oldest first.
    fn thread(&self, repo: &str, d: &Value, is_pull: bool) -> anyhow::Result<Value> {
        let n = d["number"].as_u64().unwrap_or_default();
        let comments = self.list(&format!("repos/{repo}/issues/{n}/comments"))?;
        let (reviews, on_lines) = if is_pull {
            (
                self.list(&format!("repos/{repo}/pulls/{n}/reviews"))?,
                self.list(&format!("repos/{repo}/pulls/{n}/comments"))?,
            )
        } else {
            (vec![], vec![])
        };
        Ok(json!({
            "author": d["user"]["login"], "body": d["body"], "created_at": d["created_at"], "url": d["html_url"],
            "comments": comments.iter().map(|c| json!({"author": c["user"]["login"], "body": c["body"],
                                                      "created_at": c["created_at"], "url": c["html_url"]}))
                                .collect::<Vec<_>>(),
            "reviews": reviews.iter().map(|r| json!({"author": r["user"]["login"], "state": r["state"], "body": r["body"],
                                                    "submitted_at": r["submitted_at"], "url": r["html_url"]}))
                              .collect::<Vec<_>>(),
            "review_comments": on_lines.iter().map(|c| json!({
                "author": c["user"]["login"], "body": c["body"], "created_at": c["created_at"], "path": c["path"],
                "line": if c["line"].is_null() { c["original_line"].clone() } else { c["line"].clone() },
                "url": c["html_url"], "in_reply_to": c.get("in_reply_to_id").cloned().unwrap_or(Value::Null),
                "review_id": c.get("pull_request_review_id").cloned().unwrap_or(Value::Null)}))
                .collect::<Vec<_>>(),
        }))
    }

    /// A request with a JSON body (`-X POST`, `PUT`…), for what changes GitHub: its answer, or GitHub's own
    /// words for why not.
    fn send(&self, method: &str, path: &str, body: &Value) -> anyhow::Result<Value> {
        let input = serde_json::to_vec(body)?;
        let (out, _) = self
            .call_with(path, &["-X", method, "--input", "-"], Some(input), false, 1024 * 1024)
            .map_err(|e| anyhow::anyhow!(said(&e)))?;
        Ok(serde_json::from_slice(&out).unwrap_or(Value::Null))
    }

    /// An open PR as this login may act on it: who that is, how the repo lets it be merged and whether
    /// this login may, whether it can be (GitHub's mergeable state), its review decision and its checks.
    pub fn merge_status(&self, repo: &str, number: u64) -> anyhow::Result<Value> {
        let (owner, name) = repo.split_once('/').context("a repo is owner/name")?;
        let query = format!("query={MERGE_QUERY}");
        let (owner, name, number) = (format!("owner={owner}"), format!("name={name}"), format!("number={number}"));
        let (out, _) =
            self.call_with("graphql", &["-f", &query, "-F", &owner, "-F", &name, "-F", &number], None, false, 1 << 20)?;
        let got: Value = serde_json::from_slice(&out).context("GitHub's answer for a PR's merge status")?;
        Ok(merge_of(&got["data"]))
    }

    /// A review of a PR as this login, on the head it was shown at: APPROVE, REQUEST_CHANGES or COMMENT,
    /// with its words (GitHub wants words for the last two).
    pub fn review(&self, repo: &str, number: u64, event: &str, body: &str, sha: &str) -> anyhow::Result<()> {
        if !matches!(event, "APPROVE" | "REQUEST_CHANGES" | "COMMENT") {
            bail!("{event} isn't a review: APPROVE, REQUEST_CHANGES or COMMENT");
        }
        let mut review = json!({"event": event, "commit_id": sha});
        if !body.trim().is_empty() {
            review["body"] = json!(body.trim());
        }
        self.send("POST", &format!("repos/{repo}/pulls/{number}/reviews"), &review)?;
        Ok(())
    }

    /// Merges a PR as this login, only while its head is still `sha` (what was shown), by `method`: merge,
    /// squash or rebase.
    pub fn merge(&self, repo: &str, number: u64, method: &str, sha: &str) -> anyhow::Result<()> {
        if !matches!(method, "merge" | "squash" | "rebase") {
            bail!("{method} isn't a way to merge: merge, squash or rebase");
        }
        let done = self.send(
            "PUT",
            &format!("repos/{repo}/pulls/{number}/merge"),
            &json!({"merge_method": method, "sha": sha}),
        )?;
        if done["merged"] == false {
            bail!("{}", done["message"].as_str().unwrap_or("GitHub didn't merge it"));
        }
        Ok(())
    }

    /// What the viewer shows for a link.
    pub fn view(&self, link: &Link) -> anyhow::Result<Value> {
        let repo = &link.repo;
        let mut out = json!({"kind": link.kind, "repo": repo, "url": link.url, "pull": null, "issue": null,
                             "file": null, "files": [], "thread": null, "lines": link.lines});
        match link.kind {
            "pull" => {
                let d = self.json(&format!("repos/{repo}/pulls/{}", link.reference)).map_err(not_there("PR"))?;
                out["title"] = d["title"].clone();
                out["ref"] = d["head"]["sha"].clone();
                out["pull"] = pull(&d, d["merged"].as_bool() == Some(true));
                out["files"] = Value::Array(self.pull_files(repo, link.reference.parse()?)?);
                out["thread"] = self.thread(repo, &d, true)?;
                // an open PR: what this login may do with it (an answer that fails says why, in place)
                if d["state"] == "open" && d["merged"] != true {
                    out["merge"] = match self.merge_status(repo, link.reference.parse()?) {
                        Ok(m) => m,
                        Err(e) => json!({"error": said(&e)}),
                    };
                }
            }
            "issue" => {
                let d = self.json(&format!("repos/{repo}/issues/{}", link.reference)).map_err(not_there("issue"))?;
                out["title"] = d["title"].clone();
                out["ref"] = Value::Null;
                out["issue"] = json!({"number": d["number"], "title": d["title"], "state": d["state"]});
                out["thread"] = self.thread(repo, &d, false)?;
            }
            "commit" => {
                let d = self.json(&format!("repos/{repo}/commits/{}", link.reference)).map_err(not_there("commit"))?;
                let message = d["commit"]["message"].as_str().unwrap_or_default();
                out["title"] = json!(message.lines().next().unwrap_or_default());
                out["ref"] = d["sha"].clone();
                let files = d["files"].as_array().cloned().unwrap_or_default();
                out["files"] = files.iter().filter(|f| f.is_object()).take(FILES_MAX).map(changed).collect();
            }
            _ => {
                let path = link.path.clone().unwrap_or_default();
                let api_path = path.split('/').map(encode).collect::<Vec<_>>().join("/");
                let (raw, truncated) = self
                    .call(&format!("repos/{repo}/contents/{api_path}?ref={}", encode(&link.reference)), true, TEXT_MAX)
                    .map_err(not_there("file"))?;
                let text = match String::from_utf8(raw) {
                    Ok(t) => Some(t),
                    // cut mid-character: up to the last whole one
                    Err(e) if truncated && e.utf8_error().error_len().is_none() => {
                        let n = e.utf8_error().valid_up_to();
                        Some(String::from_utf8_lossy(&e.into_bytes()[..n]).into_owned())
                    }
                    Err(_) => None, // a binary file: the panel says so
                };
                out["title"] = json!(path.rsplit('/').next().unwrap_or(&path));
                out["ref"] = json!(link.reference);
                out["file"] = json!({"path": path, "text": text, "language": language(&path), "truncated": truncated});
                // how this file changed in the PR its commit is in, when there is one
                let prs = self.json(&format!("repos/{repo}/commits/{}/pulls", encode(&link.reference)));
                if let Some(first) = prs.ok().and_then(|p| p.as_array().and_then(|a| a.first().cloned())) {
                    out["pull"] = pull(&first, first["merged_at"].is_string());
                    if let Some(n) = first["number"].as_u64() {
                        let files = self.pull_files(repo, n)?;
                        out["files"] = files
                            .into_iter()
                            .filter(|f| f["path"] == json!(path) || f["previous_path"] == json!(path))
                            .collect();
                    }
                }
            }
        }
        Ok(out)
    }
}

/// An open PR's merge status in one GraphQL query: the login, the repo's merge methods and this login's
/// permission, and the PR's mergeability, review decision and its head's checks.
const MERGE_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { viewer { login } \
    repository(owner: $owner, name: $name) { mergeCommitAllowed squashMergeAllowed rebaseMergeAllowed viewerPermission \
    pullRequest(number: $number) { mergeable mergeStateStatus reviewDecision isDraft viewerDidAuthor baseRefName \
    viewerLatestReview { state } commits(last: 1) { nodes { commit { oid statusCheckRollup { state \
    contexts(first: 100) { nodes { __typename ... on CheckRun { status conclusion } ... on StatusContext { state } } } } } } } } } }";

/// MERGE_QUERY's answer as the viewer keeps it.
pub fn merge_of(data: &Value) -> Value {
    let repo = &data["repository"];
    let pr = &repo["pullRequest"];
    let methods: Vec<&str> =
        [("mergeCommitAllowed", "merge"), ("squashMergeAllowed", "squash"), ("rebaseMergeAllowed", "rebase")]
            .iter()
            .filter(|(allowed, _)| repo[*allowed] == true)
            .map(|(_, method)| *method)
            .collect();
    let rollup = &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"];
    let (mut passed, mut failed, mut pending) = (0, 0, 0);
    for c in rollup["contexts"]["nodes"].as_array().into_iter().flatten() {
        let outcome = match c["__typename"].as_str() {
            Some("CheckRun") if c["status"] != "COMPLETED" => "pending",
            Some("CheckRun") => match c["conclusion"].as_str() {
                Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => "passed",
                _ => "failed",
            },
            _ => match c["state"].as_str() {
                Some("SUCCESS") => "passed",
                Some("PENDING" | "EXPECTED") => "pending",
                _ => "failed",
            },
        };
        match outcome {
            "passed" => passed += 1,
            "pending" => pending += 1,
            _ => failed += 1,
        }
    }
    json!({
        "viewer": data["viewer"]["login"], "methods": methods,
        "can_merge": matches!(repo["viewerPermission"].as_str(), Some("ADMIN" | "MAINTAIN" | "WRITE")),
        "mergeable": pr["mergeable"], "state": pr["mergeStateStatus"], "review": pr["reviewDecision"],
        "draft": pr["isDraft"], "authored": pr["viewerDidAuthor"], "my_review": pr["viewerLatestReview"]["state"],
        "base": pr["baseRefName"], "head": pr["commits"]["nodes"][0]["commit"]["oid"],
        "checks": {"state": rollup["state"], "passed": passed, "failed": failed, "pending": pending},
    })
}

/// The reasons in an error answer's `errors` (strings, or `{message}` / `{field, code}`), joined.
fn errors_in(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let why: Vec<String> = v["errors"]
        .as_array()?
        .iter()
        .filter_map(|e| match e {
            Value::String(s) => Some(s.clone()),
            _ => e["message"].as_str().map(String::from).or_else(|| {
                Some(format!(
                    "{} {}",
                    e["field"].as_str()?,
                    e["code"].as_str().unwrap_or("isn't right").replace('_', " ")
                ))
            }),
        })
        .collect();
    (!why.is_empty()).then(|| why.join("; "))
}

/// What GitHub said, from gh's error: `gh: Can not approve your own pull request (HTTP 422)` is "Can not
/// approve your own pull request".
pub fn said(e: &anyhow::Error) -> String {
    let text = e.to_string();
    let line = text.rsplit("gh: ").next().unwrap_or(&text).trim();
    let line = Regex::new(r"\s*\(HTTP \d{3}\)$").map_or(line.to_string(), |re| re.replace(line, "").into_owned());
    if line.is_empty() {
        text
    } else {
        line
    }
}

fn not_there(what: &'static str) -> impl Fn(anyhow::Error) -> anyhow::Error {
    move |e| match e.downcast_ref::<NotThere>() {
        Some(_) => anyhow::anyhow!("GitHub doesn't show that {what} to this machine's gh login"),
        None => e,
    }
}

fn pull(d: &Value, merged: bool) -> Value {
    let state = if merged {
        json!("merged")
    } else if d["draft"].as_bool() == Some(true) && d["state"] == "open" {
        json!("draft")
    } else {
        d["state"].clone()
    };
    json!({"number": d["number"], "title": d["title"], "state": state})
}

fn changed(f: &Value) -> Value {
    json!({"path": f["filename"], "previous_path": f.get("previous_filename").cloned().unwrap_or(Value::Null),
           "status": f["status"], "additions": f["additions"], "deletions": f["deletions"],
           "patch": f.get("patch").cloned().unwrap_or(Value::Null)})
}

/// Answers kept for a minute, by link (fragment aside: the lines are the link's own).
#[derive(Default)]
pub struct Kept(Mutex<HashMap<String, (Instant, Value)>>);

impl Kept {
    pub fn get(&self, link: &Link) -> Option<Value> {
        let kept = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let (at, v) = kept.get(&link.url.to_lowercase())?;
        (at.elapsed() < KEEP).then(|| {
            let mut v = v.clone();
            v["lines"] = json!(link.lines);
            v
        })
    }

    pub fn put(&self, link: &Link, v: &Value) {
        let mut kept = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if kept.len() > 500 {
            kept.retain(|_, (at, _)| at.elapsed() < KEEP);
        }
        kept.insert(link.url.to_lowercase(), (Instant::now(), v.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_status_and_githubs_reasons() {
        let data = json!({"viewer": {"login": "james"}, "repository": {
            "mergeCommitAllowed": true, "squashMergeAllowed": false, "rebaseMergeAllowed": true, "viewerPermission": "WRITE",
            "pullRequest": {"mergeable": "MERGEABLE", "mergeStateStatus": "BLOCKED", "reviewDecision": "REVIEW_REQUIRED",
                            "isDraft": false, "viewerDidAuthor": true, "baseRefName": "main", "viewerLatestReview": {"state": "COMMENTED"},
                            "commits": {"nodes": [{"commit": {"oid": "abc", "statusCheckRollup": {"state": "PENDING", "contexts": {"nodes": [
                                {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SKIPPED"},
                                {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "TIMED_OUT"},
                                {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": null},
                                {"__typename": "StatusContext", "state": "ERROR"}]}}}}]}}}});
        assert_eq!(
            merge_of(&data),
            json!({"viewer": "james", "methods": ["merge", "rebase"], "can_merge": true, "mergeable": "MERGEABLE",
                   "state": "BLOCKED", "review": "REVIEW_REQUIRED", "draft": false, "authored": true, "my_review": "COMMENTED",
                   "base": "main", "head": "abc", "checks": {"state": "PENDING", "passed": 1, "failed": 2, "pending": 1}})
        );
        let read_only = json!({"repository": {"viewerPermission": "READ", "pullRequest": {}}});
        assert_eq!(merge_of(&read_only)["can_merge"], false);
        assert_eq!(
            said(&anyhow::anyhow!("gh api x: gh: Pull Request is not mergeable (HTTP 405)")),
            "Pull Request is not mergeable"
        );
        assert_eq!(said(&anyhow::anyhow!("couldn't run gh")), "couldn't run gh");
        assert_eq!(
            errors_in(br#"{"message":"Unprocessable Entity","errors":["Can not approve your own pull request"]}"#)
                .as_deref(),
            Some("Can not approve your own pull request")
        );
        assert_eq!(
            errors_in(
                br#"{"errors":[{"resource":"Label","code":"missing_field","field":"name"},{"message":"too long"}]}"#
            )
            .as_deref(),
            Some("name missing field; too long")
        );
        assert_eq!(errors_in(br#"{"message":"Not Found"}"#), None);
    }

    #[test]
    fn links() {
        let l = parse("https://github.com/pointman-ai/pointman/pull/42/files#diff-abc").unwrap();
        assert_eq!(
            (l.kind, l.reference.as_str(), l.url.as_str()),
            ("pull", "42", "https://github.com/pointman-ai/pointman/pull/42/files")
        );
        let l = parse("https://github.com/pointman-ai/pointman/commit/6946239").unwrap();
        assert_eq!((l.kind, l.repo.as_str()), ("commit", "pointman-ai/pointman"));
        let l = parse("https://github.com/a/b/blob/c8b3559/src/my%20file.py#L10-L4").unwrap();
        assert_eq!((l.kind, l.path.as_deref(), l.lines), ("file", Some("src/my file.py"), Some([4, 10])));
        assert_eq!(parse("https://github.com/a/b/blob/main/x.rs#L340").unwrap().lines, Some([340, 340]));
        for bad in [
            "https://github.com/a/b/issues/abc1234",
            "https://github.com/a/b/discussions/1",
            "https://gitlab.com/a/b/pull/1",
            "https://github.com/a/b/blob/main/../secrets",
            "https://github.com/a/b/blob/main/x/%2E%2E/y",
            "https://github.com/a/b/blob/../x",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn languages_and_segments() {
        assert_eq!(language("node/src/main.rs"), Some("rust"));
        assert_eq!(language("Dockerfile"), Some("dockerfile"));
        assert_eq!(language("LICENSE"), None);
        assert_eq!(encode("my file#1.py"), "my%20file%231.py");
    }
}
