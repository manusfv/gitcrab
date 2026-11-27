use super::*;
use crate::test_repo::Fixture;

#[test]
fn predicted_status_matches_git_for_common_changes() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    for name in ["modified", "deleted", "staged", "partly", "staged-delete"] {
        fixture.write(name, "base\n");
    }
    git.run(&["add", "."]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    fixture.write("modified", "edit\n");
    std::fs::remove_file(git.path.join("deleted")).unwrap();
    fixture.write("staged", "edit\n");
    fixture.write("partly", "edit\n");
    fixture.write("added", "new\n");
    fixture.write("added-edited", "new\n");
    git.run(&["add", "staged", "partly", "added", "added-edited"])
        .unwrap();
    git.run(&["rm", "-q", "staged-delete"]).unwrap();
    fixture.write("partly", "edit again\n");
    fixture.write("added-edited", "newer\n");
    fixture.write("untracked", "new\n");

    let mut checked = 0;
    for unstage in [false, true] {
        for file in git.status().unwrap() {
            if !file.on_side(unstage) {
                continue;
            }
            let Some(predicted) = file.predicted(unstage) else {
                panic!("no prediction for {file:?}");
            };
            // Apply the real change and compare. Staging only rewrites the
            // index file, so restoring its bytes puts the state back.
            let index = git.path.join(".git/index");
            let saved = std::fs::read(&index).unwrap();
            git.stage_reviewed(&file, unstage).unwrap();
            let actual = git
                .status()
                .unwrap()
                .into_iter()
                .find(|f| f.path == file.path);
            let verb = if unstage { "unstage" } else { "stage" };
            assert_eq!(predicted, actual, "{verb} {file:?}");
            checked += 1;
            std::fs::write(&index, saved).unwrap();
        }
    }
    assert!(checked >= 10, "only {checked} cases checked");
}

#[test]
fn index_listing_is_reused_only_while_the_index_is_unchanged() {
    thread_local!(static LISTINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });
    fn count(_: &Git, args: &[&str]) -> GitResult<()> {
        if args.starts_with(&["ls-files", "--stage"]) {
            LISTINGS.with(|n| n.set(n.get() + 1));
        }
        Ok(())
    }
    let mut fixture = Fixture::new();
    fixture.write("a.txt", "one\n");
    fixture.git.run(&["add", "a.txt"]).unwrap();
    fixture.git.observe_command = Some(count);
    let git = &fixture.git;

    let first = git.write_identity().unwrap();
    assert_eq!(git.write_identity().unwrap(), first);
    assert_eq!(
        LISTINGS.with(|n| n.get()),
        1,
        "unchanged index is not listed again"
    );

    fixture.write("a.txt", "two\n");
    git.run(&["add", "a.txt"]).unwrap();
    let second = git.write_identity().unwrap();
    assert_ne!(second.index, first.index);
    assert_eq!(
        LISTINGS.with(|n| n.get()),
        2,
        "a changed index is listed again"
    );
    assert_eq!(
        second.index,
        git.bytes(&["ls-files", "--stage", "-z"]).unwrap()
    );
}

#[test]
fn short_ref_names_match_git_including_ambiguous_names() {
    let fixture = Fixture::new();
    fixture.history(2, 0);
    let git = &fixture.git;
    let tip = git.run(&["rev-parse", "HEAD"]).unwrap();
    let tip = tip.trim();
    // A branch and a tag with one name, a branch shaped like a remote
    // branch, a branch under `tags/`, a remote HEAD symref and a stash.
    let refs = [
        "refs/heads/v1",
        "refs/tags/v1",
        "refs/tags/main",
        "refs/heads/origin/topic",
        "refs/remotes/origin/topic",
        "refs/remotes/origin/main",
        "refs/heads/tags/x",
        "refs/tags/x",
        "refs/heads/FETCH_HEAD",
        "refs/heads/plain",
        "refs/notes/commits",
    ]
    .iter()
    .map(|name| format!("update {name} {tip}\n"))
    .collect::<String>();
    git.fixture_input(&["update-ref", "--stdin"], refs.as_bytes())
        .unwrap();
    git.run(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ])
    .unwrap();
    std::fs::write(git.path.join(".git/FETCH_HEAD"), format!("{tip}\n")).unwrap();

    let expected = git
        .run(&["for-each-ref", "--format=%(refname)%00%(refname:short)"])
        .unwrap();
    let (full, short): (Vec<&str>, Vec<&str>) = expected
        .lines()
        .map(|line| line.split_once('\0').unwrap())
        .unzip();
    // The fixture really exercises the ambiguity rules.
    for name in ["heads/v1", "tags/v1", "origin", "heads/FETCH_HEAD"] {
        assert!(
            short.contains(&name),
            "Git did not shorten to {name}: {short:?}"
        );
    }
    assert_eq!(git.short_ref_names(&full), short);
}

#[test]
fn history_search_matches_reference_only_with_normalized_whitespace() {
    let commit = Commit {
        subject: "Implement feature".into(),
        refs: vec!["Release/October".into()],
        ..Default::default()
    }
    .index_search();
    assert!(commit.matches_search(&normalize_search("  RELEASE/october  ")));
    assert!(!commit.matches_search(&normalize_search("november")));
}

#[test]
fn body_preview_joins_lines_and_caps_length() {
    assert_eq!(body_preview(""), "");
    assert_eq!(
        body_preview("First line\n\n  Second\tline\n\nCo-Authored-By: A <a@b>\n"),
        "First line Second line Co-Authored-By: A <a@b>"
    );
    let long = body_preview(&"é ".repeat(500));
    assert_eq!(long.chars().count(), 201);
    assert!(long.ends_with('…'));
}

#[test]
#[ignore = "repeatable history search timing fixture"]
fn benchmark_history_search() {
    let commits: Vec<_> = (0..20_000)
        .map(|i| Commit {
            subject: format!("Add fixture feature number {i}"),
            author: "Fixture Author".into(),
            id: format!("{i:040x}"),
            refs: vec![format!("feature/group-{}", i % 30)],
            ..Default::default()
        })
        .collect();
    let index_start = std::time::Instant::now();
    let indexed: Vec<_> = commits.iter().cloned().map(Commit::index_search).collect();
    let index_ms = index_start.elapsed().as_secs_f64() * 1000.0;
    let mut before = vec![];
    let mut after = vec![];
    for i in 0..11 {
        let query = normalize_search(&format!(" FEATURE/GROUP-{i} "));
        let start = std::time::Instant::now();
        let old = commits
            .iter()
            .filter(|c| {
                format!("{} {} {} {}", c.subject, c.author, c.id, c.refs.join(" "))
                    .to_lowercase()
                    .contains(&query)
            })
            .count();
        before.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = std::time::Instant::now();
        let new = indexed.iter().filter(|c| c.matches_search(&query)).count();
        after.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(old, new);
    }
    before.sort_by(f64::total_cmp);
    after.sort_by(f64::total_cmp);
    println!("20,000 commits: median search {:.2} ms before, {:.2} ms indexed. One-time index build (including cloned fixture) {index_ms:.2} ms.", before[5], after[5]);
}

#[test]
fn canonical_tag_identity_is_separate_from_a_same_named_branch() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    git.run(&["commit", "--allow-empty", "-m", "tagged"])
        .unwrap();
    let tagged = git.resolve_commit("HEAD").unwrap();
    git.run(&["tag", "-a", "release", "-m", "release"]).unwrap();
    git.run(&["commit", "--allow-empty", "-m", "branch tip"])
        .unwrap();
    let tip = git.resolve_commit("HEAD").unwrap();
    git.create_branch("release").unwrap();
    let snapshot = git.snapshot(100).unwrap();
    assert_eq!(
        snapshot.loaded_ref_tip(ReferenceKind::Tag, "refs/tags/release"),
        Some(tagged.as_str())
    );
    assert_eq!(
        snapshot.loaded_ref_tip(ReferenceKind::LocalBranch, "release"),
        Some(tip.as_str())
    );
}

// APFS rejects file names that are not valid UTF-8 (EILSEQ), so macOS cannot
// create these files. Linux filesystems store the raw bytes. History with such
// names is covered on every platform by the test after this one.
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn non_utf8_paths_are_distinct_disabled_rows_in_status_and_commit_details() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let git = &fixture.git;
    for raw in [b"name-\xff.txt", b"name-\xfe.txt"] {
        std::fs::write(git.path.join(std::ffi::OsStr::from_bytes(raw)), b"data\n").unwrap();
    }
    let status = git.status().unwrap();
    assert_eq!(status.len(), 2);
    assert_ne!(status[0].path, status[1].path);
    assert!(status.iter().all(|entry| entry.path_issue().is_some()));
    assert!(git.stage_reviewed(&status[0], false).is_err());
    assert!(git.write_identity().unwrap().index.is_empty());
    git.stage_all(false).unwrap();
    git.commit("non-Unicode paths").unwrap();
    let details = git.changed_files("HEAD").unwrap();
    assert_eq!(details.len(), 2);
    assert!(details.iter().all(|entry| entry.path_issue().is_some()));
}

/// A repository cloned from Linux can carry names that are not UTF-8 in its
/// history even where the filesystem cannot hold them, as on macOS. Building
/// the commit through the index keeps those names off the disk.
#[test]
fn non_utf8_paths_in_history_are_distinct_disabled_rows() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("data.txt", "data\n");
    let blob = git.run(&["hash-object", "-w", "data.txt"]).unwrap();
    std::fs::remove_file(git.path.join("data.txt")).unwrap();
    let mut entries = Vec::new();
    for raw in [b"name-\xff.txt", b"name-\xfe.txt"] {
        entries.extend_from_slice(format!("100644 {}\t", blob.trim()).as_bytes());
        entries.extend_from_slice(raw);
        entries.push(0);
    }
    let cmd = git
        .command(&["update-index", "-z", "--index-info"])
        .unwrap();
    let output = capture(cmd, Some(&entries), 0, None).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    git.commit("non-Unicode paths").unwrap();
    let details = git.changed_files("HEAD").unwrap();
    assert_eq!(details.len(), 2);
    assert_ne!(details[0].path, details[1].path);
    assert!(details.iter().all(|entry| entry.path_issue().is_some()));
}

#[test]
fn gitlink_preview_pins_short_output_and_rejects_partial_updates() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    git.run(&["commit", "--allow-empty", "-m", "old target"])
        .unwrap();
    let old = git.resolve_commit("HEAD").unwrap();
    git.run(&["commit", "--allow-empty", "-m", "new target"])
        .unwrap();
    let new = git.resolve_commit("HEAD").unwrap();
    git.run(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{old},module"),
    ])
    .unwrap();
    git.commit("old gitlink").unwrap();
    git.run(&[
        "update-index",
        "--cacheinfo",
        &format!("160000,{new},module"),
    ])
    .unwrap();
    git.run(&["config", "diff.submodule", "diff"]).unwrap();
    let hunks = git.hunks("module", true).unwrap();
    assert_eq!(hunks.len(), 1);
    assert!(hunks[0]
        .patch
        .contains(&format!("+Subproject commit {new}")));
    let reviewed = git.write_identity().unwrap();
    assert!(git.apply_hunk(&hunks[0], true).is_err());
    assert_eq!(git.write_identity().unwrap(), reviewed);
    assert!(git.run(&["ls-files", "-s"]).unwrap().starts_with("160000 "));
}

#[test]
fn capture_preserves_exit_status_when_git_closes_large_stdin_early() {
    let fixture = Fixture::new();
    let cmd = fixture
        .git
        .command(&["gitcrab-no-such-subcommand"])
        .unwrap();
    let result = capture(cmd, Some(&vec![b'x'; 512 * 1024]), 0, None).unwrap();
    assert!(!result.status.success());
    assert!(!result.stderr.is_empty());
}
#[test]
fn reviewed_commit_rejects_index_head_hooks_and_active_markers() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("one.txt", "one\n");
    git.stage("one.txt").unwrap();
    let unborn = git.write_identity().unwrap();
    let sha = git.commit_reviewed("first", &unborn).unwrap();
    assert_eq!(sha, git.run(&["rev-parse", "HEAD"]).unwrap().trim());
    fixture.write("one.txt", "two\n");
    git.stage("one.txt").unwrap();
    let reviewed = git.write_identity().unwrap();
    fixture.write("extra.txt", "extra\n");
    git.stage("extra.txt").unwrap();
    assert!(git
        .commit_reviewed("stale index", &reviewed)
        .unwrap_err()
        .message
        .contains("changed"));
    let reviewed = git.write_identity().unwrap();
    git.run(&["commit", "--allow-empty", "-m", "external"])
        .unwrap();
    assert!(git.commit_reviewed("stale HEAD", &reviewed).is_err());
    fixture.write("one.txt", "three\n");
    git.stage("one.txt").unwrap();
    let reviewed = git.write_identity().unwrap();
    fixture.hook(
        "pre-commit",
        "#!/bin/sh\necho controlled-hook-failure >&2\nexit 1\n",
    );
    let failed = git.commit_reviewed("hook fails", &reviewed).unwrap_err();
    assert_eq!(failed.kind, FailureKind::GitFailure);
    assert!(
        failed.message.contains("controlled-hook-failure"),
        "{failed:?}"
    );
    assert_eq!(git.write_identity().unwrap(), reviewed);
    fixture.hook("pre-commit", "#!/bin/sh\nexit 0\n");
    let marker = git.run(&["rev-parse", "--git-path", "MERGE_HEAD"]).unwrap();
    std::fs::write(git.path.join(marker.trim()), format!("{}\n", reviewed.head)).unwrap();
    let snapshot = git.snapshot(100).unwrap();
    assert!(snapshot.commit_blocked());
    assert!(!snapshot.status.iter().any(|f| f.index == 'U'));
    assert!(git
        .commit_reviewed("ordinary commit", &reviewed)
        .unwrap_err()
        .message
        .contains("active Git operation"));
}
#[test]
fn commit_with_hook_modified_index_requires_inspection_and_keeps_actual_commit() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "base\n");
    git.stage("a.txt").unwrap();
    git.commit("base").unwrap();
    fixture.write("a.txt", "reviewed\n");
    git.stage("a.txt").unwrap();
    let reviewed = git.write_identity().unwrap();
    fixture.hook(
        "pre-commit",
        "#!/bin/sh\nprintf 'hook edit\\n' > a.txt\ngit add -- a.txt\n",
    );
    let failure = git
        .commit_reviewed("changed by hook", &reviewed)
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Inspect);
    let after = git.write_identity().unwrap();
    assert_ne!(after.head, reviewed.head);
    assert_ne!(after.index, reviewed.index);
    assert_eq!(git.resolve_commit("HEAD^").unwrap(), reviewed.head);
    assert_eq!(git.run(&["show", "HEAD:a.txt"]).unwrap(), "hook edit\n");
    assert_eq!(git.run(&["show", ":a.txt"]).unwrap(), "hook edit\n");
    assert_eq!(
        std::fs::read(git.path.join("a.txt")).unwrap(),
        b"hook edit\n"
    );
}

#[test]
fn reviewed_commit_verifies_a_signed_commit_with_signature_display_enabled() {
    let executable = if cfg!(windows) {
        "ssh-keygen.exe"
    } else {
        "ssh-keygen"
    };
    let Some(ssh_keygen) = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join(executable))
            .find(|path| path.is_file())
    }) else {
        eprintln!("Skipping SSH signing test because ssh-keygen is unavailable");
        return;
    };
    let fixture = Fixture::new();
    let git = &fixture.git;
    let key = git.path.join(".git/signing-key");
    let output = std::process::Command::new(&ssh_keygen)
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let public_key = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    fixture.write(
        ".git/allowed-signers",
        &format!("test@example.invalid {public_key}"),
    );
    fixture.write("a.txt", "base\n");
    git.stage("a.txt").unwrap();
    git.commit("base").unwrap();
    for (setting, value) in [
        ("gpg.format", "ssh"),
        ("gpg.ssh.program", ssh_keygen.to_str().unwrap()),
        ("gpg.ssh.allowedSignersFile", ".git/allowed-signers"),
        ("user.signingKey", key.to_str().unwrap()),
        ("commit.gpgSign", "true"),
        ("log.showSignature", "true"),
    ] {
        git.run(&["config", setting, value]).unwrap();
    }
    fixture.write("a.txt", "reviewed\n");
    git.stage("a.txt").unwrap();
    let reviewed = git.write_identity().unwrap();
    let sha = git.commit_reviewed("signed change", &reviewed).unwrap();
    assert_eq!(git.write_identity().unwrap().head, sha);
    let display = git.run(&["show", "-s", "--format=%P%x00%T", &sha]).unwrap();
    assert!(display.lines().count() > 1);
}

#[test]
fn failed_read_after_a_successful_commit_requires_inspection() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "reviewed\n");
    git.stage("a.txt").unwrap();
    let reviewed = git.write_identity().unwrap();
    fixture.hook(
        "post-commit",
        "#!/bin/sh\ngit symbolic-ref HEAD refs/heads/empty\n",
    );
    let failure = git
        .commit_reviewed("committed on main", &reviewed)
        .unwrap_err();
    assert_eq!(failure.kind, FailureKind::Inspect);
    let after = git.write_identity().unwrap();
    assert_eq!(after.symbolic_head, "refs/heads/empty");
    assert!(after.head.is_empty());
    assert_eq!(after.index, reviewed.index);
    assert_eq!(
        git.run(&["show", "refs/heads/main:a.txt"]).unwrap(),
        "reviewed\n"
    );
    assert_eq!(
        std::fs::read(git.path.join("a.txt")).unwrap(),
        b"reviewed\n"
    );
}

#[test]
fn discard_only_reviewed_hunk_preserves_index_head_and_other_hunk() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let original = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
    fixture.write("日本語.txt", &original);
    git.stage(".").unwrap();
    git.commit("base").unwrap();
    let staged = original.replace("line 1\n", "staged one\n");
    fixture.write("日本語.txt", &staged);
    git.stage(".").unwrap();
    let working = staged
        .replace("line 10\n", "work ten\n")
        .replace("line 30\n", "work thirty\n");
    fixture.write("日本語.txt", &working);
    let identity = git.write_identity().unwrap();
    let hunks = git.hunks("日本語.txt", false).unwrap();
    assert_eq!(hunks.len(), 2);
    git.discard_hunk("日本語.txt", &hunks[0]).unwrap();
    assert_eq!(git.write_identity().unwrap(), identity);
    let expected = staged.replace("line 30\n", "work thirty\n");
    assert_eq!(
        std::fs::read(git.path.join("日本語.txt")).unwrap(),
        expected.as_bytes()
    );
    assert!(git.discard_hunk("日本語.txt", &hunks[0]).is_err());
    fixture.write("new.txt", "untracked\n");
    let added = git.hunks("new.txt", false).unwrap();
    assert!(git.discard_hunk("new.txt", &added[0]).is_err());
}
#[test]
fn selected_lines_stage_unstage_and_discard_independently() {
    use crate::hunk_edit::{select_lines, Keep};
    let fixture = Fixture::new();
    let git = &fixture.git;
    let original = (0..20).map(|i| format!("line {i}\n")).collect::<String>();
    fixture.write("f.txt", &original);
    git.stage(".").unwrap();
    git.commit("base").unwrap();
    // One hunk: two replacements next to each other and an insertion.
    let working = original
        .replace("line 5\n", "five\n")
        .replace("line 6\n", "six\n")
        .replace("line 7\n", "line 7\nadded\n");
    fixture.write("f.txt", &working);
    let display = |staged| {
        let patch = git.diff(None, "f.txt", staged).unwrap();
        let lines: Vec<String> = patch
            .lines()
            .skip_while(|l| !l.starts_with("@@ "))
            .map(str::to_owned)
            .collect();
        (patch, lines)
    };
    let row = |lines: &[String], text: &str| lines.iter().position(|l| l == text).unwrap();
    let (patch, lines) = display(false);
    assert_eq!(lines.iter().filter(|l| l.starts_with("@@ ")).count(), 1);
    // Stage only "line 5" -> "five".
    let rows = [row(&lines, "-line 5"), row(&lines, "+five")];
    git.apply_hunk(&select_lines(&patch, &rows, Keep::Before).unwrap(), false)
        .unwrap();
    let index = git.run(&["show", ":f.txt"]).unwrap();
    assert_eq!(index, original.replace("line 5\n", "five\n"));
    // Stage the insertion alone, then unstage only the replacement.
    let (patch, lines) = display(false);
    let rows = [row(&lines, "+added")];
    git.apply_hunk(&select_lines(&patch, &rows, Keep::Before).unwrap(), false)
        .unwrap();
    let (patch, lines) = display(true);
    let rows = [row(&lines, "-line 5"), row(&lines, "+five")];
    git.apply_hunk(&select_lines(&patch, &rows, Keep::After).unwrap(), true)
        .unwrap();
    let index = git.run(&["show", ":f.txt"]).unwrap();
    assert_eq!(index, original.replace("line 7\n", "line 7\nadded\n"));
    // Discard only "line 6" -> "six" from the working file.
    let (patch, lines) = display(false);
    let rows = [row(&lines, "-line 6"), row(&lines, "+six")];
    git.discard_hunk("f.txt", &select_lines(&patch, &rows, Keep::After).unwrap())
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(git.path.join("f.txt")).unwrap(),
        working.replace("six\n", "line 6\n")
    );
    assert_eq!(git.run(&["show", ":f.txt"]).unwrap(), index);
    // Part of an untracked file can be staged, line by line.
    fixture.write("new.txt", "a\nb\nc\n");
    let (patch, lines) = {
        let patch = git.diff(None, "new.txt", false).unwrap();
        let lines: Vec<String> = patch
            .lines()
            .skip_while(|l| !l.starts_with("@@ "))
            .map(str::to_owned)
            .collect();
        (patch, lines)
    };
    let rows = [row(&lines, "+b")];
    git.apply_hunk(&select_lines(&patch, &rows, Keep::Before).unwrap(), false)
        .unwrap();
    assert_eq!(git.run(&["show", ":new.txt"]).unwrap(), "b\n");
}
#[test]
fn unique_path_counts_and_linked_worktree_markers_stay_local() {
    let snapshot = RepoSnapshot {
        status: vec![
            FileChange {
                path: "new".into(),
                index: 'R',
                worktree: 'M',
                original_path: Some("old".into()),
                unsupported_path: false,
            },
            FileChange {
                path: "new".into(),
                index: 'M',
                worktree: 'M',
                original_path: None,
                unsupported_path: false,
            },
        ],
        ..Default::default()
    };
    assert_eq!(snapshot.unique_changes().len(), 1);
    assert_eq!(snapshot.change_counts(), [0, 0, 0, 1]);
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a", "a");
    git.stage(".").unwrap();
    git.commit("root").unwrap();
    let linked_path = git.path.join("linked");
    git.run(&[
        "worktree",
        "add",
        "-b",
        "linked",
        linked_path.to_str().unwrap(),
    ])
    .unwrap();
    let linked = Git::open(&linked_path).unwrap();
    let marker = linked
        .run(&["rev-parse", "--git-path", "CHERRY_PICK_HEAD"])
        .unwrap();
    let marker = PathBuf::from(marker.trim());
    std::fs::write(
        if marker.is_absolute() {
            marker
        } else {
            linked.path.join(marker)
        },
        git.run(&["rev-parse", "HEAD"]).unwrap(),
    )
    .unwrap();
    assert_eq!(linked.operation_markers().unwrap(), ["cherry-pick"]);
    assert!(git.operation_markers().unwrap().is_empty());
}
#[test]
fn rename_comparisons_keep_paths_and_blobs_in_the_correct_namespace() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let old = "legacy 日本語 [1].txt";
    let new = "archive 日本語 [2].txt";
    let text = (0..30)
        .map(|n| format!("stable line {n}\n"))
        .collect::<String>();
    fixture.write(old, &text);
    git.stage(old).unwrap();
    git.commit("root").unwrap();
    let before = git.run(&["rev-parse", &format!("HEAD:{old}")]).unwrap();
    git.run(&["mv", "--", old, new]).unwrap();
    let staged = git.diff(None, new, true).unwrap();
    assert!(staged.contains("rename from "));
    assert!(!staged.contains("@@"));
    assert_eq!(before, git.run(&["rev-parse", &format!(":{new}")]).unwrap());
    git.commit("pure rename").unwrap();
    let id = git.run(&["rev-parse", "HEAD"]).unwrap();
    let files = git.changed_files(id.trim()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].original_path.as_deref(), Some(old));
    assert_eq!(files[0].path, new);
    let committed = git.diff(Some(id.trim()), new, false).unwrap();
    assert!(committed.contains("similarity index 100%"));
    assert!(!committed.contains("@@"));
    // An unstaged edit after a staged rename compares against the new
    // index path, while the staged comparison still uses HEAD's old path.
    git.run(&["mv", "--", new, old]).unwrap();
    fixture.write(old, &text.replace("stable line 15", "edited line 15"));
    let working = git.diff(None, old, false).unwrap();
    assert!(working.contains("-stable line 15"));
    assert!(working.contains("+edited line 15"));
    assert!(!working.contains("rename from"));
    git.stage(old).unwrap();
    let edited = git.diff(None, old, true).unwrap();
    assert!(edited.contains("rename from"));
    assert!(edited.contains("-stable line 15"));
    git.commit("rename with edit").unwrap();
    let id = git.run(&["rev-parse", "HEAD"]).unwrap();
    let mut expected_args = patch_args("diff");
    expected_args.extend(["--find-renames", "HEAD^1", "HEAD", "--", new, old]);
    assert_eq!(
        git.diff(Some(id.trim()), old, false).unwrap(),
        git.run(&expected_args).unwrap()
    );
    assert_eq!(
        git.run(&["show", &format!("HEAD:{old}")]).unwrap(),
        text.replace("stable line 15", "edited line 15")
    );
}

#[test]
fn real_git_create_branch_at_keeps_head_and_index() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("file", "one");
    git.stage("file").unwrap();
    git.commit("first").unwrap();
    let first = git.run(&["rev-parse", "HEAD"]).unwrap();
    fixture.write("file", "two");
    git.stage("file").unwrap();
    git.commit("second").unwrap();
    let head = git.run(&["rev-parse", "HEAD"]).unwrap();
    let tree = git.run(&["write-tree"]).unwrap();
    git.create_branch_at("feature/日本語", first.trim())
        .unwrap();
    assert_eq!(git.run(&["rev-parse", "feature/日本語"]).unwrap(), first);
    assert_eq!(git.run(&["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(git.run(&["write-tree"]).unwrap(), tree);
    assert!(git.create_branch_at("--bad", first.trim()).is_err());
}
#[test]
fn real_git_unicode_paths_and_branch_names() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let name = "notes 日本語 [draft].txt";
    fixture.write(name, "First version\n");
    git.stage(name).unwrap();
    git.commit("Unicode path").unwrap();
    git.create_branch("feature/日本語").unwrap();
    git.checkout("feature/日本語").unwrap();
    fixture.write(name, "Second version\n");
    let state = git.snapshot(10).unwrap();
    assert_eq!(state.head, "feature/日本語");
    assert_eq!(state.status[0].path, name);
    assert!(git
        .diff(None, name, false)
        .unwrap()
        .contains("+Second version"));
    let nested = git.path.join("folder with spaces 日本語");
    std::fs::create_dir(&nested).unwrap();
    assert_eq!(
        Git::open(nested).unwrap().path.canonicalize().unwrap(),
        git.path.canonicalize().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn untracked_symlink_preview_cannot_leave_repository() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    outside.write("outside.txt", "outside repository\n");
    std::os::unix::fs::symlink(
        outside.git.path.join("outside.txt"),
        fixture.git.path.join("link.txt"),
    )
    .unwrap();
    let error = fixture.git.diff(None, "link.txt", false).unwrap_err();
    assert!(error.contains("leaves the repository"));
    assert_eq!(
        std::fs::read_to_string(outside.git.path.join("outside.txt")).unwrap(),
        "outside repository\n"
    );
}

#[test]
fn status_keeps_rename_destination_and_literal_names() {
    let rows = parse_status(b"R  new name\0old name\0 M [literal]*.rs\0?? line\nbreak\0");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].path, "new name");
    assert_eq!(rows[0].original_path.as_deref(), Some("old name"));
    assert_eq!(rows[1].path, "[literal]*.rs");
    assert_eq!(rows[2].path, "line\nbreak");
}
#[test]
fn split_patch_keeps_context_and_headers() {
    let patches = split_hunks("diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n@@ -20 +20 @@\n-before\n+after\n");
    assert_eq!(patches.len(), 2);
    assert!(patches[1].patch.contains("--- a/a\n+++ b/a\n@@ -20"));
    assert!(!patches[1].patch.contains("+new"));
}
#[test]
fn real_git_unborn_literal_paths_and_independent_hunks() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    assert!(git.snapshot(100).unwrap().commits.is_empty());
    let original: String = (0..40).map(|i| format!("line {i}\n")).collect();
    fixture.write("[literal].txt", &original);
    assert!(git
        .diff(None, "[literal].txt", false)
        .unwrap()
        .contains("+line 2"));
    git.stage("[literal].txt").unwrap();
    assert_eq!(git.snapshot(100).unwrap().status[0].index, 'A');
    git.unstage("[literal].txt").unwrap();
    assert_eq!(git.snapshot(100).unwrap().status[0].index, '?');
    git.stage("[literal].txt").unwrap();
    git.commit("Initial content").unwrap();
    fixture.write(
        "[literal].txt",
        &original
            .replace("line 2\n", "first edit\n")
            .replace("line 32\n", "second edit\n"),
    );
    let hunks = git.hunks("[literal].txt", false).unwrap();
    assert_eq!(hunks.len(), 2);
    git.apply_hunk(&hunks[0], false).unwrap();
    let cached = git.diff(None, "[literal].txt", true).unwrap();
    assert!(cached.contains("+first edit"));
    assert!(!cached.contains("+second edit"));
    let remaining = git.diff(None, "[literal].txt", false).unwrap();
    assert!(remaining.contains("+second edit"));
    assert!(!remaining.contains("+first edit"));
    git.apply_hunk(&git.hunks("[literal].txt", true).unwrap()[0], true)
        .unwrap();
    assert!(git.diff(None, "[literal].txt", true).unwrap().is_empty());
    assert_eq!(git.hunks("[literal].txt", false).unwrap().len(), 2);
}
#[test]
fn real_git_merge_refs_root_diff_and_detached_head() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("root.txt", "root\n");
    git.stage("root.txt").unwrap();
    git.commit("Root").unwrap();
    let root = git.snapshot(100).unwrap().commits[0].id.clone();
    assert_eq!(
        git.changed_files(&root)
            .unwrap()
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["root.txt"]
    );
    assert!(git
        .diff(Some(&root), "root.txt", false)
        .unwrap()
        .contains("+root"));
    git.create_branch("feature").unwrap();
    git.checkout("feature").unwrap();
    fixture.write("feature.txt", "feature\n");
    git.stage("feature.txt").unwrap();
    git.commit("Feature").unwrap();
    git.checkout("main").unwrap();
    fixture.write("main.txt", "main\n");
    git.stage("main.txt").unwrap();
    git.commit("Main").unwrap();
    git.merge("feature").unwrap();
    git.run(&["tag", "-a", "v1", "-m", "Version one"]).unwrap();
    let state = git.snapshot(100).unwrap();
    assert_eq!(state.commits[0].parents.len(), 2);
    assert!(state.commits[0].refs.contains(&"v1".into()));
    assert!(state.commits[0].refs.contains(&"HEAD".into()));
    assert_eq!(
        git.changed_files(&state.commits[0].id)
            .unwrap()
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["feature.txt"]
    );
    git.checkout(&root).unwrap();
    assert!(git.run(&["symbolic-ref", "--quiet", "HEAD"]).is_err());
    let detached = git.snapshot(100).unwrap();
    assert_eq!(detached.checked_out_branch(), None);
    assert!(detached
        .commits
        .iter()
        .any(|c| c.id == root && c.refs.contains(&"HEAD".into())));
}
#[test]
fn checked_out_branch_is_the_primary_decoration_among_shared_refs() {
    use gitcrab_core::presentation::{resolve_decorations, DecorationKind};
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("file", "base\n");
    git.stage("file").unwrap();
    git.commit("Base").unwrap();
    git.run(&["branch", "feature/a"]).unwrap();
    git.run(&["branch", "feature/b"]).unwrap();
    git.run(&["tag", "v1"]).unwrap();
    let state = git.snapshot(100).unwrap();
    let branch = state.checked_out_branch().unwrap().to_owned();
    let decorations = resolve_decorations(&state, &state.commits[0].refs);
    let labels: Vec<_> = decorations.iter().map(|d| d.label()).collect();
    assert_eq!(decorations[0].kind, DecorationKind::HeadBranch);
    assert_eq!(decorations[0].name, branch);
    assert_eq!(
        labels,
        [
            format!("{branch} (checked out)"),
            "Branch feature/a".into(),
            "Branch feature/b".into(),
            "Tag v1".into(),
        ]
    );
}
#[test]
fn real_git_cherry_pick_revert_stash_and_conflict_feedback() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("shared.txt", "original\n");
    git.stage("shared.txt").unwrap();
    git.commit("Root").unwrap();
    git.create_branch("feature").unwrap();
    git.checkout("feature").unwrap();
    fixture.write("feature.txt", "feature\n");
    git.stage("feature.txt").unwrap();
    git.commit("Feature addition").unwrap();
    let feature = git.run(&["rev-parse", "HEAD"]).unwrap().trim().to_string();
    git.checkout("main").unwrap();
    git.cherry_pick(&feature).unwrap();
    assert!(git.path.join("feature.txt").exists());
    git.revert("HEAD").unwrap();
    assert!(!git.path.join("feature.txt").exists());
    fixture.write("shared.txt", "work in progress\n");
    git.stash().unwrap();
    assert!(git.snapshot(100).unwrap().status.is_empty());
    git.stash_pop().unwrap();
    assert_eq!(
        std::fs::read_to_string(git.path.join("shared.txt")).unwrap(),
        "work in progress\n"
    );
    git.stage("shared.txt").unwrap();
    git.commit("Main line changes").unwrap();
    git.checkout("feature").unwrap();
    fixture.write("shared.txt", "feature conflicting version\n");
    git.stage("shared.txt").unwrap();
    git.commit("Feature line changes").unwrap();
    git.checkout("main").unwrap();
    assert!(git.merge("feature").is_err());
    assert!(git
        .snapshot(100)
        .unwrap()
        .status
        .iter()
        .any(|f| f.index == 'U' && f.worktree == 'U'));
    assert!(git.path.join(".git/MERGE_HEAD").exists());
}

/// Leaves `shared.txt` as a `UU` merge conflict on `main`.
fn conflicted_fixture() -> Fixture {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("shared.txt", "base\n");
    fixture.write("other.txt", "other\n");
    git.stage("shared.txt").unwrap();
    git.stage("other.txt").unwrap();
    git.commit("Root").unwrap();
    git.create_branch("feature").unwrap();
    git.checkout("feature").unwrap();
    fixture.write("shared.txt", "feature\n");
    git.stage("shared.txt").unwrap();
    git.commit("Feature").unwrap();
    git.checkout("main").unwrap();
    fixture.write("shared.txt", "main\n");
    git.stage("shared.txt").unwrap();
    git.commit("Main").unwrap();
    assert!(git.merge("feature").is_err());
    fixture
}

#[test]
fn generic_staging_never_clears_unmerged_entries() {
    let fixture = conflicted_fixture();
    let git = &fixture.git;
    fixture.write("other.txt", "other edited\n");
    fixture.write("new.txt", "arrived\n");
    let unmerged = git.run(&["ls-files", "-u"]).unwrap();
    assert!(!unmerged.is_empty());
    let conflict = git
        .status()
        .unwrap()
        .into_iter()
        .find(|file| file.path == "shared.txt")
        .unwrap();
    assert!(conflict.is_unmerged());
    assert!(!conflict.on_side(true) && !conflict.on_side(false));
    for unstage in [false, true] {
        let error = git.stage_reviewed(&conflict, unstage).unwrap_err();
        assert_eq!(error.kind, FailureKind::Conflict);
    }
    assert!(git.stage("shared.txt").is_err());
    assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);

    let staged = git.stage_all(false).unwrap();
    assert_eq!(staged.conflicts, 1);
    assert_eq!(staged.changed, 2);
    assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
    assert_eq!(git.run(&["show", ":other.txt"]).unwrap(), "other edited\n");
    assert_eq!(git.run(&["show", ":new.txt"]).unwrap(), "arrived\n");
    let snapshot = git.snapshot(10).unwrap();
    let staged_paths: Vec<_> = snapshot
        .side(true)
        .into_iter()
        .map(|i| snapshot.status[i].path.as_str())
        .collect();
    assert_eq!(staged_paths, ["new.txt", "other.txt"]);
    assert_eq!(snapshot.conflicts().len(), 1);
    assert_eq!(snapshot.operation_label().as_deref(), Some("merge"));
    assert!(snapshot
        .commit_blocker()
        .is_some_and(|reason| reason.contains("1 conflicted file")));
    assert!(snapshot
        .operation_recovery()
        .contains("Abort returns to the state before the merge"));

    let unstaged = git.stage_all(true).unwrap();
    assert_eq!(unstaged.conflicts, 1);
    assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
    assert_eq!(git.run(&["show", ":other.txt"]).unwrap(), "other\n");
    assert!(git.run(&["ls-files", "--", "new.txt"]).unwrap().is_empty());
    assert!(git
        .snapshot(10)
        .unwrap()
        .status
        .iter()
        .any(|f| f.path == "shared.txt" && f.index == 'U' && f.worktree == 'U'));
}

#[test]
fn mark_resolved_refuses_conflict_markers_unless_confirmed() {
    let fixture = conflicted_fixture();
    let git = &fixture.git;
    let unmerged = git.run(&["ls-files", "-u"]).unwrap();
    let refused = git.mark_resolved("shared.txt", false).unwrap_err();
    assert_eq!(refused.kind, FailureKind::ConflictMarkers);
    assert!(refused.message.contains("line 1"));
    assert_eq!(git.run(&["ls-files", "-u"]).unwrap(), unmerged);
    assert!(git.mark_resolved("other.txt", false).is_err());
    assert!(git.mark_resolved("../shared.txt", false).is_err());

    fixture.write("shared.txt", "main and feature\n");
    git.mark_resolved("shared.txt", false).unwrap();
    assert!(git.run(&["ls-files", "-u"]).unwrap().is_empty());
    assert_eq!(
        git.run(&["show", ":shared.txt"]).unwrap(),
        "main and feature\n"
    );
    assert!(git.mark_resolved("shared.txt", false).is_err());

    let fixture = conflicted_fixture();
    let git = &fixture.git;
    git.mark_resolved("shared.txt", true).unwrap();
    assert!(git.run(&["ls-files", "-u"]).unwrap().is_empty());
    assert!(git
        .run(&["show", ":shared.txt"])
        .unwrap()
        .contains("<<<<<<<"));

    let fixture = conflicted_fixture();
    let git = &fixture.git;
    std::fs::remove_file(git.path.join("shared.txt")).unwrap();
    git.mark_resolved("shared.txt", false).unwrap();
    assert!(git
        .run(&["ls-files", "--", "shared.txt"])
        .unwrap()
        .is_empty());
}

#[test]
fn conflict_markers_need_an_opening_or_closing_line() {
    assert_eq!(conflict_marker_line(b"Title\n=======\nbody\n"), None);
    assert_eq!(
        conflict_marker_line(b"a\r\n<<<<<<< HEAD\r\nb\r\n=======\r\nc\r\n>>>>>>> topic\r\n"),
        Some(2)
    );
    assert_eq!(
        conflict_marker_line(b"x\n=======\ny\n>>>>>>> topic\n"),
        Some(2)
    );
    assert_eq!(conflict_marker_line(b"<<<<<<<< not a marker\n"), None);
}

#[test]
fn statuses_have_names_and_rename_sources() {
    let renamed = FileChange {
        path: "new.txt".into(),
        original_path: Some("old.txt".into()),
        index: 'R',
        worktree: 'M',
        unsupported_path: false,
    };
    assert_eq!(renamed.status_name(true), "Renamed");
    assert_eq!(renamed.status_name(false), "Modified");
    assert_eq!(renamed.describe(true), "Renamed: old.txt → new.txt");
    let conflict = FileChange {
        path: "c.txt".into(),
        index: 'U',
        worktree: 'D',
        ..Default::default()
    };
    assert_eq!(conflict.status_name(false), "Deleted by them");
    let untracked = FileChange {
        path: "u.txt".into(),
        index: '?',
        worktree: '?',
        ..Default::default()
    };
    assert_eq!(untracked.status_name(false), "Untracked");
}

#[test]
fn detached_and_unborn_snapshots_explain_commit_routes() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("first.txt", "first\n");
    let unborn = git.snapshot(10).unwrap();
    assert_eq!(unborn.head_label(), "main (no commits)");
    assert!(!unborn.is_detached());
    git.stage("first.txt").unwrap();
    assert_eq!(git.snapshot(10).unwrap().commit_blocker(), None);
    git.commit("First").unwrap();
    git.run(&["switch", "--detach", "HEAD"]).unwrap();
    fixture.write("first.txt", "changed\n");
    git.stage("first.txt").unwrap();
    let detached = git.snapshot(10).unwrap();
    assert!(detached.is_detached());
    assert!(detached
        .commit_blocker()
        .is_some_and(|reason| reason.contains("Create a branch")));
}

#[test]
fn real_git_rename_unstage_restores_both_index_paths_and_stale_stage_all_is_safe() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let original="Original component\nKeep this content to identify the renamed file.\nMore stable content.\n";
    fixture.write("old [name].txt", original);
    git.stage("old [name].txt").unwrap();
    git.commit("Track original name").unwrap();
    git.run(&["mv", "--", "old [name].txt", "new [name].txt"])
        .unwrap();
    let changed = format!("{original}Additional unstaged line\n");
    fixture.write("new [name].txt", &changed);
    git.stage("new [name].txt").unwrap();
    let state = git.snapshot(100).unwrap();
    assert_eq!(state.status.len(), 1);
    assert_eq!(state.status[0].index, 'R');
    assert_eq!(state.status[0].worktree, ' ');
    assert_eq!(
        state.status[0].original_path.as_deref(),
        Some("old [name].txt")
    );
    git.unstage("new [name].txt").unwrap();
    assert!(git
        .run(&["diff", "--cached", "--name-only"])
        .unwrap()
        .is_empty());
    assert!(!git.path.join("old [name].txt").exists());
    assert_eq!(
        std::fs::read_to_string(git.path.join("new [name].txt")).unwrap(),
        changed
    );
    for _ in 0..2 {
        git.stage_all(false).unwrap();
    }
    assert_eq!(git.snapshot(100).unwrap().status[0].index, 'R');
    git.unstage("new [name].txt").unwrap();
    assert!(git
        .run(&["diff", "--cached", "--name-only"])
        .unwrap()
        .is_empty());
}
#[test]
fn single_file_rejects_directory_replacement_but_bulk_uses_live_scope() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("tracked", "base\n");
    git.stage("tracked").unwrap();
    git.commit("base").unwrap();
    fixture.write("tracked", "reviewed edit\n");
    let review = git.status().unwrap().remove(0);
    let head = git.write_identity().unwrap();
    std::fs::remove_file(git.path.join("tracked")).unwrap();
    std::fs::create_dir(git.path.join("tracked")).unwrap();
    fixture.write("tracked/arrived.txt", "new nested file\n");
    fixture.write("late.txt", "appeared after review\n");
    assert!(git.stage_reviewed(&review, false).is_err());
    assert_eq!(git.write_identity().unwrap(), head);
    assert_eq!(git.run(&["show", ":tracked"]).unwrap(), "base\n");
    git.stage_all(false).unwrap();
    assert_eq!(
        git.run(&["show", ":tracked/arrived.txt"]).unwrap(),
        "new nested file\n"
    );
    assert_eq!(
        git.run(&["show", ":late.txt"]).unwrap(),
        "appeared after review\n"
    );
    git.stage_all(true).unwrap();
    assert_eq!(git.write_identity().unwrap(), head);
    assert_eq!(
        std::fs::read(git.path.join("tracked/arrived.txt")).unwrap(),
        b"new nested file\n"
    );
}

#[test]
fn rename_source_can_be_staged_as_an_independent_worktree_path() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("old.txt", "original\n");
    git.stage("old.txt").unwrap();
    git.commit("base").unwrap();
    let head = git.write_identity().unwrap().head;
    git.run(&["mv", "old.txt", "new.txt"]).unwrap();
    fixture.write("new.txt", "unrelated change in rename destination\n");
    fixture.write("old.txt", "independent worktree file\n");
    let review = git
        .status()
        .unwrap()
        .into_iter()
        .find(|file| file.path == "old.txt" && file.on_side(false))
        .unwrap();
    git.stage_reviewed(&review, false).unwrap();
    assert_eq!(git.write_identity().unwrap().head, head);
    assert_eq!(
        git.run(&["show", ":old.txt"]).unwrap(),
        "independent worktree file\n"
    );
    assert_eq!(git.run(&["show", ":new.txt"]).unwrap(), "original\n");
    assert_eq!(
        std::fs::read(git.path.join("new.txt")).unwrap(),
        b"unrelated change in rename destination\n"
    );
}

#[test]
fn staged_mode_and_rename_hunks_cannot_change_the_index() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let before = (0..40).map(|i| format!("line {i}\n")).collect::<String>();
    let after = before
        .replace("line 1\n", "first edit\n")
        .replace("line 35\n", "second edit\n");
    fixture.write("original.txt", &before);
    git.stage("original.txt").unwrap();
    git.commit("base").unwrap();
    fixture.write("original.txt", &after);
    git.stage("original.txt").unwrap();
    git.run(&["update-index", "--chmod=+x", "original.txt"])
        .unwrap();
    let hunks = git.hunks("original.txt", true).unwrap();
    assert_eq!(hunks.len(), 2);
    let reviewed = git.write_identity().unwrap();
    assert!(git
        .apply_hunk(&hunks[0], true)
        .unwrap_err()
        .message
        .contains("whole-file"));
    assert_eq!(git.write_identity().unwrap(), reviewed);
    assert_eq!(git.run(&["show", ":original.txt"]).unwrap(), after);
    assert!(git.run(&["ls-files", "-s"]).unwrap().starts_with("100755 "));

    git.run(&["mv", "original.txt", "renamed.txt"]).unwrap();
    let hunks = git.hunks("renamed.txt", true).unwrap();
    assert_eq!(hunks.len(), 2);
    let reviewed = git.write_identity().unwrap();
    assert!(git
        .apply_hunk(&hunks[0], true)
        .unwrap_err()
        .message
        .contains("whole-file"));
    assert_eq!(git.write_identity().unwrap(), reviewed);
    assert!(git.run(&["show", ":original.txt"]).is_err());
    assert_eq!(git.run(&["show", ":renamed.txt"]).unwrap(), after);
    assert!(!git.path.join("original.txt").exists());
    assert_eq!(
        std::fs::read_to_string(git.path.join("renamed.txt")).unwrap(),
        after
    );
}

#[test]
fn deletion_hunks_require_whole_file_actions_in_both_directions() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("deleted.txt", "delete me\n");
    git.stage("deleted.txt").unwrap();
    git.commit("base").unwrap();
    std::fs::remove_file(git.path.join("deleted.txt")).unwrap();
    let head = git.write_identity().unwrap();
    let hunk = git.hunks("deleted.txt", false).unwrap().remove(0);
    assert!(git
        .apply_hunk(&hunk, false)
        .unwrap_err()
        .message
        .contains("whole-file"));
    assert_eq!(git.write_identity().unwrap(), head);
    assert_eq!(git.run(&["show", ":deleted.txt"]).unwrap(), "delete me\n");
    git.stage("deleted.txt").unwrap();
    let staged = git.write_identity().unwrap();
    let hunk = git.hunks("deleted.txt", true).unwrap().remove(0);
    assert!(git
        .apply_hunk(&hunk, true)
        .unwrap_err()
        .message
        .contains("whole-file"));
    assert_eq!(git.write_identity().unwrap(), staged);
    assert!(git.run(&["show", ":deleted.txt"]).is_err());
    assert!(!git.path.join("deleted.txt").exists());
}

#[test]
fn machine_patches_round_trip_under_user_diff_settings() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let old = "first\n\nold\n\nlast\n";
    let new = "first\n\nnew\n\nlast\n";
    fixture.write(".gitattributes", "*.txt diff=custom\n");
    fixture.write("tracked.txt", old);
    git.stage(".").unwrap();
    git.commit("base").unwrap();
    let head = git.write_identity().unwrap();
    git.run(&["config", "--global", "diff.noprefix", "true"])
        .unwrap();
    for (key, value) in [
        ("diff.noprefix", "true"),
        ("diff.srcprefix", "custom-source/"),
        ("diff.dstprefix", "custom-destination/"),
        ("diff.context", "0"),
        ("diff.suppressBlankEmpty", "true"),
        ("diff.submodule", "diff"),
        ("diff.custom.textconv", "gitcrab-test-command-must-not-run"),
    ] {
        git.run(&["config", key, value]).unwrap();
    }
    let root = git.diff(Some(&head.head), "tracked.txt", false).unwrap();
    assert!(
        root.starts_with("diff --git a/tracked.txt b/tracked.txt\n"),
        "{root}"
    );
    fixture.write("tracked.txt", new);
    fixture.write("untracked.txt", new);
    for path in ["tracked.txt", "untracked.txt"] {
        let hunks = git.hunks(path, false).unwrap();
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].patch.contains(&format!("+++ b/{path}\n")));
        if path == "tracked.txt" {
            assert!(hunks[0].patch.contains("\n \n-old\n+new\n \n"));
        }
        git.apply_hunk(&hunks[0], false).unwrap();
        assert_eq!(git.run(&["show", &format!(":{path}")]).unwrap(), new);
        let staged = git.hunks(path, true).unwrap();
        git.apply_hunk(&staged[0], true).unwrap();
        assert_eq!(git.write_identity().unwrap(), head);
        assert_eq!(std::fs::read_to_string(git.path.join(path)).unwrap(), new);
    }
}

#[test]
fn broken_head_and_corrupt_index_are_errors_instead_of_empty_identities() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let original = std::fs::read(git.path.join(".git/HEAD")).unwrap();
    fixture.write(".git/HEAD", "not a reference\n");
    assert!(git.write_identity().is_err());
    std::fs::write(git.path.join(".git/HEAD"), original).unwrap();
    fixture.write(".git/index", "invalid index\n");
    assert!(git.write_identity().is_err());
}

#[cfg(unix)]
#[test]
fn unstaged_mode_and_content_hunks_preserve_the_executable_bit() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("file.txt", "base\n");
    git.stage("file.txt").unwrap();
    git.commit("base").unwrap();
    fixture.write("file.txt", "edited\n");
    std::fs::set_permissions(
        git.path.join("file.txt"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let before = git.write_identity().unwrap();
    let hunk = git.hunks("file.txt", false).unwrap().remove(0);
    assert!(git
        .apply_hunk(&hunk, false)
        .unwrap_err()
        .message
        .contains("whole-file"));
    assert_eq!(git.write_identity().unwrap(), before);
    assert_eq!(
        std::fs::metadata(git.path.join("file.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(git.run(&["show", ":file.txt"]).unwrap(), "base\n");
    assert_eq!(
        std::fs::read(git.path.join("file.txt")).unwrap(),
        b"edited\n"
    );
}

#[test]
fn real_git_new_file_hunk_can_be_staged_and_unstaged_without_touching_worktree() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let name = "new [file] with spaces.txt";
    let content = "First untracked line\nSecond untracked line\n";
    fixture.write(name, content);
    let before = git.write_identity().unwrap();
    let hunks = git.hunks(name, false).unwrap();
    assert_eq!(hunks.len(), 1);
    git.apply_hunk(&hunks[0], false).unwrap();
    assert_eq!(git.snapshot(100).unwrap().status[0].index, 'A');
    assert_eq!(git.run(&["show", &format!(":{name}")]).unwrap(), content);
    assert!(git
        .run(&["ls-files", "--stage"])
        .unwrap()
        .starts_with("100644 "));
    assert_eq!(git.write_identity().unwrap().head, before.head);
    let staged = git.hunks(name, true).unwrap();
    git.apply_hunk(&staged[0], true).unwrap();
    assert_eq!(git.snapshot(100).unwrap().status[0].index, '?');
    assert_eq!(git.write_identity().unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(git.path.join(name)).unwrap(),
        content
    );
}

#[test]
fn ignored_paths_reports_ignored_files_but_not_tracked_or_plain_ones() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    std::fs::create_dir_all(git.path.join("target/debug")).unwrap();
    fixture.write(".gitignore", "target/\n*.log\n");
    fixture.write("kept.log", "tracked\n");
    git.run(&["add", ".gitignore"]).unwrap();
    git.run(&["add", "-f", "kept.log"]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    fixture.write("target/debug/out.o", "build\n");
    fixture.write("run.log", "log\n");
    fixture.write("kept.log", "edited\n");
    fixture.write("notes", "new\n");
    let path = |name: &str| git.path.join(name);
    let asked = [
        path("target/debug/out.o"),
        path("target/debug"),
        path("run.log"),
        path("kept.log"),
        path("notes"),
        path(".gitignore"),
    ];
    let ignored = git.ignored_paths(&asked).unwrap();
    assert_eq!(
        ignored,
        [
            path("target/debug/out.o"),
            path("target/debug"),
            path("run.log")
        ]
        .into_iter()
        .collect()
    );
    // Nothing ignored is exit status 1, which is not a failure.
    assert!(git.ignored_paths(&asked[3..]).unwrap().is_empty());
}

#[test]
fn stashes_show_as_one_row_on_their_base_commit() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "base\n");
    git.run(&["add", "."]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    git.run(&["checkout", "-qb", "side"]).unwrap();
    fixture.write("a.txt", "side\n");
    git.run(&["commit", "-qam", "side work"]).unwrap();
    let side = git.run(&["rev-parse", "HEAD"]).unwrap().trim().to_owned();
    fixture.write("a.txt", "stashed\n");
    fixture.write("untracked.txt", "new\n");
    git.run(&["stash", "push", "-u", "-m", "on side"]).unwrap();
    git.run(&["checkout", "-q", "main"]).unwrap();
    git.run(&["branch", "-qD", "side"]).unwrap();
    fixture.write("a.txt", "working\n");

    let snapshot = git.snapshot(50).unwrap();
    let stash = git
        .run(&["rev-parse", "refs/stash"])
        .unwrap()
        .trim()
        .to_owned();
    let stash_rows: Vec<_> = snapshot.commits.iter().filter(|c| c.stash).collect();
    assert_eq!(stash_rows.len(), 1);
    assert_eq!(stash_rows[0].id, stash);
    assert_eq!(stash_rows[0].parents, std::slice::from_ref(&side));
    assert_eq!(stash_rows[0].subject, "on side");
    assert_eq!(snapshot.stashes[0].parts(), ("on side", Some("side")));
    let position = |id: &str| snapshot.commits.iter().position(|c| c.id == id).unwrap();
    assert!(position(&stash) < position(&side));
    assert_eq!(snapshot.commits.len(), 3);
    assert!(snapshot
        .commits
        .iter()
        .all(|c| c.refs.iter().all(|r| r != "stash")));
    assert_eq!(snapshot.stashes.len(), 1);
}

#[test]
fn snapshot_lists_worktrees_and_marks_the_current_one() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "base\n");
    git.run(&["add", "."]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    let linked = fixture.root().join("linked");
    let gone = fixture.root().join("gone");
    for (path, args) in [(&linked, &["-b", "side"][..]), (&gone, &["--detach"])] {
        let path = path.to_str().unwrap();
        git.run(&[&["worktree", "add", "-q"][..], args, &[path]].concat())
            .unwrap();
    }
    std::fs::remove_dir_all(&gone).unwrap();

    let worktrees = git.snapshot(50).unwrap().worktrees;
    let summary: Vec<_> = worktrees
        .iter()
        .map(|w| (w.label(), w.current, w.prunable))
        .collect();
    assert_eq!(
        summary,
        [
            ("main".into(), true, false),
            ("gone".into(), false, true),
            ("side".into(), false, false)
        ]
    );
}

#[test]
fn stash_rows_follow_date_order_and_wait_for_their_base() {
    let commit = |id: &str, parent: &str, date: &str| Commit {
        id: id.into(),
        parents: if parent.is_empty() {
            vec![]
        } else {
            vec![parent.into()]
        },
        date: date.into(),
        ..Default::default()
    };
    let stash = |id: &str, base: &str, date: &str| Commit {
        stash: true,
        ..commit(id, base, date)
    };
    let history = [
        commit("c", "b", "2026-01-03T00:00:00+00:00"),
        commit("b", "a", "2026-01-02T00:00:00+00:00"),
        commit("a", "", "2026-01-01T00:00:00+00:00"),
    ];
    let ids = |rows: Vec<Commit>| rows.into_iter().map(|c| c.id).collect::<Vec<_>>();
    // Newer than every commit: at the top, though its base is the oldest.
    assert_eq!(
        ids(with_stashes(
            &history,
            [stash("s", "a", "2026-01-04T00:00:00+00:00")]
        )),
        ["s", "c", "b", "a"]
    );
    // Offsets count: 01:00+02:00 is earlier than b.
    assert_eq!(
        ids(with_stashes(
            &history,
            [stash("s", "a", "2026-01-02T01:00:00+02:00")]
        )),
        ["c", "b", "s", "a"]
    );
    // Never below its base, even with an older date.
    assert_eq!(
        ids(with_stashes(
            &history,
            [stash("s", "b", "2025-12-31T00:00:00Z")]
        )),
        ["c", "s", "b", "a"]
    );
    assert_eq!(
        ids(with_stashes(
            &history,
            [stash("s", "missing", "2026-01-04T00:00:00Z")]
        )),
        ["c", "b", "a"]
    );
}

#[test]
fn snapshots_report_which_conflicted_files_still_have_markers() {
    let fixture = conflicted_fixture();
    let git = &fixture.git;
    let markers = |git: &Git| git.snapshot(10).unwrap().conflict_markers;
    assert!(markers(git).contains("shared.txt"));
    fixture.write("shared.txt", "main and feature\n");
    assert!(markers(git).is_empty());
    let (status, _) = git.worktree_state().unwrap();
    assert!(status.iter().any(|file| file.is_unmerged()));
    assert!(git.conflict_marker_paths(&status).is_empty());
}

/// Ten numbered lines, with the given lines replaced.
fn numbered(edits: &[(usize, &str)]) -> String {
    (1..=10)
        .map(|i| {
            let text = edits
                .iter()
                .find(|(line, _)| *line == i)
                .map_or_else(|| i.to_string(), |(_, text)| (*text).to_owned());
            format!("{text}\n")
        })
        .collect()
}

#[test]
fn stash_apply_with_index_restores_staging_or_falls_back_without_touching_the_tree() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("app.txt", &numbered(&[]));
    git.stage("app.txt").unwrap();
    git.commit("base").unwrap();
    fixture.write("app.txt", &numbered(&[(2, "two staged")]));
    git.stage("app.txt").unwrap();
    fixture.write("app.txt", &numbered(&[(2, "two staged"), (9, "nine")]));
    git.stash().unwrap();
    let id = git.resolve_commit("refs/stash").unwrap();

    assert!(git.stash_apply_with_index(&id).unwrap());
    let status = git.status().unwrap();
    assert_eq!((status[0].index, status[0].worktree), ('M', 'M'));
    assert_eq!(
        git.run(&["show", ":app.txt"]).unwrap(),
        numbered(&[(2, "two staged")])
    );
    git.reset_tracked_to_head().unwrap();

    // A commit inside the staged hunk's context stops Git from rebuilding the
    // stashed index, while the working-tree merge stays clean.
    fixture.write("app.txt", &numbered(&[(5, "five")]));
    git.stage("app.txt").unwrap();
    git.commit("five").unwrap();
    fixture.write("other.txt", "local\n");
    git.run(&["add", "other.txt"]).unwrap();
    let before = git.capture_tracked_state().unwrap().unwrap();
    let refused = git.stash_apply(&id, true).unwrap_err();
    assert!(refused.message.contains("without --index"), "{refused:?}");
    assert!(git.tracked_state_matches(&before).unwrap());

    assert!(!git.stash_apply_with_index(&id).unwrap());
    let status = git.status().unwrap();
    let app = status.iter().find(|file| file.path == "app.txt").unwrap();
    assert_eq!((app.index, app.worktree), (' ', 'M'));
    assert_eq!(
        std::fs::read_to_string(git.path.join("app.txt")).unwrap(),
        numbered(&[(2, "two staged"), (5, "five"), (9, "nine")])
    );
    assert_eq!(git.run(&["show", ":other.txt"]).unwrap(), "local\n");
    assert_eq!(git.stash_entries().unwrap()[0].0, id);
}

fn head_parents(git: &Git) -> usize {
    git.run(&["show", "-s", "--format=%P", "HEAD"])
        .unwrap()
        .split_whitespace()
        .count()
}

fn head_message(git: &Git) -> String {
    git.run(&["show", "-s", "--format=%B", "HEAD"])
        .unwrap()
        .trim()
        .to_owned()
}

fn resolve(fixture: &Fixture, text: &str) {
    fixture.write("shared.txt", text);
    fixture.git.mark_resolved("shared.txt", false).unwrap();
}

#[test]
fn committing_while_merge_head_exists_records_a_merge_commit_with_gits_message() {
    let fixture = conflicted_fixture();
    let git = &fixture.git;
    let main = git.resolve_commit("HEAD").unwrap();
    let snapshot = git.snapshot(10).unwrap();
    assert_eq!(snapshot.sequence(), Some(Sequence::Merge));
    let message = snapshot.operation_message.unwrap();
    assert_eq!(message, "Merge branch 'feature'");
    resolve(&fixture, "resolved\n");
    git.commit(&message).unwrap();
    assert_eq!(head_parents(git), 2);
    assert_eq!(git.resolve_commit("HEAD^1").unwrap(), main);
    assert_eq!(head_message(git), message);
    assert!(git.operation_markers().unwrap().is_empty());
}

#[test]
fn stopped_operations_abort_continue_and_skip_without_an_editor() {
    let fixture = conflicted_fixture();
    let git = &fixture.git;
    // Any step that opened an editor would fail.
    git.run(&["config", "core.editor", "false"]).unwrap();
    let main = git.resolve_commit("HEAD").unwrap();

    git.abort_sequence(Sequence::Merge).unwrap();
    assert_eq!(git.resolve_commit("HEAD").unwrap(), main);
    assert!(git.operation_markers().unwrap().is_empty());
    assert!(!git.has_unmerged_entries().unwrap());
    assert!(git.skip_sequence(Sequence::Merge).is_err());

    assert!(git.merge("feature").is_err());
    assert!(git.continue_sequence(Sequence::Merge, None).is_err());
    resolve(&fixture, "merged\n");
    git.continue_sequence(Sequence::Merge, None).unwrap();
    assert_eq!(head_parents(git), 2);
    assert_eq!(head_message(git), "Merge branch 'feature'");
    git.reset(&main, false).unwrap();
    git.run(&["checkout", "--", "shared.txt"]).unwrap();

    assert!(git.cherry_pick("feature").is_err());
    let snapshot = git.snapshot(10).unwrap();
    assert_eq!(snapshot.sequence(), Some(Sequence::CherryPick));
    assert_eq!(snapshot.operation_message.as_deref(), Some("Feature"));
    resolve(&fixture, "picked\n");
    git.continue_sequence(Sequence::CherryPick, Some("Feature, picked by hand"))
        .unwrap();
    assert_eq!(head_parents(git), 1);
    assert_eq!(head_message(git), "Feature, picked by hand");
    assert!(git.operation_markers().unwrap().is_empty());
    let picked = git.resolve_commit("HEAD").unwrap();

    assert!(git.cherry_pick("feature").is_err());
    git.skip_sequence(Sequence::CherryPick).unwrap();
    assert_eq!(git.resolve_commit("HEAD").unwrap(), picked);
    assert!(git.operation_markers().unwrap().is_empty());

    git.checkout("feature").unwrap();
    assert!(git.rebase("main").is_err());
    let snapshot = git.snapshot(10).unwrap();
    assert_eq!(snapshot.sequence(), Some(Sequence::Rebase));
    assert_eq!(snapshot.operation_message, None);
    assert!(snapshot.is_detached());
    resolve(&fixture, "rebased\n");
    git.continue_sequence(Sequence::Rebase, Some("ignored"))
        .unwrap();
    assert!(git.operation_markers().unwrap().is_empty());
    assert_eq!(git.resolve_commit("HEAD^").unwrap(), picked);
    assert_eq!(head_message(git), "Feature");
    assert_eq!(git.head_state().unwrap().0, "refs/heads/feature");
}

#[test]
fn a_sequence_between_picks_is_named_from_its_todo_list() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    let sequencer = git.path.join(".git/sequencer");
    std::fs::create_dir(&sequencer).unwrap();
    std::fs::write(sequencer.join("todo"), "revert 0123456 Fix\n").unwrap();
    let operations = git.operation_markers().unwrap();
    assert_eq!(operations, ["revert", "sequencer"]);
    assert_eq!(
        Sequence::from_operations(&operations),
        Some(Sequence::Revert)
    );
    std::fs::write(sequencer.join("todo"), "pick 0123456 Fix\n").unwrap();
    assert_eq!(
        git.operation_markers().unwrap(),
        ["cherry-pick", "sequencer"]
    );
}

#[test]
fn worktree_changes_add_lock_remove_and_prune() {
    use gitcrab_core::worktree::WorktreeChange;
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "base\n");
    git.run(&["add", "."]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    let wt = fixture.root().join("repo.worktrees").join("topic");
    let change = |change: WorktreeChange| git.change_worktree(&change, None);
    change(WorktreeChange::Add {
        path: wt.clone(),
        start: "main".into(),
        branch: Some("topic".into()),
    })
    .unwrap();
    let listed = |git: &Git| git.snapshot(10).unwrap().worktrees;
    let added = listed(git);
    assert_eq!(added[1].branch.as_deref(), Some("topic"));
    assert_eq!(added[1].changes, Some(WorktreeChanges::default()));

    std::fs::write(wt.join("a.txt"), "edit\n").unwrap();
    std::fs::write(wt.join("new.txt"), "new\n").unwrap();
    std::fs::remove_file(wt.join("a.txt")).unwrap();
    std::fs::write(wt.join("b.txt"), "b\n").unwrap();
    let changes = listed(git)[1].changes.unwrap();
    assert_eq!(
        (changes.added, changes.modified, changes.deleted),
        (2, 0, 1)
    );

    change(WorktreeChange::Lock {
        path: wt.clone(),
        reason: "busy".into(),
    })
    .unwrap();
    assert!(listed(git)[1].locked);
    let remove = |force| WorktreeChange::Remove {
        path: wt.clone(),
        force,
        delete_branch: Some("topic".into()),
        delete_remote: None,
    };
    assert!(change(remove(false)).is_err());
    let bad_branch = WorktreeChange::Remove {
        path: wt.clone(),
        force: true,
        delete_branch: Some("-bad".into()),
        delete_remote: None,
    };
    assert!(change(bad_branch).is_err());
    assert!(wt.exists(), "a refused branch name keeps the worktree");
    change(WorktreeChange::Unlock { path: wt.clone() }).unwrap();
    assert!(!listed(git)[1].locked);
    assert!(change(remove(false)).is_err(), "dirty worktree needs force");
    change(remove(true)).unwrap();
    assert_eq!(listed(git).len(), 1);
    assert!(git.run(&["rev-parse", "--verify", "-q", "topic"]).is_err());

    let gone = fixture.root().join("gone");
    change(WorktreeChange::Add {
        path: gone.clone(),
        start: "HEAD".into(),
        branch: Some("gone".into()),
    })
    .unwrap();
    std::fs::remove_dir_all(&gone).unwrap();
    assert!(listed(git)[1].prunable);
    change(WorktreeChange::Prune).unwrap();
    assert_eq!(listed(git).len(), 1);
}

#[test]
fn worktree_changes_ignore_a_planted_git_pointer() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "base\n");
    git.run(&["add", "."]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    let linked = fixture.root().join("linked");
    git.run(&[
        "worktree",
        "add",
        "-q",
        "--detach",
        linked.to_str().unwrap(),
    ])
    .unwrap();
    let elsewhere = Fixture::new();
    let pointer = elsewhere.git.path.join(".git").canonicalize().unwrap();
    std::fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", pointer.display()),
    )
    .unwrap();
    assert_eq!(git.snapshot(10).unwrap().worktrees[1].changes, None);
}

#[test]
fn removing_a_worktree_can_delete_its_branch_on_the_remote() {
    use gitcrab_core::worktree::WorktreeChange;
    let (local, _) = network::network_tests::connected();
    let git = &local.git;
    let wt = local.root().join("topic");
    git.change_worktree(
        &WorktreeChange::Add {
            path: wt.clone(),
            start: "main".into(),
            branch: Some("topic".into()),
        },
        None,
    )
    .unwrap();
    git.run(&["push", "-q", "origin", "topic"]).unwrap();
    let message = git
        .change_worktree(
            &WorktreeChange::Remove {
                path: wt,
                force: false,
                delete_branch: Some("topic".into()),
                delete_remote: Some(("origin".into(), "topic".into())),
            },
            Some(&network::network_tests::scope()),
        )
        .unwrap();
    assert!(message.ends_with("and origin/topic"), "{message}");
    assert!(git
        .run(&["ls-remote", "--heads", "origin", "topic"])
        .unwrap()
        .is_empty());
}

#[test]
fn a_changed_worktree_gets_a_history_row_above_its_head() {
    let fixture = Fixture::new();
    let git = &fixture.git;
    fixture.write("a.txt", "base\n");
    git.run(&["add", "."]).unwrap();
    git.run(&["commit", "-qm", "base"]).unwrap();
    fixture.write("a.txt", "next\n");
    git.run(&["commit", "-qam", "next"]).unwrap();
    let linked = fixture.root().join("old");
    git.run(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "old",
        linked.to_str().unwrap(),
        "HEAD~1",
    ])
    .unwrap();
    let base = git.run(&["rev-parse", "HEAD~1"]).unwrap().trim().to_owned();
    let mut cache = None;
    let read = |mode, cache: &mut _| git.snapshot_cached(10, mode, cache).unwrap().0.commits;
    assert!(read(HistoryRead::Full, &mut cache)
        .iter()
        .all(|c| c.worktree.is_none()));

    std::fs::write(linked.join("a.txt"), "dirty\n").unwrap();
    let ids = |rows: &[Commit]| rows.iter().map(|c| c.id.clone()).collect::<Vec<_>>();
    assert_eq!(
        read(HistoryRead::Worktree, &mut cache).len(),
        2,
        "a worktree read reuses the last counts"
    );
    let rows = read(HistoryRead::Full, &mut cache);
    let wip = rows.iter().position(|c| c.worktree.is_some()).unwrap();
    assert_eq!(ids(&rows)[wip + 1], base);
    assert_eq!(rows[wip].parents, [base]);
    assert_eq!(rows[wip].subject, "WIP on old");
    assert_eq!(rows[wip].body_preview, "1 modified");
}
