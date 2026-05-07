# CX-45 Rebase Audit
Audited: 2026-05-06

---

## Purpose

CX-45 rebased 6 open Human Review PRs onto the current `submain` tip (`abb71a5`).
This document records the post-rebase audit verifying that none of the rebased
branches became empty during the operation — i.e., every branch still carries
real diffs relative to `submain`.

**Prior submain tip (CX-39 base):** `802b76e`
**New submain tip (CX-45 base):** `abb71a5` (includes CX-40 and CX-41)

**Audit trigger:** CX-45 — rebase 6 remaining Human Review PRs onto current submain
with empty-rebase detection.

---

## What Changed in Submain Since CX-39

Two commits landed on `submain` between the CX-39 rebase (`802b76e`) and this
rebase (`abb71a5`):

| Commit   | Ticket | Summary | Files Changed |
|----------|--------|---------|---------------|
| `c752e9a` | CX-40  | Enforce loop variable read-only invariant in IR validator | `ir/builder.rs`, `ir/lower.rs`, `ir/printer.rs`, `ir/types.rs`, `ir/validate.rs` |
| `85b29e8` / `5a6804c` | CX-41  | Implement Jump and Branch terminators (fix CX-27 regression) | `backend/cranelift/host_boundary.rs` |

**Side effect detected:** CX-40 added `read_only: bool` to the `BlockParam` struct,
but CX-41's test code in `host_boundary.rs` was committed without the new field,
breaking `cargo test --features jit` on `submain`. This CX-45 branch fixes all 3
affected `BlockParam` struct initializers in `host_boundary.rs` test code.

---

## Methodology

For each branch `stokowski/<ticket>`, the following was checked:

1. `git log --oneline origin/submain..origin/stokowski/<ticket>` — commits unique to the branch
2. `git diff origin/submain...origin/stokowski/<ticket> --stat` — file-level change summary

A branch passes the audit if:
- At least one unique commit exists ahead of `submain`
- The diff stat shows at least one file changed with a non-zero insertion/deletion count

A branch fails the audit (empty rebase) if:
- No unique commits exist, or
- The diff stat is empty (all changes were rebased away)

---

## Results

All 6 branches pass. No empty rebases detected.

| Branch               | Unique Commits | Files Changed | Lines Changed | Conflicts | Status |
|----------------------|---------------|---------------|---------------|-----------|--------|
| `stokowski/cx-30`    | 1             | 1             | +317/-11      | 8         | PASS ✓ |
| `stokowski/cx-32`    | 1             | 1             | +241/-5       | 10        | PASS ✓ |
| `stokowski/cx-33`    | 1             | 1             | +316/-16      | 10        | PASS ✓ |
| `stokowski/cx-34`    | 1             | 13            | +296/-24      | 0 (clean) | PASS ✓ |
| `stokowski/cx-36`    | 1             | 1             | +274/-16      | 9         | PASS ✓ |
| `stokowski/cx-38`    | 1             | 3             | +592/-83      | 10        | PASS ✓ |

---

## Branch Details

### stokowski/cx-30
**Commit:** `dd33b3c` CX-30: Cranelift emit: direct function calls — IrInst::Call (Phase 14 sub-packet 4)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 328 +++++++++++++++++++++++++++++++--
1 file changed, 317 insertions(+), 11 deletions(-)
```

**Conflicts:** 8 conflicts in `host_boundary.rs` with CX-41's Jump/Branch additions.
Resolution: additive merge — kept both `IrInst::Compare` (CX-41) and `IrInst::Call`
(CX-30) match arms; combined all test functions from both branches.

**Verdict:** Real diff — Call instruction emission, two-pass execute(), function call tests.

---

### stokowski/cx-32
**Commit:** `36037fa` CX-32: Wire up PtrOffset and PtrAdd in the Cranelift JIT emit path (Phase 15 sub-packet 1)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 246 +++++++++++++++++++++++++++++++--
1 file changed, 241 insertions(+), 5 deletions(-)
```

**Conflicts:** 10 conflicts in `host_boundary.rs` with CX-41's Jump/Branch additions.
Resolution: additive merge — PtrOffset/PtrAdd match arms added alongside Compare; all
Jump/Branch and PtrOffset/PtrAdd tests preserved.

**Verdict:** Real diff — PtrOffset and PtrAdd instruction emission in host_boundary.rs.

---

### stokowski/cx-33
**Commit:** `75f3f2b` CX-33: Wire SsaBind, ConstFloat, and Cast JIT instructions (Phase 15 sub-packet 2)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 332 +++++++++++++++++++++++++++++++--
1 file changed, 316 insertions(+), 16 deletions(-)
```

**Conflicts:** 10 conflicts in `host_boundary.rs` with CX-41's Jump/Branch additions.
Resolution: additive merge — SsaBind, ConstFloat, Cast match arms added alongside
Compare; all tests from both branches preserved.

**Verdict:** Real diff — SsaBind, ConstFloat, and Cast instruction wiring.

---

### stokowski/cx-34
**Commit:** `6496c88` CX-34: Expand differential harness JIT comparison to full supported 0.1 construct set (Phase 12 sub-packet 3)

**Diff stat:**
```
src/tests/verification_matrix/jit_t04_arith_div.cx   |   7 +
src/tests/verification_matrix/jit_t05_arith_rem.cx   |   7 +
... (13 files total)
13 files changed, 296 insertions(+), 24 deletions(-)
```

**Conflicts:** None — cx-34 only touches verification matrix `.cx` test files and
the harness driver, which are disjoint from CX-40/CX-41 changes.

**BlockParam fix:** Applied to `host_boundary.rs` (from inherited submain state) to
restore `cargo test` compilability on this branch.

**Verdict:** Real diff — differential harness expansion with new .cx test fixtures.

---

### stokowski/cx-36
**Commit:** `edb9881` CX-36: Add float arithmetic dispatch to Cranelift JIT Binary handler (Phase 15 sub-packet 3)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 290 +++++++++++++++++++++++++++++++--
1 file changed, 274 insertions(+), 16 deletions(-)
```

**Conflicts:** 9 conflicts in `host_boundary.rs` with CX-41's Jump/Branch additions.
Resolution: additive merge — ConstFloat instruction arm added alongside Compare;
all Jump/Branch and float arithmetic tests preserved.

**Verdict:** Real diff — float arithmetic dispatch in JIT Binary handler.

---

### stokowski/cx-38
**Commit:** `23827ad` CX-38: Implement runtime intrinsics dispatch mechanism (Phase 9 sub-packet 2)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 311 +++++++++++++++++++++++++++++--
src/ir/lower.rs                        | 324 ++++++++++++++++++++++++++-------
src/ir/validate.rs                     |  33 +++-
3 files changed, 592 insertions(+), 83 deletions(-)
```

**Conflicts:** 10 conflicts in `host_boundary.rs` with CX-41's Jump/Branch additions.
`lower.rs` and `validate.rs` auto-merged cleanly (CX-40's changes were
orthogonal to CX-38's intrinsics additions in those files).

Resolution: additive merge — Call dispatch (CX-38 intrinsics) + Compare/Jump/Branch
(CX-41) match arms combined; all test functions from both branches preserved.

**Verdict:** Real diff — runtime intrinsics dispatch mechanism across host_boundary,
lower, and validate modules.

---

## Submain Fix Included in CX-45

`cargo test --features jit` was broken on `submain` because CX-41 introduced 3
`BlockParam { value, ty }` struct initializers in test code without the `read_only`
field that CX-40 added. This CX-45 branch fixes those 3 initializers in
`src/backend/cranelift/host_boundary.rs`:

- Line 1202: `jit_jump_passes_value_via_block_param` block1 param
- Line 1464: `jit_branch_with_block_args_on_both_edges` block1 param
- Line 1470: `jit_branch_with_block_args_on_both_edges` block2 param

All three get `read_only: false` (loop counter invariant does not apply to these
test block parameters).

---

## Validation

```
cargo build --features jit  → Finished (21 warnings, 0 errors)
cargo test --features jit   → test result: ok. 194 passed; 0 failed
```

(Run on `stokowski/CX-45` branch, which includes the BlockParam fix.)
