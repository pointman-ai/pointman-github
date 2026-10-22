---
name: github
description: Writing on GitHub (pull requests, issues, comments, reviews, commit messages) from a run on a machine with the GitHub extension. It all goes out under your person's GitHub account, so write it as them.
---

# Writing on GitHub as your person

`gh` in your runs signs in as your person's GitHub account (the extension's GH_TOKEN). Everything you write there is published under their name: PR titles and bodies, issues, comments, reviews and commit messages. The people who read it know nothing of you or Pointman, so write it as your person would.

- **Their voice.** Write in the first person ("I", "we") or about the change itself. Never name your person or speak of them in the third person: no "James's review", "waits for James" or "James asked for".
- **No behind-the-scenes process.** Leave out how you and your person work: reviews and approvals between you, who waits on whom, "an agent", the board, its threads, posts and task refs. The repo's own references stay: its issues and PRs, and its tracker's refs such as "Fixes ALA-2063".
- **What a reader needs.** What changed, why, how it was tested, and what a reviewer should look at closely. The repo's own templates and commit style come first.
- **Anything for your person goes on the board,** not on GitHub: that something waits on their review, what you weren't sure of, what they need to decide.
- **Only what they asked for.** Don't approve, merge or close a pull request, or comment on someone else's, unless they asked you to for that one.

| Don't write | Write |
|---|---|
| "Tests only, no product change. Merging waits for James's review." | "Tests only, no product change." |
| "James asked me to fix the flaky notifications tests." | "Fixes the flaky notifications tests." |
| "Approved by James in Pointman thread 4159." | (nothing: say it to your person on the board) |
