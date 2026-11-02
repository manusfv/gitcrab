import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { Commit, Graph } from "./contract";
export const ROW = 28,
  REFS = 140;
export const colors = [
  "#15a0bf",
  "#0669f7",
  "#8e00c2",
  "#c517b6",
  "#d90171",
  "#cd0101",
  "#f25d2e",
  "#f2ca33",
  "#7bd938",
  "#2ece9d",
];
export function History({
  commits,
  graph,
  head,
  selected,
  onSelect,
  search,
  onContext,
  onMore,
  limit,
  workingCount,
}: {
  commits: Commit[];
  graph: Graph;
  head: string;
  selected: string | null;
  onSelect: (id: string | null) => void;
  search: string;
  onContext: (id: string, x: number, y: number) => void;
  onMore: () => void;
  limit: number;
  workingCount: number;
}) {
  const viewport = useRef<HTMLDivElement>(null),
    canvas = useRef<HTMLCanvasElement>(null);
  const [position, setPosition] = useState({
    top: 0,
    left: 0,
    height: 500,
    width: 600,
    dpr: devicePixelRatio,
  });
  const graphWidth = Math.max(150, graph.lanes * 22 + 45);
  const graphStart = Math.max(0, Math.min(graphWidth, position.left - REFS));
  const visibleGraphWidth = Math.max(
    0,
    Math.min(graphWidth, position.left + position.width - REFS) - graphStart,
  );
  useLayoutEffect(() => {
    const el = viewport.current!;
    const observer = new ResizeObserver(() =>
      setPosition({
        top: el.scrollTop,
        left: el.scrollLeft,
        height: el.clientHeight,
        width: el.clientWidth,
        dpr: devicePixelRatio,
      }),
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const start = Math.max(0, Math.floor(position.top / ROW) - 3),
    end = Math.min(
      commits.length,
      Math.ceil((position.top + position.height) / ROW) + 3,
    );
  useEffect(() => {
    const c = canvas.current!,
      ctx = c.getContext("2d")!;
    const dpr = position.dpr;
    c.width = Math.max(1, visibleGraphWidth * dpr);
    c.height = position.height * dpr;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, visibleGraphWidth, position.height);
    if (!visibleGraphWidth) return;
    ctx.lineWidth = 2;
    const x = (lane: number) => 35 + lane * 22 - graphStart,
      y = (row: number) => (row + 1) * ROW + ROW / 2 - position.top;
    // Every edge spans a single row in the backend model. Include the preceding
    // row so incoming curves survive viewport clipping.
    for (
      let i = Math.max(0, start - 2);
      i < Math.min(graph.rows.length, end + 1);
      i++
    ) {
      const row = graph.rows[i];
      for (const edge of row.edges) {
        if (
          Math.max(x(edge.from_lane), x(edge.to_lane)) < -2 ||
          Math.min(x(edge.from_lane), x(edge.to_lane)) > visibleGraphWidth + 2
        )
          continue;
        ctx.strokeStyle = colors[edge.color % 10];
        ctx.beginPath();
        ctx.moveTo(x(edge.from_lane), y(i));
        ctx.bezierCurveTo(
          x(edge.from_lane),
          y(i) + ROW * 0.65,
          x(edge.to_lane),
          y(i) + ROW * 0.35,
          x(edge.to_lane),
          y(i + 1),
        );
        ctx.stroke();
      }
    }
    for (let i = start; i < end; i++) {
      const row = graph.rows[i];
      if (!row) continue;
      if (commits[i].refs.length) {
        ctx.strokeStyle = colors[row.color % 10];
        ctx.beginPath();
        ctx.moveTo(-graphStart, y(i));
        ctx.lineTo(x(row.lane), y(i));
        ctx.stroke();
      }
      if (x(row.lane) < -12 || x(row.lane) > visibleGraphWidth + 12) continue;
      ctx.strokeStyle = colors[row.color % 10];
      ctx.fillStyle = "#1c1e23";
      ctx.beginPath();
      ctx.arc(
        x(row.lane),
        y(i),
        commits[i].parents.length > 1 ? 5 : 9,
        0,
        Math.PI * 2,
      );
      ctx.fill();
      ctx.stroke();
      if (commits[i].parents.length <= 1) {
        ctx.fillStyle = colors[row.color % 10];
        ctx.font = "10px system-ui";
        ctx.textAlign = "center";
        ctx.fillText(commits[i].author.slice(0, 1), x(row.lane), y(i) + 3.5);
      }
    }
    const headRow = commits.findIndex((c) => c.id === head);
    if (position.top < ROW * 2) {
      ctx.strokeStyle = colors[0];
      ctx.setLineDash([3, 3]);
      ctx.beginPath();
      ctx.arc(x(0), ROW / 2 - position.top, 9, 0, Math.PI * 2);
      ctx.stroke();
      ctx.setLineDash([]);
      if (headRow >= 0) {
        ctx.beginPath();
        ctx.moveTo(x(0), ROW / 2 + 9 - position.top);
        ctx.lineTo(x(0), Math.min(y(headRow), position.height));
        ctx.stroke();
      }
    }
  }, [
    graph,
    commits,
    start,
    end,
    position,
    graphStart,
    visibleGraphWidth,
    head,
  ]);
  function navigate(key: string) {
    const current =
      selected === null ? -1 : commits.findIndex((c) => c.id === selected);
    const next = Math.max(
      -1,
      Math.min(commits.length - 1, current + (key === "ArrowDown" ? 1 : -1)),
    );
    onSelect(next < 0 ? null : commits[next].id);
    const el = viewport.current!,
      top = (next + 1) * ROW;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW > el.scrollTop + el.clientHeight)
      el.scrollTop = top + ROW - el.clientHeight;
  }
  return (
    <section className="history" aria-label="Commit history">
      <div className="history-header-clip">
        <div
          className="history-header"
          style={{
            gridTemplateColumns: `${REFS}px ${graphWidth}px 1fr`,
            width: Math.max(position.width, REFS + graphWidth + 180),
            transform: `translateX(${-position.left}px)`,
          }}
        >
          <span>BRANCH / TAG</span>
          <span>GRAPH</span>
          <span>COMMIT MESSAGE</span>
        </div>
      </div>
      <div
        className="history-viewport"
        ref={viewport}
        tabIndex={0}
        role="listbox"
        aria-label="History, use arrow keys to select"
        aria-activedescendant={selected ? `commit-${selected}` : "working-row"}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            navigate(e.key);
          }
          if (
            selected &&
            (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10"))
          ) {
            e.preventDefault();
            const bounds = viewport.current!.getBoundingClientRect();
            onContext(selected, bounds.left + 100, bounds.top + 60);
          }
        }}
        onScroll={(e) => {
          const el = e.currentTarget;
          setPosition((p) => ({
            ...p,
            top: el.scrollTop,
            left: el.scrollLeft,
            dpr: devicePixelRatio,
          }));
        }}
      >
        <div
          className="history-space"
          style={{
            height: (commits.length + 1) * ROW + 36,
            minWidth: REFS + graphWidth + 180,
          }}
        >
          <div
            id="working-row"
            role="option"
            aria-selected={selected === null}
            className={`history-row wip ${selected === null ? "selected" : ""}`}
            style={{
              top: 0,
              gridTemplateColumns: `${REFS}px ${graphWidth}px 1fr`,
            }}
            onClick={() => onSelect(null)}
          >
            <span />
            <span />
            <em>WIP · {workingCount} changes in working directory</em>
          </div>
          {commits.slice(start, end).map((commit, offset) => {
            const i = start + offset,
              row = graph.rows[i],
              color = colors[(row?.color ?? 0) % 10];
            return (
              <div
                id={`commit-${commit.id}`}
                key={commit.id}
                role="option"
                aria-selected={selected === commit.id}
                className={`history-row ${selected === commit.id ? "selected" : ""} ${search && !`${commit.subject} ${commit.author} ${commit.id} ${commit.refs.join(" ")}`.toLowerCase().includes(search.toLowerCase()) ? "dim" : ""}`}
                style={{
                  top: (i + 1) * ROW,
                  gridTemplateColumns: `${REFS}px ${graphWidth}px 1fr`,
                }}
                onClick={() => onSelect(commit.id)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  onSelect(commit.id);
                  onContext(commit.id, e.clientX, e.clientY);
                }}
              >
                <span
                  className="refs"
                  title={commit.refs.join("\n")}
                  style={{ color }}
                >
                  {commit.refs.length > 0 && (
                    <>
                      <span
                        className="ref-pill"
                        style={{ background: `${color}33` }}
                      >
                        {commit.id === head ? "✓ " : ""}
                        {commit.refs.filter((r) => r !== "HEAD")[0] ?? "HEAD"}
                      </span>
                      {commit.refs.filter((r) => r !== "HEAD").length > 1 && (
                        <small>
                          +{commit.refs.filter((r) => r !== "HEAD").length - 1}
                        </small>
                      )}
                    </>
                  )}
                </span>
                <span />
                <span
                  className="subject"
                  title={`${commit.subject}\n${commit.author}\n${commit.id}`}
                >
                  <span>{commit.subject}</span>
                  <code>{commit.id.slice(0, 7)}</code>
                </span>
              </div>
            );
          })}
          <div
            className="history-bottom"
            style={{ top: (commits.length + 1) * ROW }}
          >
            {commits.length} commits{" "}
            {commits.length >= limit && limit < 100_000 && (
              <button onClick={onMore}>Load 500 more</button>
            )}
          </div>
        </div>
        <canvas
          ref={canvas}
          className="graph-canvas"
          aria-hidden="true"
          style={{
            left: REFS + graphStart,
            width: visibleGraphWidth,
            height: position.height,
            top: position.top,
          }}
        />
      </div>
    </section>
  );
}
