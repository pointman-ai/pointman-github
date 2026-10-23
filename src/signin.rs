//! Signing a GitHub account in from the apps (James, Pointman thread 649 [4500]: sign in through the apps,
//! the token kept in the vault for every node). GitHub's own device flow through `gh auth login --web`: the
//! person opens github.com/login/device anywhere and enters the code the form shows. That's all they do: the
//! sign-in `waits` (EXT-69; James, thread 17421 [19057]), so the node asks `credentials.wait`, which answers once
//! GitHub has said yes. A node from before that asks for the code back instead (`credentials.code`, MB-216's
//! paste-back step), which still works: the same code, pasted.
//!
//! - **This machine's own gh login is never touched:** each sign-in has a config folder of its own, the
//!   token kept in that folder's file rather than the Keychain (`--insecure-storage`), and the folder goes
//!   once the token is out. gh gets no PATH, so it doesn't copy the code to this machine's clipboard or
//!   open a browser here.
//! - **The token** comes only out of `wait` or `code`, as the vault item's value, for the node to seal into
//!   the vault. Nothing here writes it anywhere else, logs it or keeps it.
//! - **A sign-in waits** 15 minutes, as GitHub's device codes do, then it's dropped.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use pointman_extension::protocol::CredentialsSignInResponse;
use regex::Regex;
use tempfile::TempDir;

pub const WAIT: Duration = Duration::from_secs(15 * 60);

struct Session {
    child: Child,
    config: TempDir,
    said: Arc<Mutex<String>>,
    code: String,
    started: Instant,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The sign-ins waiting for their code, by the node's session id.
#[derive(Default)]
pub struct SignIns {
    open: Mutex<HashMap<String, Session>>,
}

fn device_code() -> Regex {
    Regex::new(r"\b([A-Z0-9]{4}-[A-Z0-9]{4})\b").expect("compiles")
}

impl SignIns {
    fn sweep(&self) {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, s| s.started.elapsed() < WAIT);
    }

    /// Start one: GitHub's link and the code to enter there.
    pub fn start(
        &self,
        gh: &std::path::Path,
        session_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<CredentialsSignInResponse> {
        if session_id.is_empty() {
            bail!("a sign-in needs the node's session");
        }
        self.sweep();
        let config = tempfile::Builder::new().prefix("pointman-github-signin-").tempdir()?;
        let mut child = Command::new(gh)
            .args(["auth", "login", "--web", "--hostname", "github.com", "--git-protocol", "https"])
            .args(["--skip-ssh-key", "--insecure-storage"])
            .env_clear()
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("GH_CONFIG_DIR", config.path())
            .env("GH_BROWSER", "true")
            .env("BROWSER", "true")
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("couldn't run {}", gh.display()))?;
        let said = Arc::new(Mutex::new(String::new()));
        for mut out in [
            child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>),
            child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let said = said.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    said.lock().unwrap_or_else(|e| e.into_inner()).push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            });
        }
        let end = Instant::now() + timeout;
        let code = loop {
            let text = said.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(m) = device_code().captures(&text) {
                break m[1].to_string();
            }
            if Instant::now() > end || child.try_wait()?.is_some() {
                let _ = child.kill();
                let _ = child.wait();
                let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("nothing").trim().to_string();
                bail!("gh didn't give a sign-in code (it said: {last})");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let url = "https://github.com/login/device";
        let prompt = format!("Enter {code} at {url}, signed in as the GitHub account to add");
        self.open.lock().unwrap_or_else(|e| e.into_inner()).insert(
            session_id.to_string(),
            Session { child, config, said, code: code.clone(), started: Instant::now() },
        );
        Ok(CredentialsSignInResponse {
            url: url.to_string(),
            prompt: Some(prompt),
            waits: true,
            code: Some(code),
            ..Default::default()
        })
    }

    /// The node waits for this sign-in (EXT-69): gh's token once GitHub says yes, with nothing pasted.
    pub fn wait(&self, gh: &std::path::Path, session_id: &str, timeout: Duration) -> anyhow::Result<String> {
        let s = self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id)
            .ok_or_else(|| anyhow!("this sign-in has ended: start it again"))?;
        finish(gh, s, timeout)
    }

    /// The person entered the code at GitHub and pasted it here: gh's token once GitHub says yes.
    pub fn code(
        &self,
        gh: &std::path::Path,
        session_id: &str,
        code: &str,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let s = self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id)
            .ok_or_else(|| anyhow!("this sign-in has ended: start it again"))?;
        if !code.trim().eq_ignore_ascii_case(&s.code) {
            let again = s.code.clone();
            self.open.lock().unwrap_or_else(|e| e.into_inner()).insert(session_id.to_string(), s);
            bail!("that isn't this sign-in's code: paste {again}, the one you entered at GitHub");
        }
        finish(gh, s, timeout)
    }
}

/// gh's token once its device flow ends with GitHub's yes: the vault item's value, as the form's `value` matches.
/// Its folder, with the token's file, goes with it.
fn finish(gh: &std::path::Path, mut s: Session, timeout: Duration) -> anyhow::Result<String> {
    {
        let end = Instant::now() + timeout;
        let status = loop {
            if let Some(st) = s.child.try_wait()? {
                break st;
            }
            if Instant::now() > end {
                bail!("GitHub hasn't said yes yet: enter {} at github.com/login/device, then start again", s.code);
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if !status.success() {
            let text = s.said.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
            bail!("GitHub didn't sign it in ({last})");
        }
        let out = Command::new(gh)
            .args(["auth", "token", "--hostname", "github.com"])
            .env_clear()
            .env("HOME", std::env::var_os("HOME").unwrap_or_default())
            .env("GH_CONFIG_DIR", s.config.path())
            .stdin(Stdio::null())
            .output()?;
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() || !crate::is_token(&token) {
            bail!("gh signed in but didn't give a token");
        }
        drop(s); // its folder, with the token's file, goes now
        Ok(token)
    }
}
