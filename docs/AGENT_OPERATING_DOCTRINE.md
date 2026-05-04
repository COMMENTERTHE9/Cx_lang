# Agent Operating Doctrine
**Version 1.0 — Cx Project**
**Author: Zara (CTO) + Dev Lead**
**Status: Active**

---

## Purpose

This document defines how the dev lead and the coding agent work together on the Cx project. It exists to prevent two failure modes:

1. **Reckless speed** — agent freelances, makes unrequested changes, breaks things silently, costs hours of debugging.
2. **Wasteful caution** — dev lead drip-feeds tiny prompts one at a time, agent waits for permission every 30 seconds, real progress crawls and the human burns out from context switching.

The doctrine below is the middle path: structured, fast, and still safe.

---

## Core Principle

**Work in task packets, not prompt packets.**

A task packet is one full logical unit of work:
- one behavior
- one feature
- one bugfix cluster
- one refactor unit
- one migration unit

If multiple files belong to the same logical change, they stay in the same packet. If something is actually a separate behavior, it becomes a separate packet even if it touches only one file.

The unit of work is the **logical change**, not the file count.

---

## The Six Phases

Every task moves through six phases in order. Skipping phases causes either rework or scope creep.

### Phase 1 — Recon

**Goal:** gather everything needed for the entire task in one read-only prompt.

**Rules:**
- One broad read prompt, not file-by-file inspection.
- Pull every file, function, type, call site, test, entry point, and config section the task touches.
- Include upstream callers and downstream consumers.
- No code changes. Read only.

**Anti-pattern:** asking for one file, getting a report, asking for the next file. That is drip-fed inspection and it is forbidden unless the task is genuinely tiny.

### Phase 2 — Design Lock

**Goal:** define the task fully before any code is written.

**Must specify:**
- Exact behavior we want
- Exact behavior we do not want
- Files that are allowed to change
- Files that must stay untouched
- Success criteria
- Build and test commands that will validate

**Rule:** no implementation starts until design is locked. The dev lead states the design, the user confirms or corrects.

### Phase 3 — Predicted Fallout

**Goal:** identify everything the task will break or require updating, before writing the implementation prompt.

**Must call out:**
- Tests likely to fail
- Adjacent files that may need edits
- Config, docs, or migrations that may need updating
- Whether this is a local change or a system change

**Rule:** predicted fallout fixes are written into the implementation prompt up front, not discovered one by one afterward.

### Phase 4 — Implementation

**Goal:** make all the changes for the task in one coherent pass.

**One implementation prompt per task packet.** That prompt contains:
- All related file edits
- Exact additions, replacements, removals
- Predicted test fixes
- Constraints (what not to touch)
- Explicit "no unrelated cleanup, no opportunistic refactors"

**Rule:** if the task is one coherent unit, do it in one implementation pass. Splitting is only allowed under the conditions in the Splitting Rules section below.

### Phase 5 — Validation

**Goal:** confirm the task is complete using the real gate.

**The real gate is:**
- `cargo build` (or project equivalent) clean
- `cargo test` (or project equivalent) green
- Any task-specific verification

**Rule:** do not request extra "show me state" reports if the build and test outputs already answer the question. The test suite is the report. Extra reporting is reserved for:
- Architecture-changing tasks
- Migration or config tasks
- Explicit user request for a summary

### Phase 6 — Commit

**Goal:** ship the task and close the packet.

**Commit when:**
- Scope is complete
- Build passes
- Tests pass
- No known blockers remain

**Rule:** do not stretch the task once the packet is done. Do not add "one more thing" after validation passes. New work is a new packet.

---

## Splitting Rules

A task should be kept together unless one of the following is true:

- Behavior is not yet locked
- Real architectural uncertainty exists
- The change crosses too many unrelated systems
- Recon found blockers
- One-pass editing carries genuine risk

Splitting "to be safe" is not a valid reason. The build and test gate catches problems either way. Splitting wastes turns.

---

## Operating Modes

The dev lead operates in one of five modes at any time. Modes are not blurred.

| Mode | Purpose | Output |
|------|---------|--------|
| **Recon** | Gather context for a task | Read-only prompt to agent |
| **Design** | Lock task scope and behavior | Design statement to user for approval |
| **Implementation** | Direct the agent to make changes | One implementation prompt to agent |
| **Validation** | Confirm task is complete | Read build/test results, report deviations only |
| **Commit** | Ship the packet | Git commit + push prompt to agent |

The dev lead announces mode shifts only when useful. Most tasks flow Recon → Design → Implementation → Validation → Commit without explicit narration.

---

## What the Dev Lead Does Not Do

- Does not ask for state reports between every change
- Does not split coherent changes into multiple prompts out of caution
- Does not narrate scorecards after every commit
- Does not allow the agent to make unrequested changes
- Does not let the agent fix errors without reporting them first
- Does not push to main directly
- Does not allow silent deletions
- Does not write code itself — directs the agent
- Does not stretch task scope mid-packet

## What the Agent Does Not Do

- Does not freelance
- Does not make changes outside the implementation prompt
- Does not "clean up" unrelated code
- Does not skip the build/test gate before reporting complete
- Does not commit without explicit instruction
- Does not delete files without explanation
- Does not silently handle errors — reports them first

---

## Anti-Patterns to Avoid

**Drip-fed recon.** Asking for one file at a time when the task touches five. Wastes turns. Use one broad recon prompt.

**Discovery debugging.** Writing the feature, then discovering the test failures, then fixing them, then discovering more failures. Predicted fallout exists to prevent this. Predict the fallout in Phase 3, fix it in Phase 4.

**Permission-loop autocomplete.** Agent waits for the dev lead to confirm every micro-step. Doctrine exists to break this loop — once a task packet is defined, the agent executes the whole packet.

**Scope creep mid-packet.** Adding "while we're here, also fix X" during implementation. New work is a new packet.

**Over-narration.** Writing scorecards, summaries, and progress reports after every commit. Save these for milestones or explicit user request.

**Opportunistic refactoring.** Agent or dev lead noticing unrelated code and "improving" it during a task. This is forbidden. Refactor tasks are their own packets.

---

## Default Rhythm

For most tasks, the rhythm is:

1. **Recon prompt** — one broad read-only prompt for the full task
2. **Recon report** — agent returns everything needed to lock the design
3. **Implementation prompt** — one prompt covering the whole task packet, including predicted fallout fixes
4. **Validation report** — agent runs build/test, returns results
5. **Commit prompt** — dev lead authorizes commit, agent reports hash

That is **5 turns per task** instead of the 10–15 the old rhythm produced. Roughly 3x faster for the same safety, with the safety net intact.

---

## When Doctrine Is Suspended

The dev lead may suspend doctrine in these cases:

- **Emergency debugging** — when something is broken in production-equivalent state and needs immediate diagnosis. Doctrine resumes after the immediate fix.
- **Exploratory research** — when the goal is "what does this code do?" rather than "change this code."
- **User explicit override** — when the user (Zara) directs a different approach for a specific task.

Suspension is announced. Doctrine resumes by default at the next task packet.

---

## Coordination With Other Devs

Before starting any task that touches files outside the dev lead's owned area:

- Check if another dev is working on those files
- Send a coordination message before recon if there's any chance of conflict
- Wait for clearance or confirm the window is clear

Owned area for the backend dev lead:
- `src/ir/`
- `src/backend/`
- `docs/backend/`

Shared with frontend dev (coordinate before touching):
- `src/main.rs`
- Any file in `src/frontend/` or `src/runtime/`

---

## Pre-Commit Gate

Every commit must pass:

1. `cargo build --features jit` — zero errors
2. `cargo test --features jit` — all tests passing
3. Commit hash reported after push

If either gate fails, the commit does not happen. Fix first.

---

## Commit Discipline

- One commit per logical change
- Commit message describes the change, not the process
- No bundling unrelated changes into one commit
- Push immediately after commit
- Report hash to dev lead

---

## Session-End Summary

At the end of a session, the dev lead provides:

- List of commits shipped (hash + description)
- Phases or features closed
- Outstanding blockers
- Suggested next packet

This is the only place narrative scorecards belong. Not after every commit. Not after every phase. End of session only, or when explicitly requested.

---

## Doctrine Updates

This doctrine is version 1.0. Updates require:

- Explicit decision from Zara
- Version bump
- Rationale documented in changelog section below

### Changelog

- **v1.0** — Initial doctrine. Created after one full session of the old turn-by-turn rhythm proved too slow to sustain. Replaces ad-hoc workflow with task-packet model.

---

## End of Doctrine
