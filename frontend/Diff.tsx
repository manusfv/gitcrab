import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Reply, Line } from "./contract";
import { highlight } from "./highlight";
export type DiffData = Extract<Reply, { kind: "Diff" }>["data"];
const highlightCache = new WeakMap<Line, ReturnType<typeof highlight>>();
function code(line: Line | null | undefined) {
  if (!line) return "";
  return line.kind === "@" ? line.text : line.text.slice(1);
}
function Source({
  line,
  dim,
}: {
  line: Line | null | undefined;
  dim: boolean;
}) {
  let tokens = line && highlightCache.get(line);
  if (line && !tokens && line.kind !== "@") {
    tokens = highlight(code(line));
    highlightCache.set(line, tokens);
  }
  return (
    <code
      className={`${
        line?.text
          .trimStart()
          .replace(/^[-+ ]/, "")
          .trimStart()
          .startsWith("//")
          ? "comment"
          : ""
      } ${dim ? "whitespace" : ""}`}
    >
      {line?.kind === "@"
        ? code(line)
        : tokens?.map((token, i) => (
            <span key={i} className={token.kind}>
              {token.text}
            </span>
          ))}
    </code>
  );
}
export function Diff({
  data,
  split,
  setSplit,
  onClose,
  onSource,
  onStage,
  onHunk,
  busy,
}: {
  data: DiffData;
  split: boolean;
  setSplit: (v: boolean) => void;
  onClose: () => void;
  onSource: (staged: boolean) => void;
  onStage: () => void;
  onHunk: (index: number) => void;
  busy: boolean;
}) {
  const [dim, setDim] = useState(false),
    [view, setView] = useState({ top: 0, height: 500 });
  const [activeHunk, setActiveHunk] = useState(0);
  const ref = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const el = ref.current!;
    const ob = new ResizeObserver(() =>
      setView({ top: el.scrollTop, height: el.clientHeight }),
    );
    ob.observe(el);
    return () => ob.disconnect();
  }, []);
  useLayoutEffect(() => {
    ref.current!.scrollTop = 0;
    setView((v) => ({ ...v, top: 0 }));
    setActiveHunk(0);
  }, [data.token]);
  const length = split ? data.split.length : data.lines.length,
    start = Math.max(0, Math.floor(view.top / 24) - 3),
    end = Math.min(length, Math.ceil((view.top + view.height) / 24) + 3);
  const hunkIndices = useMemo(() => {
    const indices = new Map<number, number>();
    let h = -1;
    data.lines.forEach((line, index) => {
      if (line.text.startsWith("@@ ")) indices.set(index, ++h);
    });
    return indices;
  }, [data.lines]);
  const splitHeaders = useMemo(
    () =>
      data.split.flatMap(([line], index) =>
        line?.text.startsWith("@@ ") ? [index] : [],
      ),
    [data.split],
  );
  function goHunk(index: number) {
    setActiveHunk(index);
    ref.current!.scrollTop = (splitHeaders[index] ?? 0) * 24;
  }
  return (
    <section className="diff-view" aria-label={`Diff for ${data.path}`}>
      <div className="diff-title">
        <strong title={data.path}>{data.path}</strong>
        <span>
          {data.id
            ? `Commit ${data.id.slice(0, 7)}`
            : data.staged
              ? "Index"
              : "Working tree"}
        </span>
        {!data.id && (
          <button disabled={busy} onClick={onStage}>
            {data.staged ? "Unstage" : "Stage"} file
          </button>
        )}
        <button onClick={onClose} aria-label="Close diff">
          ×
        </button>
      </div>
      <div className="diff-controls">
        {!data.id && (
          <>
            <button
              className={!data.staged ? "active" : ""}
              onClick={() => onSource(false)}
            >
              Unstaged
            </button>
            <button
              className={data.staged ? "active" : ""}
              onClick={() => onSource(true)}
            >
              Staged
            </button>
          </>
        )}
        <label>
          <input
            type="checkbox"
            checked={split}
            onChange={(e) => setSplit(e.target.checked)}
          />
          Split view
        </label>
        <label>
          <input
            type="checkbox"
            checked={dim}
            onChange={(e) => setDim(e.target.checked)}
          />
          Dim whitespace
        </label>
      </div>
      {split && data.hunks.length > 0 && (
        <div className="split-hunks">
          <button
            aria-label="Previous hunk"
            disabled={activeHunk === 0}
            onClick={() => goHunk(activeHunk - 1)}
          >
            ↑
          </button>
          <button
            aria-label="Next hunk"
            disabled={activeHunk >= data.hunks.length - 1}
            onClick={() => goHunk(activeHunk + 1)}
          >
            ↓
          </button>
          <button
            title={data.hunks[activeHunk]}
            disabled={busy}
            onClick={() => onHunk(activeHunk)}
          >
            {data.staged ? "Unstage" : "Stage"} hunk {activeHunk + 1} of{" "}
            {data.hunks.length}
          </button>
        </div>
      )}
      <div
        className="diff-scroll"
        ref={ref}
        onScroll={(e) =>
          setView((v) => ({ ...v, top: e.currentTarget.scrollTop }))
        }
      >
        <div
          style={{
            height: length * 24,
            minWidth: split ? 700 : 500,
            position: "relative",
          }}
        >
          {Array.from({ length: end - start }, (_, offset) => {
            const i = start + offset;
            if (split) {
              const [left, right] = data.split[i];
              return (
                <div
                  key={i}
                  className="diff-line split"
                  style={{ top: i * 24 }}
                >
                  <span className={`split-cell kind-${left?.kind}`}>
                    <small>{left?.old}</small>
                    <Source line={left} dim={dim} />
                  </span>
                  <span className={`split-cell kind-${right?.kind}`}>
                    <small>{right?.new}</small>
                    <Source line={right} dim={dim} />
                  </span>
                </div>
              );
            }
            const line = data.lines[i],
              hi = hunkIndices.get(i);
            return (
              <div
                key={i}
                className={`diff-line kind-${line.kind}`}
                style={{ top: i * 24 }}
              >
                <small>{line.old}</small>
                <small>{line.new}</small>
                <Source line={line} dim={dim} />
                {hi !== undefined && data.hunks[hi] && (
                  <button disabled={busy} onClick={() => onHunk(hi)}>
                    {data.staged ? "Unstage" : "Stage"} hunk
                  </button>
                )}
              </div>
            );
          })}
        </div>
        {length === 0 && <p className="empty">No changes in this source.</p>}
      </div>
    </section>
  );
}
