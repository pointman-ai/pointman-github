# github

A Pointman extension that puts GitHub's CLI on every node, and has agents' runs act on GitHub as Pointman App.
With it, an agent on any of your machines can run `gh` and git (push a branch, open a PR, read CI, comment on an
issue), and GitHub shows it as the App's bot, not as you. Where the App can't reach a repo, they use the machine's
own `gh` login instead.

```
pointman add github
```

## What it does

- **GitHub's CLI on each node:** if `gh` isn't on the node's PATH, or in /opt/homebrew/bin or
  /usr/local/bin, the extension installs the latest release from
  [cli/cli](https://github.com/cli/cli) into its own state folder, checked against that release's
  checksums. No Homebrew, no sudo, nothing outside that folder. The `cli` setting points it at another
  `gh` instead.
- **Agents act as Pointman App:** once your board has the App installed on an account, each run the node starts
  (Claude Code or Codex) acts on that account's repos as the App's bot:
  - The board gives the machine an hour-long installation token for each account the App is installed on. The
    App's key stays with the board; the extension keeps the tokens fresh in its state folder, readable only by
    the machine's user.
  - A `gh` first on the run's PATH runs GitHub's CLI with the token for the repo's owner: the one `-R`, a link or
    an API path names, else the folder's `upstream`, `github` or `origin` remote, else the first installation's.
  - git asks the extension for GitHub's password over https, pushes to `git@github.com:` remotes go over https,
    and commits are the bot's. Fetches still use the remote as it is.
  - Its guidance tells agents to write about the change itself, with nothing about how they work with you, and to
    tell you on the board when the App can't reach a repo.
- **The machine's own `gh` login where the App can't act:** until your board has the App, and for an account it
  isn't installed on, a run's `gh` and git use whoever `gh auth login` signed in on that machine, and say so on
  stderr. What they do then shows as that person. When GitHub refuses the App's token (a permission it wasn't
  given, "Resource not accessible by integration", or a repo its installation can't see, 403 or 404), a read
  (`view`, `list`, `status`, `checks`, `diff`, `watch`, `search`, a `gh api` GET) and git's push, fetch, pull,
  clone and ls-remote run once more as the machine's login, naming the account and GitHub's words. A refused write
  doesn't run again, since gh may have done part of it: it says how to run the rest as the machine's login
  (`POINTMAN_GITHUB_MACHINE_LOGIN=1`). Commits made in the run stay the bot's (or the run's person's), whoever
  pushes them. git never asks the keychain's own credential helper, only `gh`'s login.
- **`POINTMAN_GITHUB_APP_ONLY=1`** turns both fallbacks off, for workflow steps that must act only as the App (a
  release's statuses and merges): a permission the App lacks fails there, rather than going out as a person.

GitHub's own pages open in the apps' web panel, signed in as you. The workspace's GitHub connection
(the App, feeds of PRs and CI, `/github watch`, commit references that move tasks, link previews) is part of
Pointman itself.

## Settings

| Setting | Default | |
|---|---|---|
| `cli` | empty | GitHub's CLI, when it isn't on the node's PATH or in /opt/homebrew/bin or /usr/local/bin; empty, the extension installs its own |

## Install and upgrade

`pointman add github` installs it on this machine's node; `--nodes studio,mini` (or `--nodes all`) on
others. Each node fetches its own platform's build (macOS arm64 and x86_64, Linux x86_64 and arm64).
`pointman upgrade github` moves it to the newest release.

## Building and testing

```
cargo test
```

The tests run the extension against a stand-in node and a stand-in `gh`. `cargo test -- --ignored` installs
the latest real `gh` into a temporary folder.

To release, bump `version` in extension.toml and Cargo.toml and push a tag for it (`git tag v0.6.1 && git
push origin v0.6.1`). The release workflow builds each platform and attaches it, and the marketplace lists
it within 30 minutes.

## Licence

MIT. See `LICENSE`.
