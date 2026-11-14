use super::*;

// Network commands: fetch, pull and push.
//
// They run under a cancellable `ReadScope::network` with no time budget, so a
// slow remote or a credential helper's sign-in window never hits the read
// deadline. `GIT_TERMINAL_PROMPT=0` (set for every command) keeps Git from
// waiting on a hidden terminal; credential helpers and SSH agents still work.

/// How Pull integrates the upstream branch into the checked-out branch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PullMode {
    /// Fast-forward when possible, otherwise create a merge commit.
    #[default]
    FastForwardIfPossible,
    /// Fast-forward only. A diverged branch is left unchanged.
    FastForwardOnly,
    /// Rebase local commits onto the upstream.
    Rebase,
}

impl PullMode {
    /// Modes in menu order. The first is the toolbar default.
    pub const ALL: [Self; 3] = [
        Self::FastForwardIfPossible,
        Self::FastForwardOnly,
        Self::Rebase,
    ];

    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Self::FastForwardIfPossible => "Pull (fast-forward if possible)",
            Self::FastForwardOnly => "Pull (fast-forward only)",
            Self::Rebase => "Pull (rebase)",
        }
    }

    /// How the dialog describes the integration.
    pub fn explain(self) -> &'static str {
        match self {
            Self::FastForwardIfPossible => {
                "fast-forward if possible, otherwise create a merge commit"
            }
            Self::FastForwardOnly => "fast-forward only; if the branches diverged nothing changes",
            Self::Rebase => "rebase local commits onto it",
        }
    }
}

/// The checked-out branch's upstream, read from local configuration and refs.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Upstream {
    /// Short name such as `origin/main`.
    pub name: String,
    /// Configured remote, or `.` for a local branch.
    pub remote: String,
    /// Branch ref on the remote, such as `refs/heads/main`.
    pub remote_ref: String,
    /// Local remote-tracking ref, such as `refs/remotes/origin/main`.
    pub tracking: String,
    pub ahead: usize,
    pub behind: usize,
    /// The tracking ref is missing: never fetched, or pruned after the remote deleted it.
    pub gone: bool,
}

/// Remotes and upstream tracking, from local configuration and refs only.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteState {
    /// Configured remote names.
    pub remotes: Vec<String>,
    pub upstream: Option<Upstream>,
    /// When this worktree's `FETCH_HEAD` was last written, in seconds since
    /// the Unix epoch. Fetches and pulls from any client update it.
    pub last_fetch: Option<u64>,
}

impl RemoteState {
    /// An upstream on a remote, as Pull and Push use it. A branch that tracks
    /// another local branch has none.
    pub fn network_upstream(&self) -> Option<&Upstream> {
        self.upstream
            .as_ref()
            .filter(|upstream| upstream.remote != ".")
    }

    /// The remote Push sets as upstream when the branch has none: the only
    /// remote, otherwise `origin`.
    pub fn default_push_remote(&self) -> Option<&str> {
        match self.remotes.as_slice() {
            [only] => Some(only),
            remotes => remotes
                .iter()
                .find(|remote| *remote == "origin")
                .map(String::as_str),
        }
    }

    /// Status text for the upstream relationship.
    pub fn context(&self, identity: &WriteIdentity) -> String {
        if identity.symbolic_head.is_empty() {
            return "Detached HEAD · no upstream".into();
        }
        match &self.upstream {
            None => "No upstream configured".into(),
            Some(upstream) if upstream.gone => format!("Upstream {} unavailable", upstream.name),
            Some(upstream) => format!(
                "{} ahead · {} behind · {}",
                upstream.ahead, upstream.behind, upstream.name
            ),
        }
    }
}

/// The local branch, remote and refs reviewed for Pull or Push.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkTarget {
    /// Checked-out local branch name.
    pub branch: String,
    pub remote: String,
    /// Branch ref on the remote, such as `refs/heads/main`.
    pub remote_ref: String,
    /// Local remote-tracking ref, such as `refs/remotes/origin/main`.
    pub tracking: String,
    /// Push sets this remote branch as the upstream (`git push -u`).
    pub set_upstream: bool,
    /// Local commits the remote branch does not have, as last fetched.
    pub ahead: usize,
    /// Remote commits the local branch does not have, as last fetched.
    pub behind: usize,
}

impl NetworkTarget {
    /// The remote branch as people name it, such as `origin/main`.
    pub fn remote_branch(&self) -> String {
        format!(
            "{}/{}",
            self.remote,
            self.remote_ref
                .strip_prefix("refs/heads/")
                .unwrap_or(&self.remote_ref)
        )
    }
}

/// HEAD before and after a successful pull.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullOutcome {
    pub before: String,
    pub after: String,
    pub message: String,
}

pub fn count_label(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Maps a failed network command's diagnostics to a plain explanation.
pub fn network_explanation(stderr: &str, target: Option<&NetworkTarget>) -> Option<String> {
    let text = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    let remote = target.map_or("the remote".to_owned(), |t| t.remote.clone());
    let remote_branch = target.map_or("the remote branch".to_owned(), |t| t.remote_branch());
    let branch = target.map_or("the branch".to_owned(), |t| t.branch.clone());
    Some(
        if has(&["[rejected]"]) && has(&["non-fast-forward", "fetch first"]) {
            format!("Push rejected: {remote_branch} has commits that {branch} does not have. Pull first, then push again. GitCrab never force-pushes.")
        } else if has(&["[remote rejected]"]) {
            format!("{remote} refused the push, for example because the branch is protected or a server hook declined it.")
        } else if has(&["not possible to fast-forward"]) {
            format!("{branch} and {remote_branch} have diverged, so a fast-forward is not possible. Nothing was changed. Choose Pull (fast-forward if possible) to merge, or Pull (rebase).")
        } else if has(&["permission denied (publickey"]) {
            format!("{remote} refused the SSH key (permission denied, publickey). Add your key to the SSH agent or check your access to the repository, then retry.")
        } else if has(&["host key verification failed"]) {
            "The SSH host key could not be verified. Connect once from a terminal to review and accept the host key, then retry.".into()
        } else if has(&[
            "terminal prompts disabled",
            "could not read username",
            "could not read password",
            "authentication failed",
            "invalid username or password",
            "http basic: access denied",
            "returned error: 401",
            "returned error: 403",
        ]) {
            format!("Authentication failed for {remote}. GitCrab does not ask for passwords itself: sign in through your Git credential helper (for example Git Credential Manager) or load your SSH key into the agent, then retry.")
        } else if has(&[
            "could not resolve host",
            "could not resolve hostname",
            "name or service not known",
            "no such host is known",
            "temporary failure in name resolution",
        ]) {
            format!("Could not resolve the host for {remote}. Check your network connection and the remote URL.")
        } else if has(&[
            "connection refused",
            "connection timed out",
            "operation timed out",
            "failed to connect",
            "network is unreachable",
            "could not connect",
        ]) {
            format!("Could not connect to {remote}. Check your network connection and try again.")
        } else if has(&[
            "repository not found",
            "does not appear to be a git repository",
        ]) {
            format!("{remote} was not found, or you do not have access to it. Check the remote URL and your permissions.")
        } else if has(&["couldn't find remote ref"]) {
            format!("{remote_branch} no longer exists on {remote}. Fetch to prune it, then choose another upstream from a terminal.")
        } else if has(&["no tracking information", "has no upstream branch"]) {
            format!("{branch} has no upstream branch.")
        } else if has(&[
            "would be overwritten by",
            "cannot pull with rebase",
            "please commit or stash them",
            "you have unstaged changes",
            "your index contains uncommitted changes",
        ]) {
            "Local changes would be overwritten. Commit or stash them, then pull again. Nothing was changed.".into()
        } else {
            return None;
        },
    )
}

/// Git's diagnostics without its advice lines.
pub fn network_diagnostics(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| !line.starts_with("hint:") && !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
