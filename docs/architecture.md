# Application state and effects

The workspace has three crates. `crates/core` (`gitcrab-core`) holds domain
logic and the Git worker and has no UI dependencies; `crates/ui`
(`gitcrab-ui`) holds the GPUI views and components and depends on core;
`crates/app` (`gitcrab`) is the binary that wires them together. Core tests run
without a window: `cargo test --locked -p gitcrab-core`. Paths below are
relative to each crate's `src/`.

- `git/` owns Git arguments, machine patches, repository identity, and write
  preflight/verification. Write errors carry a phase. Read results never grant
  permission to write through a stale confirmation. No other module passes Git
  arguments. `git/mod.rs` builds and runs commands; `model.rs` holds parsed
  repository types, `read.rs` snapshots and diffs, `write.rs` index and ref
  writes, and `network.rs` fetch, pull and push.
- `process.rs` owns child handles, pipe draining, capture limits, cancellation,
  and read deadlines. Unix readers run in a process group. Windows readers use a
  kill-on-close job. Mutations run to completion and their tabs stay open.
  Fetch, pull and push use the same supervision with no deadline
  (`ReadScope::network`), so cancelling stops Git and its helpers.
- `service.rs` runs those network actions on their own thread beside the
  worker (`NetworkJob`); worker writes are refused while one runs.
- `service.rs` owns one bounded worker per tab. `ReviewedOperation`,
  `PatchCache`, and `HistoryCache` name the review and cache lifetimes.
  The history cache publishes commits and their graph as one value after
  all snapshot reads succeed.
  Starting a write expires the cached patch. A failed write still refreshes.
  Each diff reply carries a `diff::Preview` classification; only text
  previews carry patch lines.
- `journal.rs` owns each tab's in-memory undo and redo stacks. The worker
  records an entry after a successful write, checks the top entries with every
  snapshot (`RepoSnapshot::journal`), and reviews and runs Undo and Redo as
  `Operation::Undo` and `Operation::Redo`, rechecking preconditions first.
- `controller.rs` owns request generations, selection reconciliation, draft
  revisions, and follow-up dispatches. `DiffSelection` and `ReopenDiff` separate
  the user's selection from an automatic reopen request. Completion returns
  dispatches for the desktop layer to apply. `accepts_response` gates dialog,
  input-buffer, focus, and persistence effects before native callbacks run.
  Obsolete replies still release their worker lane and dispatch pending work.
  Native refresh triggers apply cancellation only when the controller
  accepts or queues a replacement refresh.
- `ui/view/operations.rs` owns one-click operations and the inline branch-name bar. A worker resolves and checks the
  target, then the write starts and rechecks it before Git runs.
  `ui/view/repository.rs` owns trust prompts and watcher lifecycle.
  `ui/view/evidence.rs` contains diagnostic navigation.
- `ui/theme.rs` holds design tokens: palette roles (`role_color`, the
  `RoleStyled` helpers `text_role`, `bg_role` and `border_role`), the
  font-scaled size helper `scaled`, and geometry constants. `ui/components/`
  holds the reusable UI built on them: button roles and sizes (`buttons`), the
  `ListRow` used by every list and tree, `Badge`, and pane containers
  (`layout`). Views import them through `components::prelude` and compose
  panes from them instead of styling raw elements.
- `ui/preferences.rs` owns the application appearance source and a
  serialized persistence worker. Closing flushes pending saves. File replacement
  uses an owned temporary file beside the destination.

The Serde request/reply envelope is an internal diagnostic format. GitCrab does
not expose it as a supported external protocol. Its tests continue to exercise
identity and explicit trust defaults.

The desktop still applies focus, dialog, and persistence effects. Moving those
effects into one controller interface is a further slice of M1. Keep its
refresh-ordering, stale-response, and draft-revision tests when moving them.
