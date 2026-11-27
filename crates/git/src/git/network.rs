use super::*;

// Network commands: fetch, pull and push.
//
// They run under a cancellable `ReadScope::network` with no time budget, so a
// slow remote or a credential helper's sign-in window never hits the read
// deadline. `GIT_TERMINAL_PROMPT=0` (set for every command) keeps Git from
// waiting on a hidden terminal; credential helpers and SSH agents still work.

impl Git {
    /// Reads remotes, the upstream of `identity`'s branch and the last fetch
    /// time from local configuration and refs. Nothing contacts a remote.
    ///
    /// # Errors
    ///
    /// Returns an error if Git cannot list remotes or read the branch.
    pub fn remote_state(&self, identity: &WriteIdentity) -> GitResult<RemoteState> {
        std::thread::scope(|scope| {
            let remotes = scope.spawn(|| self.run(&["remote"]));
            let upstream = self.upstream_state(identity);
            let remotes = remotes.join().map_err(|_| "Git remote read failed")??;
            Ok((remotes, upstream?))
        })
        .and_then(|(remotes, upstream)| self.remote_state_from(&remotes, upstream))
    }

    fn upstream_state(&self, identity: &WriteIdentity) -> GitResult<Option<Upstream>> {
        Ok(if identity.symbolic_head.starts_with("refs/heads/") {
            let line = self.run(&[
                "for-each-ref",
                "--format=%(refname)%00%(upstream)%00%(upstream:short)%00%(upstream:remotename)%00%(upstream:remoteref)%00%(upstream:track,nobracket)",
                &identity.symbolic_head,
            ])?;
            line.lines().find_map(|line| {
                let fields: Vec<_> = line.split('\0').collect();
                if fields.len() != 6 || fields[0] != identity.symbolic_head || fields[1].is_empty()
                {
                    return None;
                }
                let (mut ahead, mut behind, mut gone) = (0, 0, false);
                for part in fields[5].split(',').map(str::trim) {
                    if let Some(count) = part.strip_prefix("ahead ") {
                        ahead = count.parse().unwrap_or(0);
                    } else if let Some(count) = part.strip_prefix("behind ") {
                        behind = count.parse().unwrap_or(0);
                    } else if part == "gone" {
                        gone = true;
                    }
                }
                Some(Upstream {
                    tracking: fields[1].into(),
                    name: fields[2].into(),
                    remote: fields[3].into(),
                    remote_ref: fields[4].into(),
                    ahead,
                    behind,
                    gone,
                })
            })
        } else {
            None
        })
    }

    fn remote_state_from(
        &self,
        remotes: &str,
        upstream: Option<Upstream>,
    ) -> GitResult<RemoteState> {
        let remotes = remotes
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        // FETCH_HEAD belongs to the worktree's own Git directory.
        let fetch_head = match &self.git_dir {
            Some(directory) => directory.join("FETCH_HEAD"),
            None => PathBuf::from(
                self.run(&[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "FETCH_HEAD",
                ])?
                .trim(),
            ),
        };
        let last_fetch = std::fs::metadata(fetch_head)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_secs());
        Ok(RemoteState {
            remotes,
            upstream,
            last_fetch,
        })
    }

    fn checked_out_for_network(identity: &WriteIdentity, action: &str) -> GitResult<String> {
        let Some(branch) = identity.symbolic_head.strip_prefix("refs/heads/") else {
            return Err(format!("HEAD is detached. Check out a branch to {action}."));
        };
        if identity.head.is_empty() {
            return Err(format!(
                "{branch} has no commits yet. Make the first commit before you {action}."
            ));
        }
        Ok(branch.to_owned())
    }

    /// Resolves what Pull would integrate: the checked-out branch's upstream.
    ///
    /// # Errors
    ///
    /// Returns a reason when HEAD is detached or unborn, or the branch has no
    /// upstream on a remote.
    pub fn pull_target(&self, identity: &WriteIdentity) -> GitResult<NetworkTarget> {
        let branch = Self::checked_out_for_network(identity, "pull")?;
        let state = self.remote_state(identity)?;
        let upstream = match &state.upstream {
            None => {
                return Err(format!(
                    "{branch} has no upstream branch. Push it to set one, or set one from a terminal with `git branch --set-upstream-to`."
                ))
            }
            Some(upstream) if upstream.remote == "." => {
                return Err(format!(
                    "{branch} tracks the local branch {}. Pull works with a branch on a remote; merge or rebase instead.",
                    upstream.name
                ))
            }
            Some(upstream) => upstream,
        };
        validate_revision(&upstream.remote)?;
        validate_revision(&upstream.remote_ref)?;
        Ok(NetworkTarget {
            branch,
            remote: upstream.remote.clone(),
            remote_ref: upstream.remote_ref.clone(),
            tracking: upstream.tracking.clone(),
            set_upstream: false,
            ahead: upstream.ahead,
            behind: upstream.behind,
        })
    }

    /// Resolves where Push would send the checked-out branch: its upstream,
    /// or the same-named branch on the only remote (or `origin`), set as the
    /// new upstream.
    ///
    /// # Errors
    ///
    /// Returns a reason when HEAD is detached or unborn, there is no remote,
    /// or the branch has no upstream and the remote is ambiguous.
    pub fn push_target(&self, identity: &WriteIdentity) -> GitResult<NetworkTarget> {
        let branch = Self::checked_out_for_network(identity, "push")?;
        let push_remote_key = format!("branch.{branch}.pushRemote");
        let (state, push_remote, default_push_remote, push_default) = std::thread::scope(|scope| {
            let push_remote = scope.spawn(|| self.config_value(&push_remote_key));
            let default_remote = scope.spawn(|| self.config_value("remote.pushDefault"));
            let push_default = scope.spawn(|| self.config_value("push.default"));
            let state = self.remote_state(identity);
            let failed = |_| "Git config read failed".to_owned();
            (
                state,
                push_remote.join().map_err(failed),
                default_remote.join().map_err(failed),
                push_default.join().map_err(failed),
            )
        });
        let state = state?;
        // Git's own order: the branch's push remote, then the default one.
        let push_remote = push_remote??.or(default_push_remote??);
        let push_default = push_default??
            .unwrap_or_else(|| "simple".into())
            .to_ascii_lowercase();
        if state.remotes.is_empty() && state.upstream.is_none() {
            return Err(
                "This repository has no remotes. Add one from a terminal with `git remote add`."
                    .into(),
            );
        }
        if push_default == "nothing" {
            return Err(format!(
                "push.default is set to nothing, so Git pushes only a branch you name. Push {branch} from a terminal with `git push <remote> {branch}`."
            ));
        }
        let follows_upstream = matches!(push_default.as_str(), "upstream" | "tracking");
        if let Some(upstream) = &state.upstream {
            if upstream.remote == "." {
                return Err(format!(
                    "{branch} tracks the local branch {}. Push it from a terminal to choose a remote.",
                    upstream.name
                ));
            }
            let remote = push_remote.as_deref().unwrap_or(&upstream.remote);
            if remote != upstream.remote {
                // A triangular workflow: pull from the upstream, push to
                // another remote. Git pushes to the same name there.
                if follows_upstream {
                    return Err(format!(
                        "{branch} pulls from {} but pushes to {remote}, and push.default is upstream, which Git refuses in that case. Push it from a terminal.",
                        upstream.remote
                    ));
                }
                return self.same_name_push(branch, remote, false);
            }
            validate_revision(&upstream.remote)?;
            validate_revision(&upstream.remote_ref)?;
            let own = format!("refs/heads/{branch}");
            if upstream.remote_ref != own && !follows_upstream {
                if push_default == "current" {
                    return self.same_name_push(branch, remote, false);
                }
                // Git's default (simple) refuses this push, so a branch
                // started from origin/main never overwrites main by accident.
                let name = upstream
                    .remote_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(&upstream.remote_ref);
                return Err(format!(
                    "{branch} tracks {}, a branch with a different name, so pushing would overwrite {name} on {remote}. Git refuses this push unless push.default is upstream, and so does GitCrab. To publish {branch} under its own name, run `git push -u {remote} {branch}` in a terminal. To update {name} on purpose, run `git push {remote} HEAD:{name}`.",
                    upstream.name
                ));
            }
            return Ok(NetworkTarget {
                branch,
                remote: upstream.remote.clone(),
                remote_ref: upstream.remote_ref.clone(),
                tracking: upstream.tracking.clone(),
                set_upstream: false,
                ahead: upstream.ahead,
                behind: upstream.behind,
            });
        }
        let remote = match push_remote.as_deref() {
            Some(remote) => remote,
            None => state.default_push_remote().ok_or_else(|| {
                format!("{branch} has no upstream and there are several remotes, none named origin. Push it once from a terminal with `git push -u <remote> {branch}`.")
            })?,
        };
        self.same_name_push(branch, remote, true)
    }

    /// Push of `branch` to the branch of the same name on `remote`.
    fn same_name_push(
        &self,
        branch: String,
        remote: &str,
        set_upstream: bool,
    ) -> GitResult<NetworkTarget> {
        validate_revision(remote)?;
        let tracking = format!("refs/remotes/{remote}/{branch}");
        // Commits the remote branch lacks, or the whole remote when it has no
        // such branch yet.
        let not_on_remote = if self.ref_exists(&tracking)? {
            tracking.clone()
        } else {
            format!("--remotes={remote}")
        };
        let ahead = self
            .run(&["rev-list", "--count", "HEAD", "--not", &not_on_remote, "--"])?
            .trim()
            .parse()
            .unwrap_or(0);
        Ok(NetworkTarget {
            remote_ref: format!("refs/heads/{branch}"),
            tracking,
            branch,
            remote: remote.to_owned(),
            set_upstream,
            ahead,
            behind: 0,
        })
    }

    /// A single configuration value, or `None` when it is not set.
    fn config_value(&self, key: &str) -> GitResult<Option<String>> {
        let output = self.output(&["config", "--get", key])?;
        match output.status.code() {
            Some(0) => String::from_utf8(output.stdout)
                .map(|value| Some(value.trim().to_owned()))
                .map_err(|e| e.to_string()),
            Some(1) => Ok(None),
            _ => Err(Self::failed_output(&output)),
        }
    }

    /// Runs a network command under `scope`, which can stop it and its helpers.
    /// Without a scope, Git runs to completion, for steps that change the
    /// branch, index or working tree and must not be stopped halfway.
    pub(super) fn network(
        &self,
        args: &[&str],
        scope: Option<&ReadScope>,
        action: &str,
    ) -> Result<std::process::Output, WriteError> {
        let mut command = self.command(args)?;
        // Never wait on an editor nobody can see. Merge messages use Git's default.
        command
            .env("GIT_EDITOR", ":")
            .env("GIT_SEQUENCE_EDITOR", ":")
            .env("GIT_MERGE_AUTOEDIT", "no");
        #[cfg(test)]
        if let Some(observe) = self.observe_command {
            observe(self, args)?;
        }
        capture(command, None, 0, scope).map_err(|error| {
            if error.kind() == std::io::ErrorKind::Interrupted {
                format!("{action} was cancelled. Git was stopped.").into()
            } else {
                WriteError::inspect(format!("Could not run Git for {action}: {error}"))
            }
        })
    }

    fn network_refs(&self) -> GitResult<std::collections::BTreeSet<String>> {
        Ok(self
            .run(&[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/remotes",
                "refs/tags",
            ])?
            .lines()
            // A remote's HEAD names its default branch; it is not news.
            .filter(|line| !line.ends_with("/HEAD"))
            .map(str::to_owned)
            .collect())
    }

    /// Fetches every remote with `--prune`, updating remote-tracking refs.
    /// Local branches, the index and the working tree are not changed.
    ///
    /// # Errors
    ///
    /// Returns a plain explanation for common authentication and connection
    /// failures, followed by Git's diagnostics, or a cancellation.
    pub(crate) fn fetch(&self, scope: &ReadScope) -> Result<String, WriteError> {
        let remotes = self.run(&["remote"])?.lines().count();
        if remotes == 0 {
            return Err(
                "This repository has no remotes. Add one from a terminal with `git remote add`."
                    .into(),
            );
        }
        let before = self.network_refs()?;
        let output = self.network(&["fetch", "--all", "--prune"], Some(scope), "Fetch")?;
        if !output.status.success() {
            let diagnostics = network_diagnostics(&output.stderr);
            let message = match network_explanation(&diagnostics, None) {
                Some(explanation) => format!("{explanation}\n\nGit said: {diagnostics}"),
                None => format!("Fetch failed. {diagnostics}"),
            };
            return Err(WriteError {
                kind: FailureKind::GitFailure,
                message,
            });
        }
        let after = self.network_refs().map_err(WriteError::inspect)?;
        let names = |refs: &std::collections::BTreeSet<String>| {
            refs.iter()
                .filter_map(|line| line.split_once(' ').map(|(_, name)| name.to_owned()))
                .collect::<std::collections::BTreeSet<_>>()
        };
        let (old_names, new_names) = (names(&before), names(&after));
        let pruned = old_names.difference(&new_names).count();
        let updated = after.difference(&before).count();
        let remotes = count_label(remotes, "remote", "remotes");
        Ok(if updated == 0 && pruned == 0 {
            format!("Fetched {remotes}, already up to date")
        } else {
            let mut message = format!(
                "Fetched {remotes}, {} updated",
                count_label(updated, "reference", "references")
            );
            if pruned > 0 {
                message.push_str(&format!(", {pruned} pruned"));
            }
            message
        })
    }

    /// Pulls the reviewed upstream into the checked-out branch.
    ///
    /// Fetches the upstream under `scope`, then merges or rebases the fetched
    /// commit without a scope, so Cancel stops only the network part and never
    /// leaves the branch half-updated. Returns HEAD before and after.
    ///
    /// # Errors
    ///
    /// Divergence with fast-forward only, local changes in the way, and
    /// authentication or connection failures are rejections with a plain
    /// explanation. Other Git failures, including conflicts, are Git failures.
    pub(crate) fn pull(
        &self,
        target: &NetworkTarget,
        mode: PullMode,
        scope: &ReadScope,
    ) -> Result<PullOutcome, WriteError> {
        validate_revision(&target.remote)?;
        validate_revision(&target.remote_ref)?;
        let before = self.write_identity()?;
        let failed = |output: &std::process::Output| {
            let diagnostics = network_diagnostics(&output.stderr);
            match network_explanation(&diagnostics, Some(target)) {
                Some(explanation) => WriteError {
                    kind: FailureKind::Rejected,
                    message: format!("{explanation}\n\nGit said: {diagnostics}"),
                },
                None => WriteError::git(format!("Pull failed. {diagnostics}")),
            }
        };
        // Pull runs as two steps, like `git pull` itself. Only the fetch waits
        // on the network, and it changes no local branch, index or file, so
        // Cancel can stop it at any moment. Stopping Git while it merges or
        // rebases could leave a stale index.lock or a half-finished rebase,
        // so the second step always runs to completion.
        let fetch = ["fetch", target.remote.as_str(), target.remote_ref.as_str()];
        let output = match self.network(&fetch, Some(scope), "Pull") {
            Ok(output) => output,
            Err(_) if scope.check().is_err() => {
                return Err(
                    "Pull was cancelled while fetching. Git was stopped before it changed the branch."
                        .into(),
                );
            }
            Err(error) => return Err(error),
        };
        if !output.status.success() {
            return Err(failed(&output));
        }
        // FETCH_HEAD names exactly what this fetch brought; the remote-tracking
        // ref was updated along with it.
        let fetched = self
            .resolve_commit("FETCH_HEAD")
            .map_err(WriteError::inspect)?;
        if scope.check().is_err() {
            return Err(format!(
                "Pull was cancelled after fetching {}. {} was not changed.",
                target.remote_branch(),
                target.branch
            )
            .into());
        }
        let message = format!("Merge remote-tracking branch '{}'", target.remote_branch());
        let fork_point;
        let integrate: Vec<&str> = match mode {
            PullMode::FastForwardIfPossible => {
                vec!["merge", "--ff", "--no-edit", "-m", &message, &fetched]
            }
            PullMode::FastForwardOnly => vec!["merge", "--ff-only", &fetched],
            PullMode::Rebase => {
                // Like `git pull --rebase`, skip commits the upstream already
                // had before it was rewritten, found from its reflog.
                fork_point = self
                    .output(&["merge-base", "--fork-point", &target.tracking, "HEAD"])
                    .ok()
                    .filter(|output| output.status.success())
                    .and_then(|output| String::from_utf8(output.stdout).ok())
                    .map(|id| id.trim().to_owned())
                    .filter(|id| !id.is_empty());
                match &fork_point {
                    Some(base) => vec!["rebase", "--onto", &fetched, base],
                    None => vec!["rebase", &fetched],
                }
            }
        };
        let output = self.network(&integrate, None, "Pull")?;
        if !output.status.success() {
            return Err(failed(&output));
        }
        let after = self.resolve_commit("HEAD").map_err(WriteError::inspect)?;
        if self.symbolic_head().map_err(WriteError::inspect)? != before.symbolic_head {
            return Err(WriteError::inspect(
                "Pull finished but HEAD no longer names the reviewed branch.",
            ));
        }
        let from = target.remote_branch();
        let message = if after == before.head {
            format!("{} is already up to date with {from}.", target.branch)
        } else {
            let range = format!("{}..{after}", before.head);
            let added = self
                .run(&["rev-list", "--count", &range, "--"])
                .ok()
                .and_then(|count| count.trim().parse().ok())
                .unwrap_or(0);
            format!(
                "Pulled {from} into {} ({}), {} → {}, {}",
                target.branch,
                match mode {
                    PullMode::Rebase => "rebased",
                    // A fast-forward leaves the branch exactly at the fetched tip.
                    _ if fetched == after => "fast-forward",
                    _ => "merged",
                },
                &before.head[..before.head.len().min(8)],
                &after[..after.len().min(8)],
                count_label(added, "new commit", "new commits"),
            )
        };
        Ok(PullOutcome {
            before: before.head,
            after,
            message,
        })
    }

    /// Pushes the checked-out branch to the reviewed remote branch without
    /// force, setting the upstream when the review said so.
    ///
    /// # Errors
    ///
    /// A non-fast-forward rejection asks the user to pull first. Other common
    /// failures have a plain explanation followed by Git's diagnostics.
    pub(crate) fn push(
        &self,
        target: &NetworkTarget,
        scope: &ReadScope,
    ) -> Result<String, WriteError> {
        validate_revision(&target.remote)?;
        validate_revision(&target.remote_ref)?;
        validate_revision(&target.branch)?;
        let refspec = format!("refs/heads/{}:{}", target.branch, target.remote_ref);
        let mut args = vec!["push"];
        if target.set_upstream {
            args.push("--set-upstream");
        }
        args.extend([target.remote.as_str(), refspec.as_str()]);
        let output = self.network(&args, Some(scope), "Push")?;
        let diagnostics = network_diagnostics(&output.stderr);
        if !output.status.success() {
            return Err(WriteError {
                kind: FailureKind::Rejected,
                message: match network_explanation(&diagnostics, Some(target)) {
                    Some(explanation) => format!("{explanation}\n\nGit said: {diagnostics}"),
                    None => format!("Push failed. {diagnostics}"),
                },
            });
        }
        let to = target.remote_branch();
        let mut message = if diagnostics.contains("Everything up-to-date") {
            format!("{to} already has every commit on {}.", target.branch)
        } else {
            format!(
                "Pushed {} to {to}, {}",
                target.branch,
                count_label(target.ahead, "commit", "commits")
            )
        };
        if target.set_upstream {
            message.push_str(&format!(". {to} is now the upstream of {}", target.branch));
        }
        Ok(message)
    }
}

#[cfg(test)]
pub(crate) mod network_tests {
    use super::*;
    use crate::test_repo::Fixture;
    use std::sync::{atomic::AtomicBool, Arc};

    pub(crate) fn scope() -> ReadScope {
        ReadScope::network(Arc::new(AtomicBool::new(false)))
    }

    pub(crate) fn commit(fixture: &Fixture, name: &str, text: &str) -> String {
        fixture.write(name, text);
        fixture.git.run(&["add", "--", name]).unwrap();
        fixture.git.run(&["commit", "--quiet", "-m", name]).unwrap();
        fixture.git.resolve_commit("HEAD").unwrap()
    }

    /// A worktree whose `main` tracks `origin/main` in a bare remote, and a
    /// second clone of that remote for writing someone else's commits.
    pub(crate) fn connected() -> (Fixture, Fixture) {
        let local = Fixture::new();
        commit(&local, "base.txt", "base\n");
        let remote = local.bare_remote("remote.git");
        let remote = remote.to_str().unwrap();
        local.git.run(&["remote", "add", "origin", remote]).unwrap();
        local
            .git
            .run(&["push", "--quiet", "--set-upstream", "origin", "main"])
            .unwrap();
        let other = Fixture::new();
        other.git.run(&["remote", "add", "origin", remote]).unwrap();
        other
            .git
            .run(&["pull", "--quiet", "origin", "main"])
            .unwrap();
        other
            .git
            .run(&["branch", "--set-upstream-to=origin/main"])
            .unwrap();
        (local, other)
    }

    /// Commits in `other` and publishes it, so `local` is behind.
    pub(crate) fn publish(other: &Fixture, name: &str) -> String {
        let id = commit(other, name, &format!("{name}\n"));
        other.git.run(&["push", "--quiet"]).unwrap();
        id
    }

    fn state(fixture: &Fixture) -> RemoteState {
        let identity = fixture.git.write_identity().unwrap();
        fixture.git.remote_state(&identity).unwrap()
    }

    fn pull(fixture: &Fixture, mode: PullMode) -> Result<PullOutcome, WriteError> {
        let identity = fixture.git.write_identity().unwrap();
        let target = fixture.git.pull_target(&identity).unwrap();
        fixture.git.pull(&target, mode, &scope())
    }

    fn push_target(fixture: &Fixture) -> NetworkTarget {
        let identity = fixture.git.write_identity().unwrap();
        fixture.git.push_target(&identity).unwrap()
    }

    #[test]
    fn remote_state_reads_remotes_upstream_and_counts_locally() {
        let lonely = Fixture::new();
        commit(&lonely, "a.txt", "a\n");
        let alone = state(&lonely);
        assert!(alone.remotes.is_empty() && alone.upstream.is_none());
        assert_eq!(alone.default_push_remote(), None);
        let identity = lonely.git.write_identity().unwrap();
        assert!(lonely
            .git
            .push_target(&identity)
            .unwrap_err()
            .contains("no remotes"));
        assert!(lonely
            .git
            .pull_target(&identity)
            .unwrap_err()
            .contains("no upstream"));

        let (local, _other) = connected();
        commit(&local, "mine.txt", "mine\n");
        let state = state(&local);
        assert_eq!(state.remotes, ["origin"]);
        let upstream = state.network_upstream().unwrap();
        assert_eq!(upstream.name, "origin/main");
        assert_eq!(upstream.remote, "origin");
        assert_eq!(upstream.remote_ref, "refs/heads/main");
        assert_eq!(upstream.tracking, "refs/remotes/origin/main");
        assert_eq!(
            (upstream.ahead, upstream.behind, upstream.gone),
            (1, 0, false)
        );
        local.git.run(&["switch", "--quiet", "--detach"]).unwrap();
        let identity = local.git.write_identity().unwrap();
        assert!(local
            .git
            .pull_target(&identity)
            .unwrap_err()
            .starts_with("HEAD is detached"));
        assert!(local
            .git
            .remote_state(&identity)
            .unwrap()
            .upstream
            .is_none());
    }

    #[test]
    fn fetch_updates_tracking_refs_and_records_the_fetch_time() {
        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        assert_eq!(state(&local).upstream.unwrap().behind, 0);
        let message = local.git.fetch(&scope()).unwrap();
        assert!(message.contains("1 reference updated"), "{message}");
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            theirs
        );
        let state = state(&local);
        assert_eq!(state.upstream.unwrap().behind, 1);
        assert!(state.last_fetch.is_some());
        assert!(local
            .git
            .fetch(&scope())
            .unwrap()
            .ends_with("already up to date"));
    }

    #[test]
    fn fast_forward_pull_moves_the_branch_to_the_upstream() {
        let (local, other) = connected();
        let before = local.git.resolve_commit("HEAD").unwrap();
        let theirs = publish(&other, "theirs.txt");
        let outcome = pull(&local, PullMode::FastForwardOnly).unwrap();
        assert_eq!(outcome.before, before);
        assert_eq!(outcome.after, theirs);
        assert!(
            outcome.message.contains("fast-forward"),
            "{}",
            outcome.message
        );
        assert_eq!(local.git.resolve_commit("HEAD").unwrap(), theirs);
        let again = pull(&local, PullMode::FastForwardOnly).unwrap();
        assert!(again.message.contains("already up to date"));
        assert_eq!(again.before, again.after);
    }

    #[test]
    fn fast_forward_only_refuses_a_diverged_branch_with_a_reason() {
        let (local, other) = connected();
        publish(&other, "theirs.txt");
        let mine = commit(&local, "mine.txt", "mine\n");
        let error = pull(&local, PullMode::FastForwardOnly).unwrap_err();
        assert_eq!(error.kind, FailureKind::Rejected);
        assert!(error.message.contains("have diverged"), "{}", error.message);
        assert!(!error.message.contains("hint:"));
        assert_eq!(local.git.resolve_commit("HEAD").unwrap(), mine);
    }

    #[test]
    fn diverged_pulls_merge_or_rebase() {
        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        let mine = commit(&local, "mine.txt", "mine\n");
        let merged = pull(&local, PullMode::FastForwardIfPossible).unwrap();
        assert!(merged.message.contains("merged"), "{}", merged.message);
        let parents = local
            .git
            .run(&["show", "-s", "--format=%P", "HEAD"])
            .unwrap();
        assert_eq!(
            parents.split_whitespace().collect::<Vec<_>>(),
            [mine.as_str(), theirs.as_str()]
        );

        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        commit(&local, "mine.txt", "mine\n");
        let rebased = pull(&local, PullMode::Rebase).unwrap();
        assert!(rebased.message.contains("rebased"), "{}", rebased.message);
        assert_eq!(local.git.resolve_commit("HEAD^").unwrap(), theirs);
        assert_eq!(local.git.symbolic_head().unwrap(), "refs/heads/main");
        assert!(local.git.operation_markers().unwrap().is_empty());
    }

    #[test]
    fn push_updates_the_upstream_or_sets_one() {
        let (local, _other) = connected();
        let mine = commit(&local, "mine.txt", "mine\n");
        let target = push_target(&local);
        assert!(!target.set_upstream);
        assert_eq!(target.ahead, 1);
        assert_eq!(target.remote_branch(), "origin/main");
        let message = local.git.push(&target, &scope()).unwrap();
        assert_eq!(message, "Pushed main to origin/main, 1 commit");
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            mine
        );

        local
            .git
            .run(&["switch", "--quiet", "-c", "topic"])
            .unwrap();
        let topic = commit(&local, "topic.txt", "topic\n");
        let target = push_target(&local);
        assert!(target.set_upstream);
        assert_eq!(target.remote, "origin");
        assert_eq!(target.remote_ref, "refs/heads/topic");
        assert_eq!(target.ahead, 1);
        let message = local.git.push(&target, &scope()).unwrap();
        assert!(
            message.ends_with("origin/topic is now the upstream of topic"),
            "{message}"
        );
        let upstream = state(&local).upstream.unwrap();
        assert_eq!(
            (upstream.name.as_str(), upstream.ahead),
            ("origin/topic", 0)
        );
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/topic")
                .unwrap(),
            topic
        );
    }

    #[test]
    fn push_without_upstream_needs_one_remote_or_origin() {
        let (local, _other) = connected();
        local
            .git
            .run(&["switch", "--quiet", "-c", "topic"])
            .unwrap();
        local
            .git
            .run(&["remote", "add", "backup", "file:///nowhere"])
            .unwrap();
        assert_eq!(push_target(&local).remote, "origin");
        local
            .git
            .run(&["remote", "rename", "origin", "upstream"])
            .unwrap();
        let identity = local.git.write_identity().unwrap();
        assert!(local
            .git
            .push_target(&identity)
            .unwrap_err()
            .contains("several remotes"));
    }

    #[test]
    fn rejected_push_suggests_pulling_and_never_forces() {
        let (local, other) = connected();
        let theirs = publish(&other, "theirs.txt");
        commit(&local, "mine.txt", "mine\n");
        let error = local.git.push(&push_target(&local), &scope()).unwrap_err();
        assert_eq!(error.kind, FailureKind::Rejected);
        assert!(
            error.message.starts_with("Push rejected"),
            "{}",
            error.message
        );
        assert!(error.message.contains("Pull first"));
        other.git.run(&["fetch", "--quiet"]).unwrap();
        assert_eq!(
            other
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            theirs
        );
    }

    #[test]
    fn common_failures_have_plain_explanations() {
        let target = NetworkTarget {
            branch: "main".into(),
            remote: "origin".into(),
            remote_ref: "refs/heads/main".into(),
            ..Default::default()
        };
        for (stderr, expected) in [
            (
                "fatal: Authentication failed for 'https://example.invalid/'",
                "Authentication failed for origin",
            ),
            (
                "fatal: could not read Username for 'https://x': terminal prompts disabled",
                "Authentication failed",
            ),
            (
                "git@example.invalid: Permission denied (publickey).",
                "refused the SSH key",
            ),
            (
                "fatal: unable to access 'https://x/': Could not resolve host: x",
                "Could not resolve the host",
            ),
            (
                "ssh: Could not resolve hostname x: No such host is known.",
                "Could not resolve the host",
            ),
            (
                " ! [rejected]        main -> main (non-fast-forward)",
                "Push rejected",
            ),
            (
                "fatal: couldn't find remote ref refs/heads/main",
                "no longer exists",
            ),
            (
                "error: Your local changes to the following files would be overwritten by merge:",
                "Local changes would be overwritten",
            ),
        ] {
            let explanation = network_explanation(stderr, Some(&target)).unwrap();
            assert!(explanation.contains(expected), "{stderr} -> {explanation}");
        }
        assert_eq!(network_explanation("fatal: something new", None), None);
    }

    #[test]
    fn cancelling_a_network_command_stops_git_and_its_helpers() {
        let fixture = Fixture::new();
        commit(&fixture, "a.txt", "a\n");
        // An SSH transport that never answers, run through Git's shell.
        for args in [
            &["config", "core.sshCommand", "sleep 30; exit 1;"][..],
            &["remote", "add", "slow", "ssh://example.invalid/repo.git"],
        ] {
            fixture.git.run(args).unwrap();
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let scope = ReadScope::network(cancelled.clone());
        let start = std::time::Instant::now();
        let error = std::thread::scope(|threads| {
            threads.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(500));
                cancelled.store(true, std::sync::atomic::Ordering::Release);
            });
            fixture.git.fetch(&scope).unwrap_err()
        });
        assert_eq!(error.kind, FailureKind::Rejected, "{}", error.message);
        assert_eq!(error.message, "Fetch was cancelled. Git was stopped.");
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn push_refuses_an_upstream_with_a_different_name_like_git() {
        let (local, _other) = connected();
        let published = local
            .git
            .resolve_commit("refs/remotes/origin/main")
            .unwrap();
        // What `git switch -c feature origin/main` sets up by default.
        local
            .git
            .run(&[
                "switch",
                "--quiet",
                "-c",
                "feature",
                "--track",
                "origin/main",
            ])
            .unwrap();
        commit(&local, "feature.txt", "feature\n");
        let identity = local.git.write_identity().unwrap();
        let error = local.git.push_target(&identity).unwrap_err();
        assert!(error.contains("a branch with a different name"), "{error}");
        assert!(error.contains("git push -u origin feature"), "{error}");
        local.git.run(&["fetch", "--quiet"]).unwrap();
        assert_eq!(
            local
                .git
                .resolve_commit("refs/remotes/origin/main")
                .unwrap(),
            published
        );

        // push.default=current publishes the branch under its own name.
        local
            .git
            .run(&["config", "push.default", "current"])
            .unwrap();
        let target = push_target(&local);
        assert_eq!(target.remote_ref, "refs/heads/feature");
        assert!(!target.set_upstream);
        assert_eq!(target.ahead, 1);

        // push.default=upstream asks for exactly this push.
        local
            .git
            .run(&["config", "push.default", "upstream"])
            .unwrap();
        let target = push_target(&local);
        assert_eq!(target.remote_branch(), "origin/main");
        local.git.push(&target, &scope()).unwrap();

        local
            .git
            .run(&["config", "push.default", "nothing"])
            .unwrap();
        let identity = local.git.write_identity().unwrap();
        assert!(local
            .git
            .push_target(&identity)
            .unwrap_err()
            .contains("push.default is set to nothing"));
    }

    #[test]
    fn push_goes_to_the_configured_push_remote() {
        let (local, _other) = connected();
        let fork = local.bare_remote("fork.git");
        local
            .git
            .run(&["remote", "add", "fork", fork.to_str().unwrap()])
            .unwrap();
        commit(&local, "mine.txt", "mine\n");
        local
            .git
            .run(&["config", "remote.pushDefault", "fork"])
            .unwrap();
        let target = push_target(&local);
        assert_eq!(
            (target.remote.as_str(), target.remote_ref.as_str()),
            ("fork", "refs/heads/main")
        );
        assert!(!target.set_upstream);
        // The branch's own push remote wins over the default.
        local
            .git
            .run(&["config", "branch.main.pushRemote", "origin"])
            .unwrap();
        assert_eq!(push_target(&local).remote_branch(), "origin/main");
    }

    static CANCEL_BEFORE_FETCH: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    static CANCEL_DURING_MERGE: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();

    #[test]
    fn cancel_stops_a_pull_only_while_it_fetches() {
        let (local, other) = connected();
        let before = local.git.resolve_commit("HEAD").unwrap();
        let theirs = publish(&other, "theirs.txt");
        let identity = local.git.write_identity().unwrap();
        let target = local.git.pull_target(&identity).unwrap();

        let cancelled = CANCEL_BEFORE_FETCH.get_or_init(Default::default).clone();
        let mut git = local.git.clone();
        git.observe_command = Some(|_, args| {
            if args.first() == Some(&"fetch") {
                CANCEL_BEFORE_FETCH
                    .get()
                    .unwrap()
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(())
        });
        let error = git
            .pull(
                &target,
                PullMode::FastForwardOnly,
                &ReadScope::network(cancelled),
            )
            .unwrap_err();
        assert!(
            error.message.contains("cancelled while fetching"),
            "{}",
            error.message
        );
        assert_eq!(local.git.resolve_commit("HEAD").unwrap(), before);

        // Cancel arriving once Git merges is too late to stop it halfway.
        let cancelled = CANCEL_DURING_MERGE.get_or_init(Default::default).clone();
        git.observe_command = Some(|_, args| {
            if args.first() == Some(&"merge") {
                CANCEL_DURING_MERGE
                    .get()
                    .unwrap()
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            Ok(())
        });
        let outcome = git
            .pull(
                &target,
                PullMode::FastForwardOnly,
                &ReadScope::network(cancelled),
            )
            .unwrap();
        assert_eq!(outcome.after, theirs);
        assert_eq!(local.git.resolve_commit("HEAD").unwrap(), theirs);
        assert!(local.git.operation_markers().unwrap().is_empty());
    }

    #[test]
    fn a_stash_drop_that_hits_the_wrong_entry_puts_it_back() {
        let fixture = Fixture::new();
        let git = &fixture.git;
        commit(&fixture, "tracked.txt", "base\n");
        fixture.write("tracked.txt", "reviewed\n");
        git.stash_with_message("reviewed").unwrap();
        let reviewed = git.resolve_commit("refs/stash").unwrap();
        fixture.write("tracked.txt", "older\n");
        git.stash_with_message("other").unwrap();
        let other = git.resolve_commit("refs/stash").unwrap();
        let before = git.stash_entries().unwrap();
        let index = before.iter().position(|(id, _)| *id == reviewed).unwrap();
        // Another client stores an entry after the list was read, so the
        // reviewed position now names a different entry.
        // (Git writes no entry for an object already at the top of the list.)
        git.write(&["stash", "store", "-m", "concurrent", &reviewed])
            .unwrap();
        let error = git.drop_stash_entry(&before, index).unwrap_err();
        assert_eq!(error.kind, FailureKind::Inspect, "{}", error.message);
        assert!(
            error.message.contains("put that entry back"),
            "{}",
            error.message
        );
        let after = git.stash_entries().unwrap();
        // Nothing was lost: both originals and the concurrent entry remain.
        assert_eq!(after.len(), 3, "{after:?}");
        assert!(after.iter().any(|entry| *entry == before[0]));
        assert_eq!(after.iter().filter(|(id, _)| *id == reviewed).count(), 2);
        // Dropping by object ID with an up-to-date list removes exactly one.
        git.drop_stash_object(&reviewed).unwrap();
        let after = git.stash_entries().unwrap();
        assert_eq!(after.iter().filter(|(id, _)| *id == reviewed).count(), 1);
        assert_eq!(after.iter().filter(|(id, _)| *id == other).count(), 1);
    }

    #[test]
    fn stash_refuses_to_report_success_when_nothing_was_saved() {
        let fixture = Fixture::new();
        commit(&fixture, "tracked.txt", "base\n");
        fixture.write("untracked.txt", "new\n");
        let error = fixture.git.stash_with_message("nothing").unwrap_err();
        assert_eq!(error.message, NOTHING_TO_STASH);
        assert!(fixture.git.stash_entries().unwrap().is_empty());
    }

    #[test]
    fn git_messages_stay_in_english_without_changing_the_character_set() {
        let fixture = Fixture::new();
        let command = fixture.git.command(&["status"]).unwrap();
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&(
            std::ffi::OsStr::new("LC_MESSAGES"),
            Some(std::ffi::OsStr::new("C"))
        )));
        assert!(envs.contains(&(std::ffi::OsStr::new("LANGUAGE"), None)));
    }
}
