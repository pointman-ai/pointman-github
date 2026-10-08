# gh

GitHub on Pointman's nodes (James, Pointman thread 649 [4500], [4505]: the GitHub integration lives on
nodes and in the apps, not in core). Today: what a GitHub link shows, for the apps' viewer panel: a PR's or a commit's changed files with GitHub's
unified diff of each, a file at a commit around its lines with how that file changed in its PR, and a PR's
or an issue's conversation. A Pointman extension that runs on a node. Nothing of it is in core (James, Pointman thread 649 [4443]).

GitHub is asked with the machine's own `gh` login, so the viewer sees what that login sees. Install
[GitHub's CLI](https://cli.github.com) and run `gh auth login` on the machine first. If `gh` isn't on the
node's PATH, or in /opt/homebrew/bin or /usr/local/bin, set its path in the extension's `cli` setting.

```
pointman add gh
```

## The apps' call

Through core's jobs, as for any extension's command:

```
POST /v1/workspaces/{ws}/jobs  {"kind": "gh.view", "params": {"url": "<a GitHub link>"}}
```

The job's result (its `job.updated` event, or `GET /v1/workspaces/{ws}/jobs/{job}`):

```
{kind: "file"|"pull"|"commit"|"issue", repo, url, title, ref, lines: [from, to] | null,
 pull: {number, title, state} | null,             // state: open, closed, merged or draft
 issue: {number, title, state} | null,
 file: {path, text, language, truncated} | null,  // kind file: the file at ref
 files: [{path, previous_path, status, additions, deletions, patch | null}],
 thread: {author, body, created_at, url,          // a PR's or an issue's, else null
          comments: [{author, body, created_at, url}],
          reviews: [{author, state, body, submitted_at, url}],            // a PR's
          review_comments: [{author, body, created_at, path, line, url, in_reply_to}]} | null}
```

- A PR or an issue link gives its conversation in `thread`: the description (`body`, markdown), its
  comments, and a PR's reviews (`state`: APPROVED, CHANGES_REQUESTED, COMMENTED…) and comments on lines,
  each oldest first, up to 300 of each. `in_reply_to` is the line comment it answers.
- A PR or a commit link gives every changed file, up to 300. `patch` is GitHub's unified diff, or null
  when GitHub has none (a binary file, a very large change).
- A file link (`https://github.com/o/r/blob/<ref>/<path>#L10-L20`) gives `lines`, the file's `text` (null
  for a binary file; `truncated` past 512 KB), its commit's PR in `pull`, and only that file's change in
  `files`.
- A failed job says why: a link that isn't a PR, a commit, an issue or a file, or one GitHub doesn't show
  to the machine's login. The apps then open the link on GitHub.

Answers are kept for a minute.

## Building

`cargo test` runs it against a stand-in node and a stand-in `gh`; `cargo test -- --ignored` asks the real
GitHub with the machine's `gh`. A release is a tag that's extension.toml's version (`git tag v0.1.0 && git
push origin v0.1.0`): each platform is built and attached, and core's catalogue takes it from there.
