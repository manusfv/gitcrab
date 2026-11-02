# Architecture

GitCrab is a native Rust desktop application. Its Git boundary, graph layout, patch model and presentation are separate modules; proprietary the reference app code, fonts and assets are not dependencies.

## Technology choices

| Choice | Rationale and tradeoff |
| --- | --- |
| `eframe = 0.31.1` / egui, Glow renderer | Exact painter coordinates, clipping, cubic edges and immediate pointer interaction suit the graph. Stock widgets supply text editing, menus, scroll areas and resizable panels. The pinned version's native entry point, renderer and font customization were checked in [official API documentation](https://docs.rs/eframe/0.31.1/eframe/). This version is deliberately pinned rather than presented as current. Custom painting requires explicit focus/accessibility work. |
| Iced and Slint considered | [Iced's application API](https://docs.iced.rs/iced/application/index.html) separates update and view; [Slint](https://docs.slint.dev/latest/docs/slint/) supplies a separate UI language, layouts and Rust integration. They remain plausible alternatives for a larger structured UI. For this implementation, keeping graph geometry and interactions directly in Rust reduced integration work. No comparative runtime benchmark was conducted. |
| Installed Git CLI behind `Git` | Direct subprocess arguments preserve familiar Git porcelain semantics across branch, commit and index operations. This incurs subprocess latency and requires Git on PATH. The observed warm snapshot of the 43-commit fixture was roughly 255ms; this is a small-repository measurement, not a scalability claim. |
| `git2` considered | The [Repository API](https://docs.rs/git2/latest/git2/struct.Repository.html) exposes apply, merge, rebase, stash, index and diff operations. It offers in-process access but adds libgit2/native dependency management and another behavior boundary to validate. |
| `gix` considered | [Official gix documentation](https://docs.rs/gix/latest/gix/) describes object/pack caching and conversion to `ThreadSafeRepository`. It is a useful future in-process history/object backend. Choosing the CLI initially keeps mutation behavior aligned with the installed Git; no claim of gix workflow incompleteness is made. |

`Cargo.toml` disables eframe defaults and enables `default_fonts`, `glow`, `x11` and `wayland`. AccessKit is currently disabled, so platform accessibility and screen-reader support remain incomplete. Available Windows system fonts are read locally; fallback fonts are supplied by egui. PNG output uses the `image` crate. Windows is the only platform built and observed in this task; Linux/macOS packaging and behavior remain unverified.

## Modules and data flow

| Module | Responsibility |
| --- | --- |
| `src/main.rs` | Native window, startup repository argument and minimum size. |
| `src/git.rs` | Repository discovery, snapshots, refs/HEAD/status, first-parent file/diff queries, index patches and local mutations. Every command uses `std::process::Command`, never shell command composition. |
| `src/graph.rs` | Original lane allocation from ordered parent identities. Produces lane/color and center-to-next-row edge segments without UI coordinates. |
| `src/diff.rs` | Unified patch parsing with old/new blob line coordinates; replacement-block pairing for basic split view. |
| `src/ui.rs` | UI state, serial background worker, response reconciliation, panels, custom graph/diff painting, local operation review and screenshot capture. |

The UI sends typed jobs over an MPSC channel. One worker serializes Git operations and owns the current backend. It also computes graph layout and parses/aligns diff lines. Replies wake the UI. Repository-open epochs reject responses from an earlier repository; details and diff replies additionally match the selected commit, file and index/worktree source. Repeated refresh requests are coalesced while one is pending. Mutations refresh state on both success and failure because a failed merge can still leave a conflict state. Commit text is cleared only when the submitted draft still matches the current editor text.

Ignoring stale replies is not process cancellation. Commands have no timeout or cancellation mechanism; slow Git commands or user hooks can delay later jobs in the serial queue. Cancellation, bounded/latest-only read queues and explicit operation lifecycle controls are follow-up work.

## Graph and viewport

Git supplies `--all --date-order` history, including detached HEAD. Parent identities remain the topology source. Layout reserves cyan lane0 for HEAD even when a newer disconnected history appears first or HEAD lies outside the loaded page. First-parent continuation retains color where possible; additional parents get independent lanes, and vacated lanes can be reused. This is an original algorithm constrained by observed appearance; the reference app's algorithm remains unknown.

Rendering converts this model to 28px rows, 22px lanes, 2px edge strokes and clipped cubic joins. A 140px reference column precedes a graph column at least 150px wide. Branch overflow is compact; full identifiers remain available in tooltips/details. The working-tree row opens staging. A fixed history prefix retains its lane geometry when additional ancestors arrive; repository changes can alter Git ordering and therefore layout.

History and diff views use `ScrollArea::show_rows`, drawing the visible rows and necessary incoming edge halves. The application initially requests 500 commits; an explicit button increases the limit by 500 and reloads that bounded prefix. It does not stream arbitrary history or retain a persistent graph index. Search dims nonmatching messages while preserving graph topology. Layout/storage costs still grow with loaded commits and active lanes; no unmeasured large-repository guarantee is implied.

## Diffs, staging and mutations

Commit changes are compared with the first parent; root commits have a separate query path. Working-tree and index diffs are explicitly distinguished. New-file previews use `git diff --no-index`, accepting its difference exit code, after ensuring the canonical preview path stays inside the repository. Binary/mode-only changes retain Git's summaries and remain stageable as files. Text must be valid UTF-8.

File staging uses literal Git pathspecs and `--`; rename staging/unstaging handles both relevant index paths. Individual hunks retain file headers and are applied only to the index with `git apply --cached`, adding `--reverse` to unstage. Git's [apply documentation](https://git-scm.com/docs/git-apply) establishes these index/reverse semantics. A stale patch is allowed to fail visibly; the application does not discard work to force it through.

Unified and basic split presentation share parsed blob line numbers, added/deleted backgrounds and a lightweight keyword/string/comment highlighter. Split pairing is positional within replacement blocks, not semantic alignment. “Dim whitespace” changes display emphasis; it does not run Git's whitespace-ignore algorithms or change the patch being staged. Hunk controls operate on the original complete patch. Line-level staging is absent.

Local operation dialogs display repository, current HEAD and target before execution. Revision inputs reject empty/option-like/NUL values, and branch names are checked with Git. Normal Git protections remain in force: checkout does not force over user changes, merge/revert/cherry-pick errors are surfaced, and conflicts refresh into status. User-configured Git hooks may run as they normally do for these operations. No remote write action is exposed. Conflict continuation/abort and an integrated resolution editor are not yet available; users must complete those states with Git or another tool.

## Maintenance limits

There is no persistent settings layer, filesystem watcher, native directory picker, undo system or application installer yet. Refresh is explicit. Non-UTF-8 filenames/output, very large patches, submodule-specialized UX, signing prompts and hook interaction need further work. The current source structure keeps these concerns behind a small boundary but is not a finished production architecture.
