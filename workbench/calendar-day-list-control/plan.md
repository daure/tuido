# Plan: Calendar Day ListControl Composition

> Source: confirmed conversation requirements and the two ListControl-composition research passes on 2026-08-29.

---

## Goal

Replace Calendar Day's private `DataView<CalendarDayRow, usize>` interaction surface with a
read-only, scoped `ListControl` composition. Calendar Day must reuse ListControl's flat
selection, block movement, placeholder, staging, conflict, commit, and cancel behavior rather
than maintain parallel versions.

Calendar remains the owner of date navigation, Month/Week rendering, calendar public events,
same-time scope definition, and the stable outer `FocusId("calendar")` contract.

## Non-negotiable contracts

- The Day list must never expose add, edit, remove, creator, form, panel, filter, or action-bar
  behavior.
- `ListControl` owns Day flat selection and scoped flat reorder. Calendar must not maintain a
  second transient selection or reorder state for Day rows.
- A Day reorder remains confined to Calendar's `reorder_group` predicate. Tuido's current exact
  `snoozed_until` equality scope must remain intact.
- Calendar emits the existing scoped `CalendarTypedEvent::EntriesReordered { entry_ids }` event;
  it must not expose `ListControlEvent` or a whole-day reorder list.
- Day-row interaction identity must remain stable across `Calendar::set_entries`. Do not use the
  temporary source `entry_index` as a ListControl row ID.
- Keep Calendar's public `Id: Clone + Eq` compatibility unless a deliberate semver decision
  explicitly accepts a new `Hash` bound.
- Keep `Calendar::on_key`, direct event behavior, Month/Week/Day switching, outer Calendar
  focus path, Tuido dialog return focus, and task persistence boundary compatible.
- No schema, migration, service, SQL, or coordinator-domain change is required.

---

## Phase 1: Display-only ListControl can host a Day surface

- [x] 1.1 — Add a display-only ListControl construction path

  **Implementation:** In `../tuicore/src/components/list_control.rs` and its node/event modules,
  add `ListControl::display(rows, row_id)` or an equivalent explicit display-only mode. It must
  build no fields or creator callback, suppress add/add-child/edit/remove/editor/confirmation
  paths, and retain `DataView` rendering, row replacement, selection, focus styling, typed event
  draining, and lifecycle support. Keep existing constructors and behavior unchanged.

  **Done when:** A display-only control renders rich rows without ListControl chrome or mutable
  commands; all existing ListControl tests pass unchanged.

- [x] 1.2 — Add safe embedding controls for Calendar Day

  **Implementation:** Add narrow APIs/builders to display-only ListControl for panel-less layout,
  headers/filter/action-bar suppression, rich wrapped row content, externally supplied semantic
  navigation bindings, inherited focused state, and internal DataView overlay application. Do not
  expose `data_view_mut()` as Calendar's normal integration path.

  **Done when:** A host can lay out and render an embedded ListControl without registering a new
  public tab stop, while keeping the host's focus identity and focus styling.

- [x] **Checkpoint** — Run the ListControl display example/test harness and verify a rich,
  panel-less read-only list has no add/edit/remove affordances or reachable mutation keys.

---

## Phase 2: ListControl supports scoped flat selection and reorder

- [x] 2.1 — Add an explicit flat reorder scope to ListControl

  **Implementation:** Extend `../tuicore/src/components/list_control/reorder.rs` and public
  configuration with a scoped flat reorder predicate, e.g.
  `.reorderable_by_scoped(column, same_scope)`. The selected/highlighted row determines the
  ordered scope. Constrain Shift/Ctrl range extension, sparse selection, single and block move,
  Up/Down/Page/Home/End/GG target movement, placeholder placement, staging, compatibility checks,
  commit, and cancel to that scope. Preserve current full-visible-list behavior when no scope is
  configured.

  **Done when:** A scoped block cannot cross a scope boundary, the `Moving N` marker advances one
  visual boundary per command, and `ListControlEvent::Reordered` contains only ordered IDs in the
  active scope.

- [x] 2.2 — Make sparse block movement visual-boundary correct

  **Implementation:** Reuse or relocate ListControl's existing `visual_target_index` and
  `move_block_visual_boundary` logic so source order, logical insertion target, and placeholder
  marker are derived from the same boundary. Cover contiguous and sparse Ctrl selections. Do not
  copy this logic into Calendar.

  **Done when:** For every movement key, the placeholder moves exactly one displayed position in
  the shown direction. Enter commits the staged order indicated by that marker.

- [x] 2.3 — Add semantic navigation binding overrides

  **Implementation:** Let an embedding host provide line/page/top/bottom/activate/reorder semantic
  bindings to display-only ListControl. Keep ListControl's existing defaults for ordinary users.
  Calendar will supply its `CalendarKeyBindings` in Phase 3 instead of accepting unrelated global
  DataView defaults.

   **Done when:** Custom host bindings drive all Day row navigation and reorder actions without
   changing existing ListControl keybinding behavior.

- [A][x] 2.4 — Correct scoped visual boundaries and complete host key semantics

  **Spec:** Scoped block movement must advance one displayed boundary at a time in interleaved
  scopes. Host bindings must cover Calendar's existing line/page/top/bottom/activate/reorder
  semantics, including two-key top navigation and reorder-key commit.

  **Design:** Keep full displayed order as the source of visual placeholder placement. Derive the
  scoped logical insertion point from that boundary. Keep the consumer-facing binding API compact;
  internal pending-prefix and staged-order details remain private.

  **Implementation:** Track scoped block visual boundaries against full displayed order. Reconcile
  refreshed transient selections to the surviving anchor's scope. Require a configured reorder
  capability before intercepting flat range selection. Let `reorderable_by` clear a prior scope.
  Extend display bindings for top-prefix and active-reorder commit behavior. Add direct and routed
  tests for interleaved sparse/contiguous movement, marker-to-commit parity, `g g`, and custom
  reorder commit.

  **Done when:** Interleaved scoped block movement advances exactly one displayed row boundary per
  keypress, the committed order matches the marker, host bindings preserve Calendar semantics, and
  ordinary ListControls retain existing selection behavior.

- [x] **Checkpoint** — In a scoped ListControl test/example with mixed scopes and sparse selected
  rows, verify Up/Down never crosses scope, the marker has no jump, Escape restores the original
  order, and Enter emits only the active scope IDs.

---

## Phase 3: Calendar Day composes the scoped ListControl

- [x] 3.1 — Introduce a private CalendarDayList adapter with stable interaction IDs

  **Implementation:** Add `../tuicore/src/components/calendar/day.rs`. Move `CalendarDayRow` and
  Day-list projection code out of `calendar/mod.rs`. The adapter wraps
  `ListControl<CalendarDayRow, CalendarDayRowKey, M>` in display-only scoped mode. Use a private
  stable-key registry that maps Calendar external `Id` values to `CalendarDayRowKey` values across
  `set_entries`; prune deleted IDs. This keeps Calendar's public `Id: Clone + Eq` bound unchanged.
  Keep `entry_index` only as a current-render lookup, never as a persisted interaction identity.

  **Done when:** Refreshing entries preserves a valid Day highlight/selection/reorder state by
  external Calendar ID; replacing entries with reused source indices cannot select a different
  entry.

- [x] 3.2 — Route Day interaction through CalendarDayList and translate events

  **Implementation:** In `../tuicore/src/components/calendar/mod.rs`, replace `day_entries:
  DataView<...>` with the adapter. In Day view, Calendar forwards layout, render, event,
  dispatch-event, focus, tick, init, mount, unmount, and destroy to the child. Drain child
  `DataView`/`ListControl` events internally and translate them to the existing Calendar state and
  events: highlight changes update `highlighted_entry`; activation emits `EntryActivated`; scoped
  reorder emits `EntriesReordered` with external entry IDs. Calendar continues to own Month/Week,
  cursor/date state, quick jumps, Today, Back, and DateActivated.

  **Done when:** Calendar's public events and `on_key` behavior remain compatible; no ListControl
  event escapes Calendar; Month and Week behavior is unchanged.

- [x] 3.3 — Preserve Calendar focus, input, and Day render contracts

  **Implementation:** Keep Calendar's outer `FocusId("calendar")` and existing route expected by
  `tuido/src/calendar.rs`. Suppress the embedded control as a separate external tab stop while
  forwarding Calendar focus to it for visual styling. Feed Calendar's Day navigation and reorder
  bindings into the adapter. Keep wheel, Yank, Day mouse, and context-free Calendar render behavior
  intentionally compatible; document and test any deliberate exception.

  **Done when:** Tuido's existing Calendar `FocusRequest::TargetAt { id: "calendar" }` paths still
  work, Day task-detail routing remains stable, and no focus traversal regression appears.

- [x] **Checkpoint** — Verify Calendar Day integration tests: change Day/Week/Month, navigate and
  activate a task, make Shift/Ctrl selections, perform sparse and contiguous same-time block moves,
  cancel, then commit. The marker must match the staged result at every keypress.

---

## Phase 4: Preserve Tuido behavior and prove parity

- [x] 4.1 — Keep Tuido's Calendar adapter and persistence boundary unchanged

  **Implementation:** Review `src/calendar.rs` after the Tuicore change. Keep
  `CalendarWorkspace::sync_after_event` consuming only `CalendarTypedEvent::EntriesReordered` and
  passing its scoped external task IDs to `persist_task_order`. Preserve same-time task ordering,
  task-detail synchronization, Calendar quick menus, grouped actions, focus return paths, and
  transient selection lifecycle. Change event routing only if the embedded child requires a stable
  internal route; retain Calendar's outer route contract.

  **Done when:** Tuido still persists only the emitted same-time IDs and does not reorder unrelated
  snooze times or issue direct SQL from the UI.

- [x] 4.2 — Add parity and regression coverage

  **Implementation:** Extend Tuicore Calendar/ListControl tests and Tuido Calendar tests. Cover
  display-only safety; scoped same-time single/block reorder; contiguous and sparse marker movement;
  source-order commit; cross-scope rejection; refresh while selected/reordering; duplicate identity
  rejection; custom bindings; focus loss; direct and routed Day events; external-ID reconciliation;
  Calendar public event translation; and Tuido rank persistence/focus/detail/quick-menu behavior.
  Keep the existing Task List Single+OnNavigate Escape visual-state test as a separate regression.

  **Done when:** The test matrix proves Day selection and movement reuse ListControl behavior, no
  marker jump remains, Calendar public contracts remain stable, and Tuido integration tests pass.

- [x] **Checkpoint** — Verify in Tuido integration tests: select non-adjacent same-time Calendar tasks, enter move mode,
  press Up/Down repeatedly, and verify `Moving N tasks` tracks the visible insertion point. Commit
  and reopen Calendar; only that time group must be reordered. Then cancel a second move and verify
  no ranks change.

---

## Validation gate

Run before acceptance:

```text
# Tuicore
cargo test --lib
cargo check --tests

# Tuido
cargo test --lib
cargo check --tests
```

Treat the two documented Tuido baseline failures separately. Do not accept new failures in the
Calendar/ListControl interaction paths.
