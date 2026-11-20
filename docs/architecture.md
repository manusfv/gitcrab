# Application state and effects

The workspace has six crates:

- `crates/core` (`gitcrab-core`): the controller, presentation, diff and graph
  logic, the repository data types, and the request/response protocol with the
  `GitWorker` and `GitBackend` traits. No UI and no I/O; its tests run without
  a window or Git: `cargo test --locked -p gitcrab-core`.
- `crates/git` (`gitcrab-git`): the Git worker that implements those traits:
  `git/`, `process.rs`, `service.rs`, `journal.rs` and `platform.rs` below.
  Its tests run real Git in isolated repositories.
- `crates/settings` (`gitcrab-settings`): preferences, session and trust files
  behind core's `SettingsStore` trait.
- `crates/update` (`gitcrab-update`): update checks behind core's `Updater`
  trait. Reads each channel's release manifest with `cargo-packager-updater`
  on a background thread. Holds `UPDATE_PUBLIC_KEY`.
- `crates/ui` (`gitcrab-ui`): GPUI views and components. Depends on core only
  and reaches Git, settings and updates through the `GitBackend`,
  `SettingsStore` and `Updater` the app installs.
- `crates/app` (`gitcrab`): the binary. Installs `gitcrab_git::Backend`,
  `gitcrab_settings::FileStore` and `gitcrab_update::ManifestUpdater`, then
  opens the window.

`scripts/check-boundaries.sh` enforces this layout: core has no UI, platform or
I/O dependency, the UI depends on no I/O crate, the I/O crates do not depend on
the UI, and core tests pass. CI runs it on every platform.

Paths below are relative to each crate's `src/`.

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
- View rules sit in core so views only render: `sidebar.rs` (reference
  filter, section counts and folder rows), `watch.rs` (which filesystem
  changes matter and how they coalesce), `tabs.rs` (tab reordering),
  `presentation.rs` (dates, commit messages, decorations) and
  `service::validate_new_branch` for the branch-name bar.
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
- `preferences.rs` (core) owns `Preferences` and its rules: clamping on load,
  merging one tab's edits into the shared copy, the recent list, session entries
  and trust matching. It also defines the `SettingsStore` trait.
  `gitcrab-settings` implements it with a serialized writer thread; closing
  flushes pending saves, and file replacement uses an owned temporary file beside
  the destination. `ui/preferences.rs` only keeps the shared copy in a gpui
  global and applies the font size to the theme.
- `updates.rs` (core) owns the update rules: the effective release channel
  (the saved choice, else prerelease for a prerelease build), the daily limit
  on automatic checks, semver comparison and skipped versions. It defines the
  `Updater` trait. `ui/updates.rs` keeps the latest result in a gpui global,
  runs the automatic schedule (10 seconds after startup, then hourly when a
  check is due) and records the skipped version and last check through
  `ui/preferences.rs`. The workspace shows the update notice; the Appearance
  dialog has the Updates group.

The Serde request/reply envelope is an internal diagnostic format. GitCrab does
not expose it as a supported external protocol. Its tests continue to exercise
identity and explicit trust defaults.

The desktop still applies focus, dialog, and persistence effects. Moving those
effects into one controller interface is a further slice of M1. Keep its
refresh-ordering, stale-response, and draft-revision tests when moving them.
