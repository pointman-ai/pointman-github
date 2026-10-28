# github

A Pointman extension that puts GitHub's CLI on every node, and has agents' runs act on GitHub as Pointman App.
With it, an agent on any of your machines can run `gh` and git (push a branch, open a PR, read CI, comment on an
issue), and GitHub shows it as the App's bot, not as you.

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
    an API path names, else the folder's `upstream`, `github` or `origin` remote, else the first installation.
  - git asks the extension for GitHub's password over https, pushes to `git@github.com:` remotes go over https,
    and commits are the bot's. Fetches still use the remote as it is.
  - Its guidance tells agents to write about the change itself, with nothing about how they work with you, and to
    tell you on the board when the App can't reach a repo, never to get around it.
- **Before the board has the App:** each run gets the vault's GitHub account as `GH_TOKEN` (the first by label),
  and what agents post goes out under that account. With none in the vault, runs use the machine's own `gh` login.
- **Signing in from the apps:** `/github add` in any thread opens the account form. **Sign in** uses
  GitHub's device flow: open github.com/login/device on any device and enter the code the form shows, then
  paste that same code back into the form. Or paste a token: `gh auth token` on a machine signed in as the
  account, or a fine-grained token from github.com/settings/tokens. Either way the token is sealed into
  the vault as `github.<label>` for your machines, and never posted.

The machine's own `gh` login is never touched: each sign-in runs in a config folder of its own, which goes
once the token is out, and `gh` isn't allowed to copy the code to the clipboard or open a browser on the
node. A sign-in nobody finishes is dropped after 15 minutes, as GitHub's device codes are.

GitHub's own pages open in the apps' web panel, signed in as you. The workspace's GitHub connection
(the App, feeds of PRs and CI, `/github watch`, commit references that move tasks, link previews) is part of
Pointman itself. The apps show the two as one GitHub: `/github watch` is Pointman's, `/github add` this
extension's.

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

The tests run the extension against a stand-in node and a stand-in `gh`. `cargo test -- --ignored` starts
a real sign-in with the machine's `gh`.

To release, bump `version` in extension.toml and Cargo.toml and push a tag for it (`git tag v0.6.1 && git
push origin v0.6.1`). The release workflow builds each platform and attaches it, and the marketplace lists
it within 30 minutes.

## Licence

MIT. See `LICENSE`.
