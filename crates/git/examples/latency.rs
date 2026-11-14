//! Headless latency benchmark for GitCrab's controller and Git worker.
//!
//! Drives the same `Controller` + `Service` loop the desktop view uses, without
//! rendering, and records every Git process the worker starts. Each scenario is
//! timed from the moment the user's action is dispatched to the moment the
//! controller is idle again ("settled"), and, where it applies, to the moment
//! the open diff is usable again ("ready").
//!
//! ```text
//! cargo run --release -p gitcrab-git --features git-trace --example latency -- \
//!     --repo <dir> [--make small|large] [--iterations 5] [--out <dir>] \
//!     [--read-only] [--git-config key=value ...]
//! ```
//!
//! `--make` creates a synthetic repository at `--repo` with `git fast-import`
//! if the directory does not exist yet. Write scenarios restore the repository
//! between iterations with plain Git commands that are not timed. Use
//! `--read-only` on a real repository you do not want changed.

use gitcrab_core::{
    controller::{Controller, Lane},
    platform::RepositoryLocation,
    service::{Action, GitCall, Operation, Request, Response},
};
use gitcrab_git::{git_trace, platform::RepositoryLocationExt, service::Service};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

const BIG_FILE: &str = "big.txt";
const BIG_LINES: usize = 3000;
const EDITED_FILES: usize = 20;
const UNTRACKED_FILES: usize = 50;

struct Args {
    repo: PathBuf,
    make: Option<String>,
    iterations: usize,
    out: Option<PathBuf>,
    read_only: bool,
    git_config: Vec<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        repo: PathBuf::new(),
        make: None,
        iterations: 5,
        out: None,
        read_only: false,
        git_config: vec![],
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--repo" => args.repo = it.next().expect("--repo <dir>").into(),
            "--make" => args.make = it.next(),
            "--iterations" => args.iterations = it.next().unwrap().parse().unwrap(),
            "--out" => args.out = it.next().map(PathBuf::from),
            "--read-only" => args.read_only = true,
            "--git-config" => args.git_config.push(it.next().expect("key=value")),
            "--help" | "-h" => {
                println!(
                    "{}",
                    include_str!("latency.rs")
                        .lines()
                        .take(14)
                        .collect::<Vec<_>>()
                        .join("\n")
                );
                std::process::exit(0);
            }
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(!args.repo.as_os_str().is_empty(), "--repo is required");
    args
}

// ---------------------------------------------------------------- git helpers

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Shape {
    files: usize,
    commits: usize,
    branches: usize,
    tags: usize,
}

fn shape(name: &str) -> Shape {
    match name {
        "small" => Shape {
            files: 100,
            commits: 200,
            branches: 3,
            tags: 5,
        },
        "large" => Shape {
            files: 50_000,
            commits: 20_000,
            branches: 40,
            tags: 200,
        },
        other => panic!("unknown --make {other}; use small or large"),
    }
}

fn big_file(seed: usize) -> String {
    (0..BIG_LINES).fold(String::new(), |mut s, i| {
        let _ = writeln!(s, "line {i} of the big file, revision {seed}");
        s
    })
}

/// Builds a repository with history, branches and tags in one fast-import pass.
fn make_repo(repo: &Path, shape: &Shape) {
    std::fs::create_dir_all(repo).unwrap();
    git(repo, &["init", "-q", "-b", "main"]);
    git(repo, &["config", "user.name", "Latency Bench"]);
    git(repo, &["config", "user.email", "bench@example.invalid"]);
    git(repo, &["config", "core.autocrlf", "false"]);
    let path = |i: usize| format!("src/d{:03}/f{i:05}.txt", i % 500);
    let mut stream = Vec::with_capacity(shape.files * 80);
    let mut time = 1_600_000_000u64;
    let mut mark = 0usize;
    let blob = |stream: &mut Vec<u8>, text: &str| {
        let _ = write!(stream, "data {}\n{text}\n", text.len());
    };
    // Initial tree.
    mark += 1;
    let _ = write!(
        stream,
        "commit refs/heads/main\nmark :{mark}\ncommitter Bench <b@x> {time} +0000\n"
    );
    blob(&mut stream, "initial");
    for i in 0..shape.files {
        let _ = writeln!(stream, "M 100644 inline {}", path(i));
        blob(&mut stream, &format!("file {i}\n"));
    }
    let _ = writeln!(stream, "M 100644 inline {BIG_FILE}");
    blob(&mut stream, &big_file(0));
    let mut rng = 0x2545F4914F6CDD1Du64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng as usize
    };
    let mut branch_points = vec![];
    for c in 1..shape.commits {
        time += 60;
        mark += 1;
        let _ = write!(
            stream,
            "commit refs/heads/main\nmark :{mark}\ncommitter Bench <b@x> {time} +0000\n"
        );
        blob(&mut stream, &format!("change {c}"));
        let _ = writeln!(stream, "from :{}", mark - 1);
        // The tip touches its own file, so switching to `side` never meets
        // the edits `reset_worktree` makes.
        let file = next() % shape.files;
        let target = if c == shape.commits - 1 {
            "last.txt".into()
        } else {
            path(file)
        };
        let _ = writeln!(stream, "M 100644 inline {target}");
        blob(&mut stream, &format!("file {file} revision {c}\n"));
        if c % (shape.commits / (shape.branches + 1)).max(1) == 0 {
            branch_points.push(mark);
        }
    }
    for (b, point) in branch_points.iter().take(shape.branches).enumerate() {
        time += 60;
        mark += 1;
        let _ = write!(
            stream,
            "commit refs/heads/topic-{b}\nmark :{mark}\ncommitter Bench <b@x> {time} +0000\n"
        );
        blob(&mut stream, &format!("topic {b}"));
        let _ = writeln!(stream, "from :{point}");
        let _ = writeln!(stream, "M 100644 inline topic-{b}.txt");
        blob(&mut stream, "topic\n");
    }
    for t in 0..shape.tags {
        let target = 1 + t * (shape.commits / shape.tags.max(1)).max(1);
        let _ = write!(
            stream,
            "reset refs/tags/v{t}\nfrom :{}\n\n",
            target.min(shape.commits)
        );
    }
    // `side` sits one commit behind main, so switching to it touches one file.
    let _ = write!(
        stream,
        "reset refs/heads/side\nfrom :{}\n\n",
        shape.commits - 1
    );
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&stream).unwrap();
    assert!(child.wait().unwrap().success(), "fast-import failed");
    git(repo, &["checkout", "-q", "-f", "main"]);
    git(repo, &["gc", "-q"]);
}

/// Puts the working tree into the state every scenario starts from: several
/// hunks in the big file, edited small files and untracked files, nothing staged.
fn reset_worktree(repo: &Path, files: usize) {
    git(repo, &["reset", "-q"]);
    let mut text = String::new();
    for i in 0..BIG_LINES {
        if i % (BIG_LINES / 8) == 10 {
            let _ = writeln!(text, "EDITED line {i}");
        } else {
            let _ = writeln!(text, "line {i} of the big file, revision 0");
        }
    }
    std::fs::write(repo.join(BIG_FILE), text).unwrap();
    for i in 0..EDITED_FILES.min(files) {
        let p = repo.join(format!("src/d{:03}/f{i:05}.txt", i % 500));
        std::fs::write(p, format!("edited {i}\n")).unwrap();
    }
    std::fs::create_dir_all(repo.join("untracked")).unwrap();
    for i in 0..UNTRACKED_FILES {
        std::fs::write(repo.join(format!("untracked/u{i}.txt")), "new\n").unwrap();
    }
}

// --------------------------------------------------------------- the driver

#[derive(Clone, Debug)]
struct Step {
    lane: Lane,
    label: &'static str,
    at_ms: f64,
    worker_ms: f64,
    queue_ms: f64,
    ok: bool,
}

#[derive(Clone, Debug)]
struct Sample {
    settled_ms: f64,
    ready_ms: Option<f64>,
    steps: Vec<Step>,
    calls: Vec<GitCall>,
    trace_start: f64,
    error: Option<String>,
}

struct Driver {
    service: Service,
    c: Controller,
    labels: HashMap<u32, &'static str>,
}

impl Driver {
    fn new() -> Self {
        Self {
            service: Service::new(),
            c: Controller::default(),
            labels: HashMap::new(),
        }
    }

    /// Dispatches `first` and runs the controller's follow-ups until idle.
    /// `ready` is checked after every response; the first time it holds is
    /// recorded as the "ready" time.
    fn run(
        &mut self,
        first: Vec<(Lane, Request)>,
        ready: Option<&dyn Fn(&Controller) -> bool>,
        started: Instant,
    ) -> (Vec<Step>, Option<f64>) {
        let mut queue: VecDeque<(Lane, tokio_rx::Receiver)> = VecDeque::new();
        for (lane, request) in first {
            self.submit(lane, request, &mut queue);
        }
        let mut steps = vec![];
        let mut ready_ms = None;
        while let Some((lane, rx)) = queue.pop_front() {
            let response: Response = rx.blocking_recv().expect("worker reply");
            let at_ms = started.elapsed().as_secs_f64() * 1000.;
            steps.push(Step {
                lane,
                label: self.labels.get(&response.sequence).copied().unwrap_or("?"),
                at_ms,
                worker_ms: response.elapsed_ms,
                queue_ms: response.queue_ms,
                ok: response.result.is_ok(),
            });
            for (lane, request) in self.c.complete(lane, response) {
                self.submit(lane, request, &mut queue);
            }
            if ready_ms.is_none() && ready.is_some_and(|f| f(&self.c)) {
                ready_ms = Some(started.elapsed().as_secs_f64() * 1000.);
            }
        }
        (steps, ready_ms)
    }

    fn submit(
        &mut self,
        lane: Lane,
        request: Request,
        queue: &mut VecDeque<(Lane, tokio_rx::Receiver)>,
    ) {
        self.labels.insert(request.sequence, request.action.label());
        let rx = self.service.submit(request).expect("submit");
        queue.push_back((lane, rx));
    }

    /// Times one user action. `start` turns the controller call into the jobs
    /// the view would dispatch.
    fn measure(
        &mut self,
        start: impl FnOnce(&mut Self) -> Vec<(Lane, Request)>,
        ready: Option<&dyn Fn(&Controller) -> bool>,
    ) -> Sample {
        git_trace::drain();
        let trace_start = git_trace::now_ms();
        let started = Instant::now();
        let jobs = start(self);
        let (steps, ready_ms) = self.run(jobs, ready, started);
        let settled_ms = started.elapsed().as_secs_f64() * 1000.;
        let error = self.c.error.then(|| self.c.notice.clone());
        Sample {
            settled_ms,
            ready_ms,
            steps,
            calls: git_trace::drain(),
            trace_start,
            error,
        }
    }

    /// Runs an action without recording it, for setup between samples.
    fn quietly(&mut self, start: impl FnOnce(&mut Self) -> Vec<(Lane, Request)>) {
        let jobs = start(self);
        self.run(jobs, None, Instant::now());
        git_trace::drain();
    }

    fn refresh_jobs(
        &mut self,
        plan: Option<gitcrab_core::controller::RefreshPlan>,
    ) -> Vec<(Lane, Request)> {
        let Some(plan) = plan else { return vec![] };
        for kind in plan.cancel {
            self.service.cancel_read(kind);
        }
        plan.dispatch.into_iter().collect()
    }

    fn reviewed(&self) -> gitcrab_core::git::WriteIdentity {
        self.c.snapshot.as_ref().unwrap().reviewed.clone().unwrap()
    }
}

mod tokio_rx {
    pub type Receiver = tokio::sync::oneshot::Receiver<gitcrab_core::service::Response>;
}

// ------------------------------------------------------------- scenarios

fn diff_ready(c: &Controller) -> bool {
    c.patch_ready() && !c.busy
}

fn one<T>(job: Option<T>) -> Vec<T> {
    job.into_iter().collect()
}

struct Results {
    scenarios: Vec<(String, Vec<Sample>)>,
}

impl Results {
    fn push(&mut self, name: &str, sample: Sample) {
        if let Some(error) = &sample.error {
            eprintln!("  {name}: controller error: {error}");
        }
        match self.scenarios.iter_mut().find(|(n, _)| n == name) {
            Some((_, samples)) => samples.push(sample),
            None => self.scenarios.push((name.into(), vec![sample])),
        }
    }
}

fn open_diff(d: &mut Driver, path: &str, staged: bool) {
    let path = path.to_owned();
    d.quietly(|d| one(d.c.diff(path, staged)));
    assert!(
        d.c.patch_ready(),
        "diff for {BIG_FILE} did not load: {}",
        d.c.notice
    );
}

fn run_scenarios(args: &Args, files: usize) -> Results {
    let repo = &args.repo;
    let mut results = Results { scenarios: vec![] };
    let mut d = Driver::new();
    let location = RepositoryLocation::discover(repo).expect("discover repository");

    for i in 0..args.iterations {
        eprintln!("iteration {}/{}", i + 1, args.iterations);
        if !args.read_only {
            reset_worktree(repo, files);
        }

        // Opening a repository: everything from trust confirmation to history.
        let loc = location.clone();
        let s = d.measure(|d| one(d.c.open_trusted(loc)), None);
        results.push("open repository", s);

        let s = d.measure(
            |d| {
                let p = d.c.refresh();
                d.refresh_jobs(p)
            },
            None,
        );
        results.push("refresh (F5, full)", s);

        let s = d.measure(
            |d| {
                let p = d.c.watcher_refresh(false);
                d.refresh_jobs(p)
            },
            None,
        );
        results.push("refresh (watcher, worktree)", s);

        let head = d.c.snapshot.as_ref().unwrap().head_id.clone();
        let s = d.measure(|d| one(d.c.select(Some(head))), None);
        results.push("select commit (details)", s);
        d.quietly(|d| one(d.c.select(None)));

        let big = if args.read_only {
            d.c.snapshot
                .as_ref()
                .unwrap()
                .status
                .first()
                .map(|f| f.path.clone())
        } else {
            Some(BIG_FILE.to_owned())
        };
        if let Some(big) = big.clone() {
            let s = d.measure(|d| one(d.c.diff(big, false)), Some(&diff_ready));
            results.push("open working diff", s);
        }
        if args.read_only {
            continue;
        }

        // Stage then unstage one hunk of the big file.
        open_diff(&mut d, BIG_FILE, false);
        let token = d.c.patch.as_ref().unwrap().token;
        let s = d.measure(
            |d| {
                one(d.c.mutate(Action::Hunk {
                    token,
                    index: 0,
                    unstage: false,
                }))
            },
            Some(&diff_ready),
        );
        results.push("stage hunk", s);
        open_diff(&mut d, BIG_FILE, true);
        let token = d.c.patch.as_ref().unwrap().token;
        let s = d.measure(
            |d| {
                one(d.c.mutate(Action::Hunk {
                    token,
                    index: 0,
                    unstage: true,
                }))
            },
            Some(&diff_ready),
        );
        results.push("unstage hunk", s);

        // Stage then unstage a whole small file.
        let small = format!("src/d000/f{:05}.txt", 0);
        let file = |d: &Driver, staged: bool| {
            d.c.snapshot
                .as_ref()
                .unwrap()
                .status
                .iter()
                .find(|f| f.path == small && f.on_side(staged))
                .cloned()
                .expect("changed file")
        };
        let reviewed = file(&d, false);
        let s = d.measure(
            |d| {
                one(d.c.mutate(Action::StageFile {
                    reviewed,
                    unstage: false,
                }))
            },
            None,
        );
        results.push("stage file", s);
        let reviewed = file(&d, true);
        let s = d.measure(
            |d| {
                one(d.c.mutate(Action::StageFile {
                    reviewed,
                    unstage: true,
                }))
            },
            None,
        );
        results.push("unstage file", s);

        let s = d.measure(
            |d| one(d.c.mutate(Action::StageAll { unstage: false })),
            None,
        );
        results.push("stage all", s);
        let s = d.measure(
            |d| one(d.c.mutate(Action::StageAll { unstage: true })),
            None,
        );
        results.push("unstage all", s);

        // Commit one staged file, then undo it outside the measurement.
        let reviewed = file(&d, false);
        d.quietly(|d| {
            one(d.c.mutate(Action::StageFile {
                reviewed,
                unstage: false,
            }))
        });
        d.c.set_draft(format!("bench commit {i}"));
        let s = d.measure(
            |d| {
                let reviewed = Some(d.reviewed());
                let message = d.c.draft.clone();
                one(d.c.mutate(Action::Commit { message, reviewed }))
            },
            None,
        );
        results.push("commit", s);
        git(repo, &["reset", "-q", "--soft", "HEAD~1"]);
        git(repo, &["reset", "-q"]);
        d.quietly(|d| {
            let p = d.c.refresh();
            d.refresh_jobs(p)
        });

        // Create a branch, then delete it outside the measurement.
        let name = format!("bench-{i}");
        let s = d.measure(
            |d| {
                let destination = d.reviewed();
                one(d.c.mutate(Action::RunOperation {
                    op: Operation::CreateBranch,
                    target: name.clone(),
                    destination,
                }))
            },
            None,
        );
        results.push("create branch", s);
        git(repo, &["branch", "-q", "-D", &name]);
        d.quietly(|d| {
            let p = d.c.refresh();
            d.refresh_jobs(p)
        });

        // Switch to `side` (one commit behind) and back.
        for (target, label) in [("side", "checkout branch"), ("main", "checkout back")] {
            let s = d.measure(
                |d| {
                    let destination = d.reviewed();
                    one(d.c.mutate(Action::RunOperation {
                        op: Operation::Checkout,
                        target: target.into(),
                        destination,
                    }))
                },
                None,
            );
            results.push(label, s);
        }

        // Stash tracked changes and pop them again.
        let s = d.measure(
            |d| {
                let destination = d.reviewed();
                one(d.c.mutate(Action::RunOperation {
                    op: Operation::Stash,
                    target: format!("bench {i}"),
                    destination,
                }))
            },
            None,
        );
        results.push("stash", s);
        let s = d.measure(
            |d| {
                let destination = d.reviewed();
                one(d.c.mutate(Action::RunOperation {
                    op: Operation::PopStash,
                    target: String::new(),
                    destination,
                }))
            },
            None,
        );
        results.push("pop stash", s);
    }
    results
}

// -------------------------------------------------------------- reporting

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// The Git subcommand of a call, skipping `-c key=value` and `-C dir` prefixes.
fn subcommand(args: &str) -> String {
    let mut words = args.split(' ').peekable();
    let mut out = vec![];
    while let Some(w) = words.next() {
        if w == "-c" || w == "-C" || w == "--git-dir" || w == "--work-tree" {
            words.next();
        } else if w.starts_with("--git-dir=")
            || w.starts_with("--work-tree=")
            || w.starts_with("-c") && w.len() > 2
        {
        } else {
            out.push(w);
            out.extend(words.by_ref().take(3));
            break;
        }
    }
    let text = out.join(" ");
    if text.len() > 70 {
        format!("{}…", &text[..70])
    } else {
        text
    }
}

/// Wall time during which at least one Git process was running.
fn busy_git_ms(calls: &[GitCall]) -> f64 {
    let mut spans: Vec<(f64, f64)> = calls
        .iter()
        .map(|c| (c.start_ms, c.start_ms + c.total_ms))
        .collect();
    spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut total = 0.;
    let mut current: Option<(f64, f64)> = None;
    for (s, e) in spans {
        match &mut current {
            Some((_, ce)) if s <= *ce => *ce = ce.max(e),
            _ => {
                if let Some((cs, ce)) = current {
                    total += ce - cs;
                }
                current = Some((s, e));
            }
        }
    }
    if let Some((cs, ce)) = current {
        total += ce - cs;
    }
    total
}

/// Median time of `git rev-parse HEAD` for each executable and Windows
/// creation flag. GitCrab is a GUI app with no console to inherit, so its
/// real choices are a hidden console (`CREATE_NO_WINDOW`) or none at all
/// (`DETACHED_PROCESS`).
fn spawn_baseline(repo: &Path) -> Vec<(String, f64)> {
    let mut exes = vec![PathBuf::from("git")];
    if let Ok(path) = std::env::var("GITCRAB_GIT") {
        exes.push(path.into());
    }
    #[cfg(windows)]
    {
        let real = PathBuf::from(r"C:\Program Files\Git\mingw64\bin\git.exe");
        if real.is_file() && !exes.contains(&real) {
            exes.push(real);
        }
    }
    #[cfg(windows)]
    let flags: &[(&str, u32)] = &[
        ("inherit console", 0),
        ("CREATE_NO_WINDOW", 0x0800_0000),
        ("DETACHED_PROCESS", 0x0000_0008),
    ];
    #[cfg(not(windows))]
    let flags: &[(&str, u32)] = &[("default", 0)];
    let mut rows = vec![];
    for exe in &exes {
        for (name, flag) in flags {
            let mut v = vec![];
            for _ in 0..20 {
                let mut cmd = Command::new(exe);
                cmd.arg("-C")
                    .arg(repo)
                    .args(["rev-parse", "HEAD"])
                    .stdin(Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(*flag);
                }
                #[cfg(not(windows))]
                let _ = flag;
                let t = Instant::now();
                let out = cmd.output().unwrap();
                assert!(out.status.success());
                v.push(t.elapsed().as_secs_f64() * 1000.);
            }
            rows.push((format!("`{}` · {name}", exe.display()), median(v)));
        }
    }
    rows
}

fn report(args: &Args, results: &Results, baseline: &[(String, f64)]) -> String {
    let mut s = String::new();
    let tracked = git(&args.repo, &["ls-files"]).lines().count();
    let commits = git(&args.repo, &["rev-list", "--all", "--count"])
        .trim()
        .to_owned();
    let version = git(&args.repo, &["--version"]).trim().to_owned();
    let _ = writeln!(s, "# GitCrab latency: {}\n", args.repo.display());
    let _ = writeln!(
        s,
        "{tracked} tracked files · {commits} commits · {version} · {} iterations · median values\n",
        args.iterations
    );
    let git_used = std::env::var("GITCRAB_GIT").unwrap_or_else(|_| "first git.exe on PATH".into());
    let _ = writeln!(s, "GitCrab ran Git as: `{git_used}`\n");
    let _ = writeln!(
        s,
        "Process baseline: `git rev-parse HEAD` with a plain `Command`, median of 20\n"
    );
    let _ = writeln!(s, "| executable · creation flags | ms |\n|---|---:|");
    for (name, ms) in baseline {
        let _ = writeln!(s, "| {name} | {ms:.1} |");
    }
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "- **settled**: from dispatch until the controller has nothing in flight."
    );
    let _ = writeln!(
        s,
        "- **ready**: until the open diff is usable again (hunk actions enabled)."
    );
    let _ = writeln!(
        s,
        "- **round trips**: worker requests in sequence. **git procs**: processes started."
    );
    let _ = writeln!(s, "- **git busy**: wall time with at least one Git process running. **spawn**: summed process start time.\n");
    let _ = writeln!(s, "| action | settled ms | ready ms | round trips | git procs | git busy ms | spawn ms (sum) | worker ms (sum) |");
    let _ = writeln!(s, "|---|---:|---:|---:|---:|---:|---:|---:|");
    for (name, samples) in &results.scenarios {
        let m = |f: &dyn Fn(&Sample) -> f64| median(samples.iter().map(f).collect());
        let ready = samples
            .iter()
            .filter_map(|x| x.ready_ms)
            .collect::<Vec<_>>();
        let _ = writeln!(
            s,
            "| {name} | {:.0} | {} | {:.0} | {:.0} | {:.0} | {:.0} | {:.0} |",
            m(&|x| x.settled_ms),
            if ready.is_empty() {
                "–".into()
            } else {
                format!("{:.0}", median(ready))
            },
            m(&|x| x.steps.len() as f64),
            m(&|x| x.calls.len() as f64),
            m(&|x| busy_git_ms(&x.calls)),
            m(&|x| x.calls.iter().map(|c| c.spawn_ms).sum()),
            m(&|x| x.steps.iter().map(|st| st.worker_ms).sum()),
        );
    }
    let _ = writeln!(s, "\n## Timelines (median sample of each action)\n");
    for (name, samples) in &results.scenarios {
        let mut sorted: Vec<&Sample> = samples.iter().collect();
        sorted.sort_by(|a, b| a.settled_ms.partial_cmp(&b.settled_ms).unwrap());
        let x = sorted[sorted.len() / 2];
        let _ = writeln!(s, "### {name} — {:.0} ms\n", x.settled_ms);
        let _ = writeln!(s, "| worker step | done at ms | worker ms | queued ms |");
        let _ = writeln!(s, "|---|---:|---:|---:|");
        for st in &x.steps {
            let _ = writeln!(
                s,
                "| {:?}: {}{} | {:.0} | {:.0} | {:.1} |",
                st.lane,
                st.label,
                if st.ok { "" } else { " (failed)" },
                st.at_ms,
                st.worker_ms,
                st.queue_ms
            );
        }
        let _ = writeln!(
            s,
            "\n| git call | starts at ms | spawn ms | total ms | stdout KB |"
        );
        let _ = writeln!(s, "|---|---:|---:|---:|---:|");
        for c in &x.calls {
            let _ = writeln!(
                s,
                "| `{}` | {:.0} | {:.1} | {:.1} | {:.0} |",
                subcommand(&c.args),
                c.start_ms - x.trace_start,
                c.spawn_ms,
                c.total_ms,
                c.stdout_bytes as f64 / 1024.
            );
        }
        let _ = writeln!(s);
    }
    s
}

fn main() {
    let args = parse_args();
    let mut files = 0;
    if let Some(kind) = &args.make {
        let shape = shape(kind);
        files = shape.files;
        if !args.repo.join(".git").exists() {
            eprintln!("creating {kind} repository at {}", args.repo.display());
            let t = Instant::now();
            make_repo(&args.repo, &shape);
            eprintln!("created in {:.1} s", t.elapsed().as_secs_f64());
        }
    } else if !args.read_only {
        panic!("write scenarios need a repository made with --make; use --read-only otherwise");
    }
    if !args.read_only {
        // Leave nothing from an interrupted run behind.
        let _ = Command::new("git")
            .arg("-C")
            .arg(&args.repo)
            .args(["checkout", "-q", "main"])
            .output();
        while Command::new("git")
            .arg("-C")
            .arg(&args.repo)
            .args(["stash", "drop", "-q"])
            .output()
            .is_ok_and(|o| o.status.success())
        {}
    }
    for kv in &args.git_config {
        let (k, v) = kv.split_once('=').expect("key=value");
        git(&args.repo, &["config", k, v]);
    }
    let baseline = spawn_baseline(&args.repo);
    let results = run_scenarios(&args, files);
    let text = report(&args, &results, &baseline);
    match &args.out {
        Some(path) => {
            std::fs::write(path, &text).unwrap();
            eprintln!("wrote {}", path.display());
        }
        None => println!("{text}"),
    }
}
