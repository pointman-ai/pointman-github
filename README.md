# gitview

What a GitHub link shows, for Pointman's viewer panel: a PR's or a commit's changed files with GitHub's
unified diff of each, or a file at a commit around its lines, with how that file changed in its PR. A
Pointman extension that runs on a node. Nothing of it is in core (James, Pointman thread 649 [4443]).

GitHub is asked with the machine's own `gh` login, so the viewer sees what that login sees. Install
[GitHub's CLI](https://cli.github.com) and run `gh auth login` on the machine first. If `gh` isn't on the
node's PATH, or in /opt/homebrew/bin or /usr/local/bin, set its path in the extension's `gh` setting.

```
pointman add gitview
```

## The apps' call

Through core's jobs, as for any extension's command:

```
POST /v1/workspaces/{ws}/jobs  {"kind": "gitview.view", "params": {"url": "<a GitHub link>"}}
```

The job's result (its `job.updated` event, or `GET /v1/workspaces/{ws}/jobs/{job}`):

```
{kind: "file"|"pull"|"commit", repo, url, title, ref, lines: [from, to] | null,
 pull: {number, title, state} | null,             // state: open, closed, merged or draft
 file: {path, text, language, truncated} | null,  // kind file: the file at ref
 files: [{path, previous_path, status, additions, deletions, patch | null}]}
```

- A PR or a commit link gives every changed file, up to 300. `patch` is GitHub's unified diff, or null
  when GitHub has none (a binary file, a very large change).
- A file link (`https://github.com/o/r/blob/<ref>/<path>#L10-L20`) gives `lines`, the file's `text` (null
  for a binary file; `truncated` past 512 KB), its commit's PR in `pull`, and only that file's change in
  `files`.
- A failed job says why: a link that isn't a PR, a commit or a file, or one GitHub doesn't show to the
  machine's login. The apps then open the link on GitHub.

Answers are kept for a minute.

## Building

`cargo test` runs it against a stand-in node and a stand-in `gh`; `cargo test -- --ignored` asks the real
GitHub with the machine's `gh`. A release is a tag that's extension.toml's version (`git tag v0.1.0 && git
push origin v0.1.0`): each platform is built and attached, and core's catalogue takes it from there.
