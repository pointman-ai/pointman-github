# github

GitHub on Pointman's machines (James, Pointman thread 649 [4443], [4500], [4505]): GitHub's CLI on each
node, installed where there's none, GitHub accounts signed in from the apps and given to agents' runs, and
what a GitHub link shows for the apps' viewer panel: a PR's or a commit's changed files with GitHub's
unified diff of each, a file at a commit around its lines with how that file changed in its PR, and a PR's
or an issue's conversation.

It's the machine half of GitHub on Pointman. Pointman's GitHub App stays in core, for what GitHub sends as
it happens: PR and CI feeds, `/github watch`, commit refs moving tasks, link pills and releases (James,
[4670]–[4673]). Core's catalogue lets this repo alone take the id `github`, and the apps show the two as one
GitHub: `/github watch` is core's, `/github add` this extension's.

GitHub is asked with the vault's account (`/github add`), else the machine's own `gh` login, so the viewer
sees what that account sees. If `gh` isn't on the node's PATH, or in /opt/homebrew/bin or /usr/local/bin,
the extension installs its own; or set its path in the extension's `cli` setting.

```
pointman add github
```

## The apps' call

Through core's jobs, as for any extension's command:

```
POST /v1/workspaces/{ws}/jobs  {"kind": "github.view", "params": {"url": "<a GitHub link>"}}
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
          review_comments: [{author, body, created_at, path, line, url, in_reply_to, review_id}]} | null}
```

- A PR or an issue link gives its conversation in `thread`: the description (`body`, markdown), its
  comments, and a PR's reviews (`state`: APPROVED, CHANGES_REQUESTED, COMMENTED…) and comments on lines,
  each oldest first, up to 300 of each. `in_reply_to` is the line comment it answers
  (the id in its url's `#discussion_r<id>`), `review_id` the review it was made in (`#pullrequestreview-<id>`).
- A PR or a commit link gives every changed file, up to 300. `patch` is GitHub's unified diff, or null
  when GitHub has none (a binary file, a very large change).
- A file link (`https://github.com/o/r/blob/<ref>/<path>#L10-L20`) gives `lines`, the file's `text` (null
  for a binary file; `truncated` past 512 KB), its commit's PR in `pull`, and only that file's change in
  `files`.
- A failed job says why: a link that isn't a PR, a commit, an issue or a file, or one GitHub doesn't show
  to the machine's login. The apps then open the link on GitHub.

Answers are kept for a minute.

## The view pane

The same, as a pane every app draws alike (Pointman's docs/dev/panes.md, MB-343), in place of the Mac's
own viewer: `[[panes]] id = "view"`, opened with `{"url": "<a GitHub link>"}`.

- A PR: a heading (repo, number, state, files and lines changed), then tabs: **Conversation**, a
  `timeline` of the description, comments, reviews and comments on lines, oldest first, each by GitHub's
  own anchor; and **Files**, a `diff`.
- A commit: its `diff`. An issue: its `timeline`. A file: its `code` scrolled to the link's lines, and how
  it changed in its PR.
- A diff too big for a frame (1 MiB) leaves out its biggest files first, and a line says how many
  GitHub's page has.
- An open PR also says where it stands, above its tabs (`pr-status`): its checks, its review decision,
  whether it can merge into its base (GitHub's merge state), and the login that acts. Under that:
  - `pr-actions`: **Approve** (not on the login's own PR, which GitHub refuses, nor once it's approved),
    and a button per merge method the repo allows (**Merge**, **Squash and merge**, **Rebase and merge**),
    for a login that may merge, unless it conflicts (nothing merges that). Merging asks first, saying into
    which branch, how and as whom. When something stands in the way (a draft, a missing required review
    or check, a failed check, a base it's behind), the buttons are **Force merge** (red), and the question
    says what they override: a draft is marked ready first, and GitHub lets a login that may bypass the
    repo's rules (an admin, unless the rules bind admins too) merge past the rest, or says why not.
  - `pr-review`: a review with words: Comment, Approve or Request changes (only Comment on one's own PR).
  - A tap or a submit goes to GitHub as this machine's login (the vault's account, else its own gh
    login), on the head the pane showed: a merge never takes a commit pushed since. The pane shows
    GitHub's answer at once. What GitHub refuses comes back with GitHub's reason, such as "Pull Request is
    not mergeable".
- A PR's or an issue's page beyond its conversation (`manage` in pane.rs), as GitHub's own page has it:
  - `meta-line`: its labels, assignees and review requests (an issue's state too, and how it closed).
  - `state`, for a login that may write: **Close** (asking first) or **Reopen**; a PR's **Convert to draft**
    or **Ready for review**, **Update branch** while it's behind its base, and **Delete branch** once it's
    merged or closed (asking first); an issue's **Close as completed** or **Close as not planned**.
  - `meta`, a form: the title, a label to add or take off, someone to assign or unassign, and (an open
    PR's) a review to request or withdraw. A field left at "—" changes nothing; the title only when it
    changed.
  - `comment`, under the conversation: a comment, as GitHub's comment box.
- Each open pane asks GitHub again every minute and sends only what changed (the SDK's `show`), so a new
  comment slides in and the open tab and scroll stay put. A tab's `select` is the app's; any other input
  is refused with why. Every write goes to GitHub as this machine's login, and GitHub's answer shows at once.

## Building

`cargo test` runs it against a stand-in node and a stand-in `gh`; `cargo test -- --ignored` asks the real
GitHub with the machine's `gh`. A release is a tag that's extension.toml's version (`git tag v0.1.0 && git
push origin v0.1.0`): each platform is built and attached, and core's catalogue takes it from there.
