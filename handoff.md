# Handoff: bulk task-list actions

## Stop point

The user asked to stop here. Do not continue implementation in this session.

The requested feature is: task-list `.` quick-menu and direct actions must apply to the transient Ctrl/Shift multi-selection. Calendar and task-detail actions remain single-task.

The last UI mason was cancelled when OpenAI servers went down. It wrote substantial changes, but did not return a completion report and its work has **not been verified as an integrated build**. Treat all current uncommitted changes as valuable but unverified. Do not reset, discard, or overwrite them.

## Mandatory project constraints

- Read `AGENTS.md`, `architecture.md`, and `~/dev/tuicore/SKILL.md` before work.
- `TuidoService` owns persistence. TUI adapters must not issue SQL.
- Persisted mutations use optimistic revisions. Bulk writes must be atomic and bump workspace revision once.
- Do not refresh over pending optimistic writes.
- Never touch unrelated concurrent edits without confirming intent.

## Current changes

### Tuicore (`/home/marlo/dev/tuicore`)

Completed mason work:

- `src/components/list_control.rs`: adds `ListControl::transient_selected_ids() -> Vec<Id>`.
  - Returns current flat/tree transient selection in display/source order.
  - Does not mutate selection; `data_view_mut()` remains destructive as before.
- `src/components/list_control/tests.rs`: adds Shift-range and sparse Ctrl-selection ordering assertions through that public API.

Prior verification from that mason:

```text
cargo test list_control        # 75 passed
cargo check --tests            # passed; 4 existing warnings
```

Other Tuicore changes in Calendar/DataView files were already in the working tree. Preserve them.

### Tuido persistence (`/home/marlo/dev/tuido`)

Completed mason work:

- `src/service.rs`
  - Adds internal `TaskMutationTarget { id, expected_revision }`.
  - Adds `bulk_patch_tasks(targets, patch)` for Snooze plus explicit Todo/InProgress/Done/Rejected state targets.
  - Adds `delete_tasks(targets)`.
  - Claims every target revision before writes, uses one transaction, and bumps workspace once.
  - Bulk InProgress keeps selected rank order while promoting the selected block above active tasks.
- `src/persistence_coordinator.rs`
  - Adds `BulkPatchTasks` and `BulkDeleteTasks` commands.
  - Captures current target revisions when execution starts.
  - Restores optimistic snapshots on failure and commits/removes revisions on success.
- `src/domain.rs`, `src/service/tests.rs`, and `src/persistence_coordinator/tests.rs`
  - Add reducer and behavior coverage for these paths.

Prior verification from that mason:

```text
cargo check --lib              # passed
# targeted bulk tests passed
cargo test --lib               # two pre-existing app-test failures reported:
# task_creation_from_notes...
# task_header_shows_new...
```

### Tuido UI bulk work — unverified/cancelled mason

The cancelled mason changed these files and appears to have implemented most of the UI integration:

- `src/app.rs`
  - Adds plural `AppMsg` variants such as `OpenTasksQuickMenu`, `OpenTasksSnooze`, `OpenDeleteTasks`, `OpenCompleteTasks`, `CompleteTasks`, `SnoozeTasks`, `UnsnoozeTasks`, and block edge-move messages.
  - Adds bulk menu/dialog handlers, optimistic bulk persistence submission, plural notifications, helpers for ordered task blocks, and table selection snapshotting.
  - Relevant selection use is near `src/app.rs:3902`; it calls `task_list().transient_selected_ids()` and emits `OpenTasksQuickMenu` near `:3935`.
- `src/task_quick_menu.rs`
  - Adds multi-target menu construction.
  - Bulk menu appears to include references, explicit Todo/InProgress, snooze, delete, complete/reject, and edge moves.
  - Generated execute/clarify commands remain single-task.
- `src/snooze.rs`
  - Adds `SnoozeTarget::Tasks { task_ids, all_snoozed }`.
  - Unsnooze is offered only when all selected tasks are snoozed.
- `src/app/dialogs.rs`
  - Adds plural delete and complete/reject dialog builders.
- `src/app/tests_workspace.rs` and `src/app/tests.rs`
  - Contain new test edits from this cancelled mason; inspect them before changing tests.

## Expected behavior to preserve

- Snapshot the ListControl transient selection when table actions start. Use the highlighted visible task only when no transient selection exists.
- Do not use `data_view_mut()` to read selection.
- Multi state operations are explicit target states. Never independently toggle every task.
- Multi references copy as newline-separated references.
- Generated execute/clarify commands, calendar actions, task-detail edits, and task-relation edits stay single-task.
- One delete confirmation covers the selected group.
- One complete/reject dialog chooses Done or Rejected for the group.
- Snooze one group to one time. Unsnooze is available only when all targets are snoozed.
- Move selected IDs as a block in visible/source order: prepend for top, append for bottom.
- Bulk writes go through the existing coordinator commands. On failure, rely on the coordinator snapshot restore and visible refresh error.
- Preserve focus and calendar behavior for existing single-task actions.

## Working-tree state

Tuido has uncommitted modifications in:

```text
src/app.rs
src/app/dialogs.rs
src/app/tests.rs
src/app/tests_workspace.rs
src/calendar.rs
src/domain.rs
src/persistence_coordinator.rs
src/persistence_coordinator/tests.rs
src/service.rs
src/service/tests.rs
src/snooze.rs
src/task_quick_menu.rs
```

Tuicore has uncommitted modifications in:

```text
src/components/calendar/tests.rs
src/components/data_view/layout.rs
src/components/data_view/render.rs
src/components/data_view/tests.rs
src/components/list_control.rs
src/components/list_control/reorder.rs
src/components/list_control/tests.rs
```

`git diff --check` was clean before the cancelled UI mason. Run it again before editing.

## Recommended restart sequence

1. Inspect `git status`, `git diff --check`, and every modified file before editing. The cancelled mason modified `app.rs`, `app/dialogs.rs`, `snooze.rs`, `task_quick_menu.rs`, and tests, so previous context is stale.
2. Delegate separate review/verification lanes to multiple agents:
   - UI/message/menu/dialog integration review and focused compile/test repair.
   - Persistence/revision/rollback review of the bulk service/coordinator work.
   - Behavioral test review for transient Shift/Ctrl selection, action wording, state transitions, delete rollback, snooze/unsnooze, and block rank order.
3. Run focused tests first, then `cargo check --tests` and the relevant full Tuido test suite. Investigate any failures; do not assume the previous two full-suite failures remain unchanged.
4. Ask an oracle for a final review after tests pass, with special attention to optimistic rollback, stale targets, and UI focus selection stability.

No commit was created.
