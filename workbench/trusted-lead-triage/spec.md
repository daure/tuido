# Spec: Trusted Lead Triage

> **Purpose:** Help a technical lead capture messy daily work, clarify it into trusted structured items, hide deferred work safely, pull only active action items onto a clean board, and choose a simple time-aware daily focus.
> **In scope:** Inbox capture, AI-assisted/manual clarification, item typing, action subtypes, item sizing, start/due dates for action items, people/team/system/project fields, snooze return behavior, board pull flow, daily 1-3-5 focus, mid-day focus swaps, frog task selection, lightweight focus history, immediate archive on done.
> **Out of scope:** Full GTD clone, autonomous AI organization, calendar view, recurrence, ticket/doc ingestion, provider-specific AI implementation, team collaboration, priority labels, hard capacity enforcement.

## Domain Model

- Raw inbox items are untrusted captures and cannot be snoozed or pulled to the board.
- Clarified items have a type: `action` or `note`.
- `action` items may have subtypes, including `waiting`, `follow_up`, and `artifact_update`.
- `waiting` is an action subtype/state for work that depends on someone else, not a separate top-level clarified item type.
- Action items have a size: `small`, `medium`, or `big`.
- Action items may have a start date and/or due date.
- Start dates make action items eligible for Daily Focus suggestions; they do not hide items from normal lists or search.
- Due dates represent commitment signals, not priority labels.
- Priority labels are not part of the model; time relevance comes from start dates, due dates, snooze returns, size, and explicit user selection.
- Clarified items may reference people, teams, systems, and project labels.
- Only `action` items can be pulled onto the board.
- The board has three columns: `To Do`, `In Progress`, and `Done`.
- Daily focus follows a 1-3-5 structure: one big action, up to three medium actions, and up to five small actions.
- A daily focus may identify one frog task: the first meaningful task the user intends to tackle.
- The frog task must be an action item that is visible and eligible for work today.
- Daily Focus is editable mid-day through explicit swaps that add new work and move other focus items out.
- Moved-out focus items return to their source list or board state unchanged unless the user explicitly changes them.
- Daily Focus history stores the original plan, selected frog, swaps, and outcomes for lightweight trend analysis.
- Snoozed clarified items are hidden until their return date/time.
- Completed board items are archived immediately when marked done.

## Scenarios

### Capturing raw work quickly

**Given** the user has a thought, note, or obligation during the day
**When** the user captures it without adding structure
**Then** it is stored as a raw inbox item
**And** it is not eligible for snooze or board pull until clarified

### AI suggests clarified items from messy text

**Given** a raw inbox item contains messy text with one or more possible work items
**When** the user starts clarification with AI available
**Then** the system suggests one or more clarified items
**And** each suggestion includes type, optional action subtype, suggested size for actions, optional date hints, and likely people, teams, systems, and project labels
**And** the suggestions remain uncommitted until the user accepts or edits them

### AI splits a large raw note

**Given** a raw inbox item contains multiple distinct ideas, notes, or obligations
**When** AI clarification runs
**Then** the system suggests separate clarified items for the distinct ideas
**And** the user can accept, edit, merge, or discard the suggested splits

### Manual clarification works without AI

**Given** AI is unavailable, misconfigured, or fails during clarification
**When** the user clarifies a raw inbox item
**Then** the user can manually create clarified items
**And** the AI failure is visible but does not block clarification

### Clarifying an action item

**Given** a raw inbox item represents work the user may perform
**When** the user accepts or creates a clarified `action` item
**Then** the item becomes eligible to be pulled onto the board
**And** it may reference people, teams, systems, and project labels
**And** it has a size of `small`, `medium`, or `big`

### Clarifying action timing

**Given** a raw inbox item describes work with timing constraints
**When** the user accepts or creates a clarified `action` item
**Then** the item may include a start date, due date, or both
**And** start dates are used to exclude future items from Daily Focus suggestions until eligible
**And** future-start action items remain visible in normal lists and search
**And** due dates act as commitment signals
**And** neither date creates a priority label

### Action size supports planning

**Given** an action item exists
**When** the user edits its planning details
**Then** the user can classify it as `small`, `medium`, or `big`
**And** the size is used for 1-3-5 daily focus suggestions
**And** size does not imply priority

### Clarifying an artifact update

**Given** a raw inbox item describes a needed update to a Confluence page, Jira ticket, or other external artifact
**When** the user clarifies it as an action with subtype `artifact_update`
**Then** the item becomes eligible to be pulled onto the board as an action
**And** the external artifact can be captured as supporting context

### Clarifying waiting work

**Given** a raw inbox item represents something the user is waiting on from someone else
**When** the user clarifies it as an action with subtype or state `waiting`
**Then** it is tracked as actionable work that depends on someone else
**And** it can reference the relevant person, team, system, or project label
**And** it can use snooze/return behavior when the user wants it hidden until follow-up time

### Clarifying a note

**Given** a raw inbox item contains useful context without a concrete action
**When** the user clarifies it as `note`
**Then** it is stored as clarified reference context outside the board
**And** it can reference people, teams, systems, and project labels

### Only clarified items can be snoozed

**Given** an item is still a raw inbox item
**When** the user attempts to snooze it
**Then** the system requires clarification before snooze can be applied

### Snoozed items are hidden until return

**Given** a clarified item has been snoozed until a future date/time
**When** that date/time has not arrived
**Then** the item is hidden from normal clarified-item lists and the board

### Snoozed items return to the clarified list

**Given** a clarified item was snoozed until a date/time
**When** that date/time arrives
**Then** the item appears at the top of the clarified items list
**And** it has a visible returned-from-snooze marker
**And** it can be pulled, snoozed again, edited, or archived according to its type

### Pulling work onto the board

**Given** an `action` item exists in the clarified items list
**When** the user pulls it onto the board
**Then** it appears in the board `To Do` column
**And** non-action clarified items remain outside the board

### Building daily focus with 1-3-5

**Given** visible action items exist in the clarified list or board
**When** the user builds today's focus
**Then** the system can group candidate actions into one big item, up to three medium items, and up to five small items
**And** the user can accept, remove, replace, or reorder suggested items
**And** no item is added to daily focus without user confirmation

### Swapping mid-day focus items

**Given** today's focus already contains selected action items
**When** new urgent or relevant work needs to enter Daily Focus mid-day
**Then** the user can add the new big, medium, or small action items
**And** the system prompts the user to move out enough existing focus items to keep the 1-3-5 shape honest
**And** the user confirms which items move out

### Moving items out of Daily Focus

**Given** an item is moved out of Daily Focus during a mid-day swap
**When** the swap is confirmed
**Then** the item leaves today's focus
**And** its clarified-list or board state remains unchanged
**And** it is not snoozed, archived, or rescheduled unless the user explicitly chooses that action

### Choosing the frog task

**Given** today's focus contains one or more action items
**When** the user chooses a frog task
**Then** the selected item is marked as today's frog
**And** the frog is visually presented as the first meaningful task to tackle
**And** the frog marker does not change item type, size, board column, or priority

### AI suggests daily sequence

**Given** visible action items include size and time context
**When** AI planning assistance is available
**Then** the system can suggest clusters, a frog candidate, and an ordered 1-3-5 sequence
**And** each suggestion includes an explanation based on size, dates, context, or related work
**And** the user can accept, edit, or ignore the suggestions

### AI suggests mid-day swaps

**Given** today's focus is already populated
**When** new urgent or relevant work is added mid-day
**Then** AI can suggest which existing focus items might move out
**And** each suggestion explains the tradeoff using size, dates, current focus shape, or context
**And** no item is removed from Daily Focus without user confirmation

### Moving active work through the board

**Given** an action item is on the board
**When** the user starts work on it
**Then** it can move from `To Do` to `In Progress`

### Completing active work

**Given** an action item is on the board
**When** the user marks it done
**Then** it is archived immediately
**And** it no longer appears on the board

### Board capacity is self-managed

**Given** the user pulls action items from the clarified list
**When** the board already contains multiple active items
**Then** the system allows the pull without enforcing a hard item limit
**And** board clutter remains the user’s responsibility in v1

### Daily focus capacity is advisory

**Given** the user builds today's 1-3-5 focus
**When** the user chooses fewer or more than one big, three medium, and five small items
**Then** the system allows the plan
**And** it shows the mismatch as guidance rather than blocking the user

### Daily focus history tracks plans and swaps lightly

**Given** the user creates or changes Daily Focus
**When** the day ends or focus state changes
**Then** the system stores the original plan, selected frog, added items, removed items, broad swap reasons, and item outcomes
**And** the history is used for trend analysis without becoming a detailed audit log

### AI never silently organizes work

**Given** AI has inferred classifications, labels, subtypes, or split items
**When** clarification results are shown
**Then** no clarified item is saved, snoozed, or pulled to the board without user confirmation

### AI does not assign priority

**Given** AI has analyzed item size, dates, clusters, trends, or frog candidates
**When** suggestions are shown
**Then** the suggestions do not include priority labels
**And** urgency or importance is explained through dates, size, relationships, repeated deferrals, or user-selected focus

## Open Questions

- Should clarified items have a dedicated visible “all clarified” list, or multiple filtered lists by type?
- Should snoozed items be discoverable through search before their return date?
- Should notes support conversion into action items after clarification?
- Should archived done items be searchable in v1?
- What confidence UX should AI suggestions use: confidence score, explanation text, alternate suggestions, or all three?
