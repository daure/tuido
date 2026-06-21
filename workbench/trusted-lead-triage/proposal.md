# Proposal: How might we build trusted lead triage?

### Problem

How might we build a trusted lead triage system that captures everything, AI-clarifies messy notes into structured items, keeps hidden work safe, and helps a technical lead choose a simple time-aware daily focus without priority-label theater?

### Options

#### Trusted Triage

What it is: Inbox → Clarify Queue → user-confirmed structured item.

What it buys: Strongest foundation; no lost notes, no label soup, no silent AI mistakes.

What it costs: Less flashy until board and AI polish grows.

#### Clean Pull Board

What it is: Board only shows active commitments, chosen from clarified items.

What it buys: Focus; no waiting/snoozed landfill.

What it costs: Needs strong off-board sections or hidden work feels unsafe.

#### Hidden Ledger

What it is: Snoozed, Someday, Reference, and Done live outside the board with return/review guarantees. Waiting is modeled on action items rather than as a separate top-level ledger.

What it buys: Trust that hidden items are not forgotten, without splitting waiting-for-someone work away from actionable follow-up.

What it costs: Requires review rituals and resurfacing rules.

#### Context Graph

What it is: Maintain people, teams, systems, projects/labels, tickets, docs, and old items.

What it buys: AI can infer better; filtering/search become powerful.

What it costs: Data model grows fast.

#### AI Copilot Layers

What it is: AI clarifies raw notes, splits big notes into items, detects follow-ups, assists review, and suggests daily capacity.

What it buys: Major leverage for technical-lead mess.

What it costs: Provider setup, privacy, explainability, and confidence UX.

#### Daily 1-3-5 Focus

What it is: Each day can be shaped into one big item, three medium items, and five small items, with one frog task called out as the first meaningful thing to tackle. The plan can change mid-day, but changes should be conscious swaps rather than silent pile-on.

What it buys: A simple planning ritual that converts a clarified backlog into a realistic daily focus without adding priority labels, while still supporting urgent work that appears mid-day.

What it costs: Requires each actionable item to carry enough size and time context for the app and AI to suggest a useful daily shape, and requires lightweight swap tracking so the focus does not become a landfill.

### Recommendation

Build **Trusted Triage first**, with **AI clarify suggestions**, a minimal **Context Graph**, and lightweight **Daily 1-3-5 Focus** from the start.

V1 shape:

```text
Inbox
Clarify Queue

Off-board:
  Snoozed
  Someday
  Reference

Board:
  To Do
  In Progress
  Done

Daily Focus:
  Frog
  1 Big
  3 Medium
  5 Small
  Mid-day Swaps
```

Maybe later the board adds `Blocked`, but not in the first version.

The board remains a pull surface, not a landfill. Daily Focus is a lens over eligible action items, not a separate state machine. An item can be in the clarified list or on the board and still be suggested for today's 1-3-5 plan if it is actionable, visible, and time-relevant.

Daily Focus is editable under pressure. When an urgent medium task, big task, or several small tasks appear mid-day, the user can add them, but the app should prompt them to make room by moving other focus items out. Moved-out items return to their source list or board state unchanged unless the user explicitly snoozes or edits them. This keeps the daily plan honest: current intentional focus, with conscious tradeoffs.

### Clarify Taxonomy

Do not make everything tags.

Use distinct fields:

```text
type:
  action
  note

action_subtype:
  task
  waiting
  follow_up
  artifact_update

state:
  inbox
  clarify
  next
  doing
  waiting
  snoozed
  someday
  reference
  done
  canceled

context:
  people
  teams
  systems
  project_labels
  tickets
  docs
  links

size:
  small
  medium
  big

time:
  start_date
  due_date
  snooze_until
  completed_at

daily_focus:
  frog_candidate
  selected_frog
  selected_for_today
  swapped_in
  swapped_out
  swap_reason
```

Do not add priority labels. Size, dates, state, and daily focus explain why something matters now. If the app needs urgency, infer it from due dates, start dates, returned snoozes, and user selection rather than a `high/medium/low priority` field.

Start dates make action items eligible for Daily Focus suggestions; they do not hide items from normal lists or search. Due dates represent commitment signals, not priority labels. Waiting is an action subtype/state for work that depends on someone else, not a separate top-level item type.

### AI Use

Use a provider-neutral interface. Do not use opencode/pi as the core library unless there is a stable SDK. Safer default: define an internal provider trait/interface.

```text
ClarifyProvider:
  input: raw inbox text + known people/systems/projects/tickets
  output: suggested items with confidence, explanation, type, size, and optional time hints
```

AI never silently moves items. It suggests; the user accepts or edits.

AI should also help with planning, but only as an assistant:

- cluster related action items;
- suggest small/medium/big sizing;
- identify likely frog candidates;
- propose a 1-3-5 sequence for today;
- suggest what to swap out when urgent work enters mid-day;
- explain why each item was suggested;
- track trends like repeated deferrals, too many big items, or recurring overload.

The user owns final selection. AI does not assign priority labels, does not auto-select the frog, and does not auto-remove work from Daily Focus.

### Discarded Options

- **Full GTD app clone** — too broad.
- **Board-first system** — risks board landfill.
- **Snoozed on board** — violates the goal: out of sight until needed.
- **Waiting on board** — useful sometimes, but better as a separate people/promise section.
- **AI autonomous organizer** — breaks trust if it misfiles silently.

### Open Questions

- Should `next` be visible as a list, or should only Clarify Queue feed Doing?
- What exact sections should exist outside the board: Snoozed, Someday, Reference, People, Review?
- Should `project_labels`, `teams`, `people`, `systems`, `tickets`, and `docs` all use the same reference model internally?
- What confidence threshold should AI suggestions need before being preselected versus shown as alternatives?
- What local data should AI be allowed to read by default, and what should require explicit confirmation?
- Should clarified items support recurrence, or should recurrence wait until after snooze/defer is stable?
- What is the minimum viable review ritual: daily, weekly, or only stale-item prompts?
- Should due dates be allowed on notes, or only actionable items?

### Resolved Questions

- Only clarified items can be snoozed. Raw inbox items must be clarified first.
- When a snoozed item returns, it appears at the top of the clarified items list where it can be pulled into active work again.
- `project_labels` is a first-class field, like `teams`, `people`, and `systems`; it is not a loose tag blob.
- AI providers should be integrated through an adapter layer. Initial target providers include AWS Bedrock, OpenAI OAuth, and Antigravity/Gemini OAuth.
- Actionable items use size (`small`, `medium`, `big`) instead of priority labels.
- Daily planning follows a lightweight 1-3-5 model, with one frog task highlighted as the first meaningful task to tackle.
- Daily Focus is a planning lens over eligible actions, not separate storage and not a board replacement.
- Start dates exclude future actions from Daily Focus suggestions, but do not hide them from normal lists or search.
- Due dates are commitment signals, not priority labels.
- The frog can be suggested by AI, but must be selected or confirmed by the user.
- Daily Focus history stores the original plan, selected frog, swaps, and outcomes for lightweight trend analysis.
- Waiting is an action subtype/state, not its own top-level clarified item type.
- Mid-day changes should use explicit swaps: urgent work can enter Daily Focus, but the user should move other focus items out to make room.
