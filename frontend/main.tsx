import { useEffect, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import type { Action, Operation, Reply } from "./contract";
import { client, errorMessage } from "./client";
import { History } from "./History";
import { Diff, type DiffData } from "./Diff";
import "./style.css";

type Snapshot = Extract<Reply, { kind: "Snapshot" }>["data"];
type Details = Extract<Reply, { kind: "Details" }>["data"];
const labels: Record<Operation, string> = {
  CreateBranch: "Create branch",
  Checkout: "Checkout",
  Merge: "Merge",
  CherryPick: "Cherry-pick",
  Revert: "Revert",
  Stash: "Stash",
  PopStash: "Pop stash",
};
function App() {
  const [repo, setRepo] = useState<Snapshot>(),
    [selected, setSelected] = useState<string | null>(null),
    [details, setDetails] = useState<Details>(),
    [diff, setDiff] = useState<DiffData>();
  const [search, setSearch] = useState(""),
    [draft, setDraft] = useState(""),
    [busy, setBusy] = useState(false),
    [loading, setLoading] = useState(false),
    [notice, setNotice] = useState("Open a local repository to begin"),
    [error, setError] = useState(false),
    [limit, setLimit] = useState(500),
    [split, setSplit] = useState(false);
  const [operation, setOperation] = useState<{
      op: Operation;
      target: string;
    }>(),
    [menu, setMenu] = useState<{ id: string; x: number; y: number }>(),
    [opening, setOpening] = useState(false);
  const selectedRef = useRef(selected),
    fileRef = useRef<DiffData | undefined>(undefined),
    draftRef = useRef(draft),
    repoRef = useRef(repo),
    busyRef = useRef(false),
    limitRef = useRef(limit),
    loadVersion = useRef(0);
  selectedRef.current = selected;
  fileRef.current = diff;
  draftRef.current = draft;
  repoRef.current = repo;
  limitRef.current = limit;
  function fail(e: unknown) {
    setNotice(errorMessage(e));
    setError(true);
  }
  function select(id: string | null) {
    client.invalidate("details");
    client.invalidate("diff");
    setSelected(id);
    selectedRef.current = id;
    setDetails(undefined);
    setDiff(undefined);
    fileRef.current = undefined;
    setMenu(undefined);
    if (id) loadDetails(id);
  }
  function loadDetails(id: string) {
    const epoch = client.epoch;
    client.lanes.details.run(async () => {
      if (epoch !== client.epoch || id !== selectedRef.current) return;
      try {
        const r = await client.request(
          { kind: "Details", args: { id } },
          "details",
        );
        if (r?.kind === "Details" && id === selectedRef.current)
          setDetails(r.data);
      } catch (e) {
        if (epoch === client.epoch && id === selectedRef.current) fail(e);
      }
    });
  }
  function loadDiff(id: string | null, path: string, staged: boolean) {
    client.invalidate("diff");
    const epoch = client.epoch;
    // Clear the previous source immediately so hunk/file buttons cannot act on
    // a patch that belongs to the preceding selection.
    setDiff(undefined);
    fileRef.current = undefined;
    client.lanes.diff.run(async () => {
      if (epoch !== client.epoch || id !== selectedRef.current) return;
      try {
        const r = await client.request(
          { kind: "Diff", args: { id, path, staged } },
          "diff",
        );
        if (r?.kind === "Diff" && id === selectedRef.current) {
          setDiff(r.data);
          fileRef.current = r.data;
        }
      } catch (e) {
        if (epoch === client.epoch) fail(e);
      }
    });
  }
  function refresh(next = limitRef.current) {
    next = Math.min(next, 100_000);
    if (!repoRef.current) return;
    setLimit(next);
    limitRef.current = next;
    setLoading(true);
    const epoch = client.epoch,
      version = ++loadVersion.current;
    client.lanes.snapshot.run(async () => {
      if (epoch !== client.epoch) return;
      try {
        const r = await client.request(
          { kind: "Refresh", args: { limit: next } },
          "snapshot",
        );
        if (r?.kind === "Snapshot") {
          setRepo(r.data);
          repoRef.current = r.data;
          const current = selectedRef.current;
          if (
            current &&
            !r.data.snapshot.commits.some((c) => c.id === current)
          ) {
            select(null);
          } else if (current) loadDetails(current);
          const f = fileRef.current;
          if (f) loadDiff(f.id, f.path, f.staged);
        }
      } catch (e) {
        if (epoch === client.epoch) fail(e);
      } finally {
        if (epoch === client.epoch && version === loadVersion.current)
          setLoading(false);
      }
    });
  }
  async function open(path?: string) {
    if (busyRef.current) return;
    try {
      path =
        path ?? (await invoke<string | null>("pick_repository")) ?? undefined;
      if (!path) return;
      client.epoch++;
      client.invalidate("snapshot");
      client.invalidate("details");
      client.invalidate("diff");
      const epoch = client.epoch;
      setOpening(true);
      setRepo(undefined);
      repoRef.current = undefined;
      select(null);
      setLimit(500);
      limitRef.current = 500;
      setError(false);
      setNotice("Opening repository…");
      setLoading(true);
      // Open uses the snapshot lane too: old refresh requests finish before the
      // new repository takes ownership, and obsolete queued reads are replaced.
      client.lanes.snapshot.run(async () => {
        try {
          const r = await client.request(
            { kind: "Open", args: { path: path!, limit: 500 } },
            "snapshot",
          );
          if (r?.kind === "Snapshot") {
            setRepo(r.data);
            repoRef.current = r.data;
            setNotice(`Opened ${r.data.path}`);
            setDraft("");
          }
        } catch (e) {
          if (epoch === client.epoch) fail(e);
        } finally {
          if (epoch === client.epoch) {
            setOpening(false);
            setLoading(false);
          }
        }
      });
    } catch (e) {
      fail(e);
    }
  }
  async function mutate(action: Action) {
    if (busyRef.current || !repoRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(false);
    setNotice("Running Git operation…");
    const epoch = client.epoch;
    client.invalidate("details");
    client.invalidate("diff");
    try {
      const r = await client.request(action, "mutation");
      if (r?.kind === "Done") {
        setNotice(r.data.message);
        if (
          r.data.committed_draft !== null &&
          r.data.committed_draft === draftRef.current
        )
          setDraft("");
      }
    } catch (e) {
      if (epoch === client.epoch) fail(e);
    } finally {
      busyRef.current = false;
      setBusy(false);
      if (epoch === client.epoch) refresh();
    }
  }
  useEffect(() => {
    void invoke<string | null>("startup_repository")
      .then((path) => {
        if (path) void open(path);
      })
      .catch(fail);
  }, []);
  // Keep shortcuts outside editors; normal copy/select-all remains native DOM.
  const shortcuts = useRef({ open, refresh });
  useEffect(() => {
    if (menu)
      document
        .querySelector<HTMLButtonElement>(".context-menu button:not(:disabled)")
        ?.focus();
  }, [menu]);
  shortcuts.current = { open, refresh };
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const mac = /Mac/.test(navigator.platform);
      if (e.key.toLowerCase() === "o" && (mac ? e.metaKey : e.ctrlKey)) {
        e.preventDefault();
        void shortcuts.current.open();
      }
      if (e.key === "F5") {
        e.preventDefault();
        shortcuts.current.refresh();
      }
      if (e.key === "Escape") {
        if (document.activeElement?.closest(".context-menu"))
          document.querySelector<HTMLElement>(".history-viewport")?.focus();
        setMenu(undefined);
        setOperation(undefined);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
  const snap = repo?.snapshot,
    commit = snap?.commits.find((c) => c.id === selected),
    staged =
      snap?.status.filter((f) => f.index !== " " && f.index !== "?") ?? [],
    unstaged =
      snap?.status.filter((f) => f.worktree !== " " || f.index === "?") ?? [],
    conflict = snap?.status.some(
      (f) =>
        f.index === "U" ||
        f.worktree === "U" ||
        `${f.index}${f.worktree}` === "AA" ||
        `${f.index}${f.worktree}` === "DD",
    );
  function review(op: Operation, target = "") {
    setMenu(undefined);
    setOperation({ op, target });
  }
  function resize(e: React.PointerEvent, side: "nav" | "inspector") {
    const start = e.clientX,
      root = document.documentElement,
      property = side === "nav" ? "--nav" : "--inspector",
      width = parseInt(getComputedStyle(root).getPropertyValue(property));
    e.currentTarget.setPointerCapture(e.pointerId);
    const move = (ev: PointerEvent) =>
      root.style.setProperty(
        property,
        `${Math.max(side === "nav" ? 150 : 260, Math.min(side === "nav" ? 330 : 560, width + (side === "nav" ? 1 : -1) * (ev.clientX - start)))}px`,
      );
    const end = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", end);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", end);
  }
  function files(isStaged: boolean) {
    const list = isStaged ? staged : unstaged;
    return (
      <section className="file-section">
        <header>
          <span>
            {isStaged ? "Staged" : "Unstaged"} files{" "}
            <small>({list.length})</small>
          </span>
          <button
            disabled={busy || !list.length}
            onClick={() =>
              void mutate({
                kind: "Stage",
                args: { path: ".", unstage: isStaged },
              })
            }
          >
            {isStaged ? "Unstage" : "Stage"} all
          </button>
        </header>
        <div className="file-list">
          {list.map((f) => (
            <div
              className={`file-row ${diff?.path === f.path && diff.staged === isStaged ? "active" : ""}`}
              key={f.path}
            >
              <span
                className={`status status-${isStaged ? f.index : f.worktree}`}
              >
                {isStaged ? f.index : f.worktree === " " ? "?" : f.worktree}
              </span>
              <button
                className="file-name"
                title={
                  f.original_path ? `${f.original_path} → ${f.path}` : f.path
                }
                onClick={() => loadDiff(null, f.path, isStaged)}
              >
                {f.path}
              </button>
              <button
                disabled={busy}
                aria-label={`${isStaged ? "Unstage" : "Stage"} ${f.path}`}
                onClick={() =>
                  void mutate({
                    kind: "Stage",
                    args: { path: f.path, unstage: isStaged },
                  })
                }
              >
                {isStaged ? "−" : "+"}
              </button>
            </div>
          ))}
          {!list.length && (
            <p className="empty">
              No {isStaged ? "staged" : "unstaged"} changes
            </p>
          )}
        </div>
      </section>
    );
  }
  return (
    <div className="app" onClick={() => menu && setMenu(undefined)}>
      <div className="menubar">
        <strong>◉ GitCrab</strong>
        <button onClick={() => void open()} disabled={busy || opening}>
          Open repository…
        </button>
        <span>LOCAL GIT CLIENT</span>
      </div>
      <div className="tabbar">
        <span>▱</span>
        <strong title={repo?.path}>
          {repo?.path.split(/[\\/]/).pop() || "Repository"}
        </strong>
        <span className="edition">Independent edition</span>
      </div>
      <div className="toolbar">
        <div className="repo-label">
          <small>repository</small>
          <strong>
            {repo?.path.split(/[\\/]/).pop() || "Open a repository"}
          </strong>
        </div>
        <div className="branch-label">
          <small>branch</small>
          <strong title={snap?.head}>{snap?.head || "—"}</strong>
        </div>
        {(
          [
            "Refresh",
            "CreateBranch",
            "Checkout",
            "Merge",
            "Stash",
            "PopStash",
          ] as const
        ).map((op, i) => (
          <button
            key={op}
            disabled={!repo || busy || (op === "Refresh" && loading)}
            onClick={() => (op === "Refresh" ? refresh() : review(op))}
          >
            <span>{op === "Refresh" ? "Refresh" : labels[op]}</span>
            <b aria-hidden="true">{["↻", "⑂", "⇄", "⑃", "▣", "▤"][i]}</b>
          </button>
        ))}
        <input
          aria-label="Search history"
          placeholder="Search history…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
      </div>
      {error && (
        <div className="error-banner" role="alert">
          <pre>{notice}</pre>
          <button aria-label="Dismiss error" onClick={() => setError(false)}>
            ×
          </button>
        </div>
      )}
      <main className={diff ? "has-diff" : ""}>
        <nav className={diff ? "rail" : ""} aria-label="Repository navigation">
          <button
            className="back"
            onClick={() => {
              client.invalidate("diff");
              setDiff(undefined);
            }}
            aria-label="Back to history"
          >
            {diff ? "❯" : "◉ Viewing all refs"}
          </button>
          <button
            className={selected === null ? "active" : ""}
            title="Working directory"
            onClick={() => select(null)}
          >
            {diff ? "◇" : `◇ Working directory  ${snap?.status.length ?? 0}`}
          </button>
          {!diff && (
            <>
              <h2>
                LOCAL <small>{snap?.branches.length ?? 0}</small>
              </h2>
              {snap?.branches.map((branch) => (
                <button
                  key={branch}
                  className={`branch ${branch === snap.head ? "active" : ""}`}
                  title={branch}
                  onDoubleClick={() => review("Checkout", branch)}
                  onContextMenu={(e) => {
                    e.preventDefault();
                    review("Checkout", branch);
                  }}
                  onClick={() => {
                    const c = snap.commits.find((c) => c.refs.includes(branch));
                    if (c) select(c.id);
                  }}
                >
                  ⑂ {branch}
                </button>
              ))}
              <h2>TAGS & REFERENCES</h2>
              {[
                ...new Set(
                  snap?.commits.flatMap((c) =>
                    c.refs.filter(
                      (r) => r !== "HEAD" && !snap?.branches.includes(r),
                    ),
                  ),
                ),
              ].map((tag) => (
                <button
                  key={tag}
                  onClick={() => {
                    const c = snap?.commits.find((c) => c.refs.includes(tag));
                    if (c) select(c.id);
                  }}
                >
                  ◇ {tag.replace("tag: ", "")}
                </button>
              ))}
              <h2>STASHES</h2>
              <button
                disabled={!repo || busy}
                onClick={() => review("PopStash")}
              >
                Pop latest stash…
              </button>
            </>
          )}
        </nav>
        {!diff && (
          <div
            className="resizer"
            role="separator"
            aria-label="Resize navigation"
            tabIndex={0}
            onPointerDown={(e) => resize(e, "nav")}
            onKeyDown={(e) => {
              if (e.key.startsWith("Arrow"))
                document.documentElement.style.setProperty(
                  "--nav",
                  `${Math.max(150, Math.min(330, parseInt(getComputedStyle(document.documentElement).getPropertyValue("--nav")) + (e.key === "ArrowRight" ? 10 : -10)))}px`,
                );
            }}
          />
        )}
        <div className="center">
          {repo ? (
            diff ? (
              <Diff
                data={diff}
                split={split}
                setSplit={setSplit}
                busy={busy}
                onClose={() => {
                  client.invalidate("diff");
                  setDiff(undefined);
                }}
                onSource={(staged) => loadDiff(diff.id, diff.path, staged)}
                onStage={() =>
                  void mutate({
                    kind: "Stage",
                    args: { path: diff.path, unstage: diff.staged },
                  })
                }
                onHunk={(index) =>
                  void mutate({
                    kind: "Hunk",
                    args: { token: diff.token, index, unstage: diff.staged },
                  })
                }
              />
            ) : (
              <History
                commits={snap!.commits}
                graph={repo.graph}
                head={snap!.head_id}
                selected={selected}
                onSelect={select}
                search={search}
                limit={limit}
                onMore={() => refresh(limit + 500)}
                workingCount={snap!.status.length}
                onContext={(id, x, y) => setMenu({ id, x, y })}
              />
            )
          ) : (
            <div className="welcome">
              <div className="crab">◉</div>
              <h1>GitCrab</h1>
              <p>Your history. Your working tree.</p>
              <button disabled={opening} onClick={() => void open()}>
                {opening ? "Opening repository…" : "Open a local repository"}
              </button>
              <small>
                {/Mac/.test(navigator.platform) ? "⌘" : "Ctrl"}+O to open · F5
                to refresh
              </small>
            </div>
          )}
        </div>
        {repo && (
          <>
            <div
              className="resizer"
              role="separator"
              aria-label="Resize inspector"
              tabIndex={0}
              onPointerDown={(e) => resize(e, "inspector")}
              onKeyDown={(e) => {
                if (e.key.startsWith("Arrow"))
                  document.documentElement.style.setProperty(
                    "--inspector",
                    `${Math.max(260, Math.min(560, parseInt(getComputedStyle(document.documentElement).getPropertyValue("--inspector")) + (e.key === "ArrowLeft" ? 10 : -10)))}px`,
                  );
              }}
            />
            <aside
              aria-label={selected ? "Commit details" : "Working directory"}
            >
              {selected && commit ? (
                <>
                  <header className="inspector-header">
                    COMMIT <code>{commit.id.slice(0, 7)}</code>
                  </header>
                  <div className="commit-detail">
                    <h1>{commit.subject}</h1>
                    <div className="author">
                      <b>{commit.author.slice(0, 1)}</b>
                      <span>
                        {commit.author}
                        <small>{commit.date}</small>
                      </span>
                    </div>
                    <code className="full-id">{commit.id}</code>
                    {commit.parents.map((p) => (
                      <div key={p} className="parent">
                        <small>parent </small>
                        <code title={p}>{p}</code>
                      </div>
                    ))}
                    <div className="detail-refs">
                      {commit.refs.map((r) => (
                        <span key={r}>{r}</span>
                      ))}
                    </div>
                    <div className="commit-actions">
                      {(
                        ["Checkout", "CherryPick", "Revert"] as Operation[]
                      ).map((op) => (
                        <button
                          disabled={busy}
                          key={op}
                          onClick={() => review(op, commit.id)}
                        >
                          {labels[op]}
                        </button>
                      ))}
                    </div>
                    <h2>CHANGED FILES ({details?.files.length ?? "…"})</h2>
                    {details?.id === selected &&
                      details.files.map((file) => (
                        <button
                          className="changed-file"
                          key={file}
                          title={file}
                          onClick={() => loadDiff(selected, file, false)}
                        >
                          ▤ {file}
                        </button>
                      ))}
                  </div>
                </>
              ) : (
                <>
                  <header className="inspector-header">
                    {snap?.status.length} file changes on <b>{snap?.head}</b>
                  </header>
                  {conflict && (
                    <p className="conflict" role="alert">
                      Resolve conflicts with Git or an external editor, then
                      refresh. Continue or abort the operation externally.
                    </p>
                  )}
                  {files(false)}
                  {files(true)}
                  <section className="commit-editor">
                    <h2>COMMIT MESSAGE</h2>
                    <textarea
                      aria-label="Commit message"
                      placeholder={"Summary\n\nDescription (optional)"}
                      value={draft}
                      onChange={(e) => setDraft(e.target.value)}
                    />
                    <button
                      className="commit-button"
                      disabled={
                        busy || conflict || !staged.length || !draft.trim()
                      }
                      onClick={() =>
                        void mutate({
                          kind: "Commit",
                          args: { message: draft },
                        })
                      }
                    >
                      {busy ? "Working…" : `Commit to ${snap?.head || "HEAD"}`}
                    </button>
                  </section>
                </>
              )}
            </aside>
          </>
        )}
      </main>
      <footer>
        <span>
          {snap
            ? `${snap.commits.length} commits loaded · ${snap.status.length} changed files`
            : "GitCrab 0.1.0"}
        </span>
        <span className={error ? "error" : ""} role="status" title={notice}>
          {busy ? "Working…" : loading ? "Loading…" : notice}
        </span>
      </footer>
      {menu && (
        <div
          className="context-menu"
          role="menu"
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              const buttons = [
                ...e.currentTarget.querySelectorAll<HTMLButtonElement>(
                  "button:not(:disabled)",
                ),
              ];
              const index = buttons.indexOf(
                document.activeElement as HTMLButtonElement,
              );
              buttons[
                (index + (e.key === "ArrowDown" ? 1 : buttons.length - 1)) %
                  buttons.length
              ]?.focus();
            }
          }}
          style={{
            left: Math.min(menu.x, window.innerWidth - 180),
            top: Math.min(menu.y, window.innerHeight - 180),
          }}
        >
          {(["Checkout", "CherryPick", "Revert"] as Operation[]).map((op) => (
            <button
              role="menuitem"
              disabled={busy}
              key={op}
              onClick={() => review(op, menu.id)}
            >
              {labels[op]} {menu.id.slice(0, 7)}
            </button>
          ))}
        </div>
      )}
      {operation && (
        <div className="modal-backdrop">
          <form
            className="operation"
            role="dialog"
            aria-modal="true"
            aria-label={labels[operation.op]}
            onSubmit={(e) => {
              e.preventDefault();
              setOperation(undefined);
              void mutate({ kind: "Operation", args: operation });
            }}
          >
            <h1>{labels[operation.op]}</h1>
            <p className="path">{repo?.path}</p>
            <p>
              Current HEAD: <b>{snap?.head}</b>
            </p>
            {operation.op !== "PopStash" && (
              <label>
                {operation.op === "Stash" ? "Stash message" : "Target"}
                <input
                  autoFocus
                  aria-label="Operation target"
                  value={operation.target}
                  onChange={(e) =>
                    setOperation({ ...operation, target: e.target.value })
                  }
                />
              </label>
            )}
            <p>
              {operation.op === "Checkout" &&
                "Checking out a commit detaches HEAD. Git protects conflicting local changes."}
              {operation.op === "PopStash" &&
                "Apply the latest stash to this working tree and remove it on success."}
              {operation.op === "Merge" &&
                "Merge the target into the current branch. Conflicts require external resolution."}
            </p>
            <div className="dialog-actions">
              <button type="button" onClick={() => setOperation(undefined)}>
                Cancel
              </button>
              <button
                className="primary"
                disabled={
                  busy ||
                  (operation.op !== "PopStash" &&
                    operation.op !== "Stash" &&
                    !operation.target.trim())
                }
              >
                Run {labels[operation.op]}
              </button>
            </div>
          </form>
        </div>
      )}
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
