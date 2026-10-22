# github

A Pointman extension that puts GitHub's CLI on every node and gives agents' runs a GitHub account, signed
in from the apps. With it, an agent on any of your machines can run `gh` (open a PR, read CI, comment on an
issue) as the account you chose.

```
pointman add github
```

## What it does

- **GitHub's CLI on each node:** if `gh` isn't on the node's PATH, or in /opt/homebrew/bin or
  /usr/local/bin, the extension installs the latest release from
  [cli/cli](https://github.com/cli/cli) into its own state folder, checked against that release's
  checksums. No Homebrew, no sudo, nothing outside that folder. The `cli` setting points it at another
  `gh` instead.
- **An account for agents' runs:** each run the node starts (Claude Code or Codex) gets the vault's GitHub
  account as `GH_TOKEN`, so `gh` works for agents on every node. With several, it's the first by label.
- **Agents write as you there:** what they post on GitHub goes out under your account, so its guidance tells them to
  write in your voice, with nothing about how they work with you (no "waits for your review"). What's for you
  goes to you in Pointman.
  With none in the vault, runs use the machine's own `gh` login.
- **Signing in from the apps:** `/github add` in any thread opens the account form. **Sign in** uses
  GitHub's device flow: open github.com/login/device on any device and enter the code the form shows, then
  paste that same code back into the form. Or paste a token: `gh auth token` on a machine signed in as the
  account, or a fine-grained token from github.com/settings/tokens. Either way the token is sealed into
  the vault as `github.<label>` for your machines, and never posted.

The machine's own `gh` login is never touched: each sign-in runs in a config folder of its own, which goes
once the token is out, and `gh` isn't allowed to copy the code to the clipboard or open a browser on the
node. A sign-in nobody finishes is dropped after 15 minutes, as GitHub's device codes are.

GitHub's own pages open in the apps' web panel, signed in as you. The workspace's GitHub connection
(feeds of PRs and CI, `/github watch`, commit references that move tasks, link previews) is part of
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
