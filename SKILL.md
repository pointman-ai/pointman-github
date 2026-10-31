---
name: github
description: Writing on GitHub (pull requests, issues, comments, reviews, commits) from a run on a machine with the GitHub extension. It goes out as Pointman App, GitHub's bot for Pointman, or as the machine's own gh login where the App can't reach.
---

# Writing on GitHub as Pointman App

In your runs, `gh` and git act on GitHub as **Pointman App**: PRs, issues, comments, reviews, pushes and commits all show as the App's bot (`<its name>[bot]`). The exception is a commit in a run a person started: it is authored as that person, with the App as committer and `Co-authored-by`, added by the `git` the extension gives you. Don't write that line or set the author yourself. The extension gives each command a short-lived token for the repo's owner, so you never sign in, set a token, or change git's name and email.

Where the App can't act (it isn't installed on the repo's account, or GitHub refuses it), `gh` and git use this machine's own `gh` login instead, and say so in a line starting `github:`. What you do then shows on GitHub as that login's person, not the bot.

The people who read it know the repo, not you or Pointman's board, so write for them:

- **About the change itself.** What changed, why, how it was tested, and what a reviewer should look at closely. Don't write as a person ("I think", "my fix"), and never speak for or about your person ("waits for James's review", "James asked for").
- **No behind-the-scenes process.** Leave out the board, its threads, posts and task refs, and who reviews or approves what between you. The repo's own references stay: its issues and PRs, and its tracker's refs such as "Fixes ALA-2063".
- **The repo's way first:** its templates, commit style and labels. No `Co-Authored-By` lines of your own: the extension adds the one that's needed.
- **Anything for your person goes on the board,** not on GitHub: that something waits on their review, what you weren't sure of, what they need to decide.
- **Only what they asked for.** Don't approve, merge or close a pull request, or comment on someone else's, unless they asked you to for that one.

| Don't write | Write |
|---|---|
| "Tests only, no product change. Merging waits for James's review." | "Tests only, no product change." |
| "James asked me to fix the flaky notifications tests." | "Fixes the flaky notifications tests." |
| "Approved by James in Pointman thread 4159." | (nothing: say it to your person on the board) |

**When the App can't reach a repo.** A `github:` line saying `gh` or git used "this machine's own login" is allowed: carry on. Tell your person on the board which account or repo it was, so they can install the App there or give it the permission. If the machine's login fails too, tell them that. Never sign in yourself (`gh auth login`), set a token, call another `gh` by its full path, or change git's credential settings: the `gh` and git you're given fall back by themselves.
