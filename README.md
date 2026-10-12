# github

GitHub on Pointman's machines (James, Pointman thread 649 [4443], [4500], [4505]): GitHub's CLI on each
node, installed where there's none, and GitHub accounts signed in from the apps and given to agents' runs.

It's the machine half of GitHub on Pointman. Pointman's GitHub App stays in core, for what GitHub sends as
it happens: PR and CI feeds, `/github watch`, commit refs moving tasks, link pills and releases (James,
[4670]–[4673]). Core's catalogue lets this repo alone take the id `github`, and the apps show the two as one
GitHub: `/github watch` is core's, `/github add` this extension's.

GitHub's own pages open in the apps' web panel, signed in as you (James, Pointman thread 8446). Until 0.6.0
this extension also had a viewer of its own (`github.view` and a `view` pane of PR and issue pages, diffs
and buttons); that's gone, since GitHub's page does all of it.

- **Agents' runs** get the vault's account as `GH_TOKEN` (`[sessions]`), so `gh` works for them on every
  node; with none in the vault, they use the machine's own `gh` login.
- **`/github add`** opens the account form in the apps: Sign in (GitHub's device flow) or paste a token,
  sealed into the vault as `github.<label>`.
- If `gh` isn't on the node's PATH, or in /opt/homebrew/bin or /usr/local/bin, the extension installs its
  own; or set its path in the extension's `cli` setting.

```
pointman add github
```

## Building

`cargo test` runs it against a stand-in node and a stand-in `gh`; `cargo test -- --ignored` starts a real
sign-in with the machine's `gh`. A release is a tag that's extension.toml's version (`git tag v0.1.0 && git
push origin v0.1.0`): each platform is built and attached, and core's catalogue takes it from there.
