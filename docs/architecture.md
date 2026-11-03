# Architecture

The default host is GPUI, with an independent Rust application boundary. Dependency provenance and platform risks are recorded in the dependency decision.

| Module | Responsibility |
| --- | --- |
| `src/git.rs` | Direct Git subprocess arguments, literal paths, repository boundaries, refs/status/diffs, index-only hunks and local mutations. |
| `src/graph.rs` | Original topology-based lane layout, HEAD reservation, disconnected roots, octopus joins and unchanged-prefix stability. |
| `src/diff.rs`, `src/highlight.rs` | Original blob coordinates, positional split alignment, UTF-8 safe bounded lexical spans, unchanged patch text. |
| `src/service.rs` | Typed operations and replies; one dedicated worker; bounded queue of eight; repository epochs, mutation serialization and backend-issued cached hunk tokens. |
| `src/controller.rs` | Framework-independent native state; one active and one replaceable pending read per snapshot/details/diff lane; latest-sequence/epoch reconciliation, draft preservation, mutation refresh and obsolete action clearing. |
| `src/native/main.rs` | Native application/window, platform actions, menu definitions, shortcuts, component initialization and assets. |
| `src/native/view.rs` | GPUI entity, worker adapter, virtual history/diff/status/file rows, visible custom graph painting, native input/menus/dialogs, staging and commit interactions. |
| `src/native/theme.rs`, `preferences.rs`, `assets.rs` | Semantic styling, bounded appearance preferences and original embedded GitCrab assets. |
| `src/platform.rs` | Git executable lookup including macOS GUI-launch fallbacks and `GITCRAB_GIT`. |

Git, layout, patch parsing, split alignment and lexical highlighting execute on the dedicated worker, not on the UI thread. The adapter awaits Tokio's runtime-independent oneshot Future on GPUI's executor: it creates no Tokio runtime and uses no Tokio timer/I/O/reactor. Completion updates use a weak entity plus the window context, so closed windows safely reject late updates. Dropped receivers do not stop the worker. No blocking Git call or receive runs on GPUI's event thread.

Reads are coalesced before the bounded service queue. Selection/repository changes invalidate old details/diff actions immediately. Mutations are serialized, cross-repository stale requests are rejected by the service, and hunk actions carry backend tokens rather than patch text. Refresh follows successful and failed mutations because Git can leave conflicts after an error. The controller only clears a committed draft if no newer edit replaced it. Operation dialogs also recheck the repository epoch at confirmation.

History references, edges, nodes and subjects share a uniform-list scroll coordinate. The retained graph model changes on data refresh, not hover/scroll. Each visible row paints outgoing and incoming edge halves under clipping; work remains proportional to visible rows/edges. Large lane sets can scroll horizontally. Diff rows are virtualized with shared horizontal scrolling; source line coordinates and the original full patch remain authoritative. Selection is an explicit line range independent of mounted rows, with copy/select-all and drag/Shift-click. Per-line lexical spans are prepared once by the worker; display/shaping is bounded to an 8,192-byte UTF-8 boundary, while copying preserves the original line. Large patches and split storage are still retained in memory in full.

Appearance settings use platform configuration directories and explicit saving on GPUI's background executor. Values are clamped and invalid settings fall back to defaults. `GITCRAB_SETTINGS` selects an isolated file for validation.

`native-evidence` is an explicit, separate build feature. It enables deterministic view selection, a one-shot readiness file and bounded synthetic animation measurements. It exposes no network/control endpoint and is excluded from development packages. Production code has no application polling or forced continuous repaint loop. Component/platform internals still consume measurable idle CPU.

GPUI is the sole host; the runnable Tauri baseline remains in Git history. Tauri/React/WebView configuration, dependencies and smoke scripts have been removed. The original `gitcrab_core` boundary has no GPUI types; `cargo test --no-default-features --lib` verifies that property. ts-rs and its frontend-generation test were replaced by a Rust serialization round-trip regression. serde remains for typed data/preferences/evidence, and Tokio sync remains necessary for the runtime-independent worker adapter.

The design overhaul splits inspector/composer, diff review and Appearance into src/native/view modules. Review context persists independently of loaded patch bytes; refresh reconciles the current path and side against the new snapshot. Native notifications use a bounded async channel and an opt-in validation-only trace module. Appearance removes covered editors from the focus order and blocks mouse events; user preferences preserve independent UI/code density, widths, branch groups and recent paths. See the implementation validation for measured behavior and remaining accessibility/platform gaps.
