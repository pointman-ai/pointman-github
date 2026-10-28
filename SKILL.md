---
name: github
description: Writing on GitHub (pull requests, issues, comments, reviews, commits) from a run on a machine with the GitHub extension. It goes out as Pointman App, GitHub's bot for Pointman, or before the board has the App, as your person's account.
---

# Writing on GitHub as Pointman App

In your runs, `gh` and git act on GitHub as **Pointman App** once your board has it installed: PRs, issues, comments, reviews, pushes and commits all show as the App's bot (`<its name>[bot]`). The extension gives each command a short-lived token for the repo's owner, so you never sign in, set a token, or change git's name and email. Until the board has the App, they go out under your person's GitHub account instead: write the same way.

The people who read it know the repo, not you or Pointman's board, so write for them:

- **About the change itself.** What changed, why, how it was tested, and what a reviewer should look at closely. Don't write as a person ("I think", "my fix"), and never speak for or about your person ("waits for James's review", "James asked for").
- **No behind-the-scenes process.** Leave out the board, its threads, posts and task refs, and who reviews or approves what between you. The repo's own references stay: its issues and PRs, and its tracker's refs such as "Fixes ALA-2063".
- **The repo's way first:** its templates, commit style and labels. No `Co-Authored-By` lines.
- **Anything for your person goes on the board,** not on GitHub: that something waits on their review, what you weren't sure of, what they need to decide.
- **Only what they asked for.** Don't approve, merge or close a pull request, or comment on someone else's, unless they asked you to for that one.

| Don't write | Write |
|---|---|
| "Tests only, no product change. Merging waits for James's review." | "Tests only, no product change." |
| "James asked me to fix the flaky notifications tests." | "Fixes the flaky notifications tests." |
| "Approved by James in Pointman thread 4159." | (nothing: say it to your person on the board) |

**When GitHub says no.** "Pointman App isn't installed" on an account, or a 403 or 404 on a repo you know is there, means the App can't reach it yet. Tell your person on the board which account or repo it was. Never get around it with another token, `gh auth login`, the machine's own `gh` by its full path, or git settings of your own.
