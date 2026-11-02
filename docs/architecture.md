# Architecture

GitCrab has a framework-independent Rust application library, a small Tauri host and a React/TypeScript WebView frontend. No proprietary application code, icons or fonts are incorporated.

## Versions and boundary

The migration resolved stable Tauri Rust 2.12.5/API and CLI 2.12.4, React/React DOM 19.3.1, Vite 8.3.5, TypeScript 5.9 and ts-rs 12.0.1 against official documentation/registries. Cargo.lock and package-lock.json are authoritative for exact transitive versions. Node 24.22.0 was used locally. Tauri 3 alpha was deliberately excluded.

| Module | Responsibility |
| --- | --- |
| `src/git.rs` | Installed Git CLI with direct arguments, literal pathspecs, repository discovery, history/status, first-parent/root diffs, rename handling and index/local mutations. |
| `src/graph.rs` | Original topology-based lane allocator, reserved HEAD lane, disconnected roots, octopus joins and stable unchanged prefixes. |
| `src/diff.rs` | Blob line coordinates, unified parsing and positional replacement-block split alignment. |
| `src/service.rs` | Typed requests/replies, repository ownership, bounded worker, mutation lifecycle, cached hunk tokens and contract export. |
| `src/platform.rs` | `GITCRAB_GIT`, PATH and macOS GUI-launch Git fallbacks. System UI fonts are selected by CSS. |
| `src/desktop.rs` | Three commands: execute an application request, choose a repository using a native dialog, and obtain the startup repository; the execute command returns typed operation responses. No general shell/filesystem interface. |
| `frontend/contract.ts` | Generated from Rust derives. A Rust regression test fails if the committed TypeScript declarations diverge. |
| `frontend/client.ts` | IPC identity validation, per-view latest-only read lanes and timing measures. |
| `frontend/main.tsx` | Repository/selection state, target review, staging/editor and error/loading state. |
| `frontend/History.tsx` / `Diff.tsx` | Virtual rows and Canvas graph; bounded diff rendering. |

`Service` owns a channel of capacity eight and a dedicated OS thread. Git processes, snapshot/layout and diff parsing run there, never on the event loop or Tokio executor. `try_send` reports a busy error rather than blocking or growing the queue. Async commands await a oneshot response. Mutations are serialized with reads. There is no process cancellation: hooks and slow commands can delay subsequent work.

Each frontend read lane permits one running task and one replaceable pending task. Snapshot, details and diff reads have independent lanes. A repository epoch invalidates responses from older repositories; monotonically increasing sequence IDs invalidate superseded selections/errors. UI mutations disable further mutations and repository opening until completion. The Rust service rejects cross-epoch mutations even if frontend reconciliation fails. Selection changes clear obsolete details/diff controls immediately. This is stale-reply rejection and backlog control, not cancellation.

Mutation completion and failure both refresh state, since Git may enter a conflicted state before reporting failure. Commit drafts clear only when the completed submission still equals the current editor value. Hunk patches never come from frontend input: Rust retains the last parsed diff and accepts a token, hunk index and direction. All mutations invalidate the token, even failures. Git's cached apply remains the final stale-patch check; the worktree is never rewritten by hunk staging.

## Rendering and data

The initial history prefix is 500 commits, expanded explicitly by 500; the service caps requests at 100,000. Snapshots transfer only on open/refresh/expansion, never on hover or animation frames. Files and patches are loaded on selection. React retains immutable snapshot/graph objects between interactions.

History is a single scroll coordinate system with 28 CSS-pixel rows, a 140px refs column and at least 150px graph column. The Canvas sizes its backing store to device pixel ratio. It draws only visible row segments plus incoming-edge overscan; the independent graph model splits edges into adjacent-row segments. DOM rows remain selectable/accessibly named. A WIP row, colored refs, overflow tooltips, HEAD and disconnected/octopus topology are retained. Search changes emphasis without changing geometry. Layout costs still scale with loaded commits/active lanes.

Unified and split diffs render visible 24px rows with overscan. Parsing and positional split pairing happen on the Rust worker and are retained in frontend state. Hunk headers and lexical syntax colors are cached; the split hunk navigator mounts a fixed number of controls. Hunk actions reference the original complete patch. Keyword/string/number/comment colors preserve the former lightweight highlighter, without claiming full language grammars. “Dim whitespace” is display emphasis, not Git whitespace-ignore semantics. Very large files still incur complete UTF-8 patch parsing/IPC/storage; virtualization only bounds rendering.

The 210px navigation and 396px inspector approximate the reference's physical proportions at device scale 1; both are resizable. Below 1000px they default to 160/300px. The diff retains a horizontal scrollbar at narrow widths. CSS tokens define palette, density, borders and focus. DOM controls provide normal text selection, labels and keyboard navigation, but screen-reader and complete keyboard context-menu coverage need more work.

## Security and packaging

Production embeds Vite output in the executable. The CSP restricts scripts/assets to local sources, blocks frames/objects, and permits only Tauri IPC connections. Inline styles are permitted for virtual row geometry, without inline scripts. The main capability grants no plugin filesystem/shell/network permissions. The native folder dialog runs in a blocking worker. Application commands accept typed operations rather than arbitrary Git arguments. Repository hooks still run under normal local Git configuration.

The optional `native-smoke` feature adds a loopback-only embedded WebDriver server to debug test builds for all native WebViews. It is opt-in, and a compile error prevents combining it with release builds. CI compiles production packages separately without it. Windows production E2E enables CDP only through its test process environment; application code never starts a debugger endpoint.

Useful history is retained in Git at egui baseline `81a73dc`; the superseded immediate-mode UI and dependencies are removed. Historical renderer/performance evidence is clearly separated from new Tauri evidence.

References: [Tauri commands](https://v2.tauri.app/develop/calling-rust/), [CSP](https://v2.tauri.app/security/csp/), [capabilities](https://v2.tauri.app/security/capabilities/), [Vite setup](https://vite.dev/guide/), [React versions](https://react.dev/versions), [native WebDriver testing](https://v2.tauri.app/develop/tests/webdriver/).
