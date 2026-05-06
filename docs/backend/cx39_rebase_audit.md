# CX-39 Rebase Audit
Audited: 2026-05-06

---

## Purpose

CX-39 rebased 8 open Human Review PRs onto the current `submain` tip (802b76e).
This document records the post-rebase audit verifying that none of the rebased
branches became empty during the operation — i.e., every branch still carries
real diffs relative to `submain`.

**Audit trigger:** CX-42 — empty-rebase audit for 7 remaining CX-39-rebased branches.

---

## Methodology

For each branch `stokowski/<ticket>`, the following was checked:

1. `git diff origin/submain...origin/stokowski/<ticket> --stat` — file-level change summary
2. `git log --oneline origin/submain..origin/stokowski/<ticket>` — commits unique to the branch

A branch passes the audit if:
- At least one unique commit exists ahead of `submain`
- The diff stat shows at least one file changed with a non-zero insertion/deletion count

A branch fails the audit (empty rebase) if:
- No unique commits exist, or
- The diff stat is empty (all changes were reverted or rebased away)

---

## Results

All 8 branches pass. No empty rebases detected.

| Branch               | Unique Commits | Files Changed | Lines Changed | Status |
|----------------------|---------------|---------------|---------------|--------|
| `stokowski/cx-27`    | 1             | 1             | 379           | PASS ✓ |
| `stokowski/cx-30`    | 1             | 1             | 322           | PASS ✓ |
| `stokowski/cx-31`    | 1             | 10            | 275           | PASS ✓ |
| `stokowski/cx-32`    | 1             | 1             | 239           | PASS ✓ |
| `stokowski/cx-33`    | 1             | 1             | 301           | PASS ✓ |
| `stokowski/cx-34`    | 1             | 12            | 315           | PASS ✓ |
| `stokowski/cx-36`    | 1             | 1             | 283           | PASS ✓ |
| `stokowski/cx-38`    | 1             | 3             | 668           | PASS ✓ |

---

## Branch Details

### stokowski/cx-27
**Commit:** `b866d21` CX-27: Cranelift emit: Compare + Jump + Branch (Phase 14 sub-packet 3)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 379 ++++++++++++++++++++++++++++++++-
1 file changed, 369 insertions(+), 10 deletions(-)
```

**Verdict:** Real diff — Compare/Jump/Branch instruction emission in host_boundary.rs.

---

### stokowski/cx-30
**Commit:** `769f528` CX-30: Cranelift emit: direct function calls — IrInst::Call (Phase 14 sub-packet 4)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 322 +++++++++++++++++++++++++++++++--
1 file changed, 311 insertions(+), 11 deletions(-)
```

**Verdict:** Real diff — Call instruction emission, two-pass execute(), alloca tests.

---

### stokowski/cx-31
**Commit:** `1be5bed` CX-31: Add differential harness JIT execution and comparison for arithmetic subset

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs              |  79 +++++++----
src/diff_harness.rs                                 | 149 ++++++++++++++++++++-
src/frontend/semantic.rs                            |   6 +-
src/ir/lower.rs                                     |  33 ++++-
src/main.rs                                         |   3 +-
src/tests/verification_matrix/jit_arith_t01_add.cx  |   1 +
src/tests/verification_matrix/jit_arith_t02_sub.cx  |   1 +
src/tests/verification_matrix/jit_arith_t03_mul.cx  |   1 +
src/tests/verification_matrix/jit_arith_t04_div.cx  |   1 +
src/tests/verification_matrix/jit_arith_t05_rem.cx  |   1 +
10 files changed, 240 insertions(+), 35 deletions(-)
```

**Verdict:** Real diff — differential harness JIT execution path plus 5 fixture files.

---

### stokowski/cx-32
**Commit:** `fa2a507` CX-32: Wire up PtrOffset and PtrAdd in the Cranelift JIT emit path (Phase 15 sub-packet 1)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 239 ++++++++++++++++++++++++++++++++-
1 file changed, 237 insertions(+), 2 deletions(-)
```

**Verdict:** Real diff — PtrOffset and PtrAdd JIT emission.

---

### stokowski/cx-33
**Commit:** `245c774` CX-33: Wire SsaBind, ConstFloat, and Cast JIT instructions (Phase 15 sub-packet 2)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 301 ++++++++++++++++++++++++++++++++-
1 file changed, 295 insertions(+), 6 deletions(-)
```

**Verdict:** Real diff — SsaBind, ConstFloat, Cast instruction wiring.

---

### stokowski/cx-34
**Commit:** `8427db5` CX-34: Expand differential harness JIT comparison to full supported 0.1 construct set (Phase 12 sub-packet 3)

**Diff stat:**
```
src/diff_harness.rs                                  | 208 +++++++++++++++++++--
src/frontend/semantic.rs                             |   9 +-
src/ir/lower.rs                                      |  28 ++-
src/main.rs                                          |  14 +-
src/tests/verification_matrix/jit_t01_arith_add.cx   |   7 +
src/tests/verification_matrix/jit_t02_arith_sub.cx   |   7 +
src/tests/verification_matrix/jit_t03_arith_mul.cx   |   7 +
src/tests/verification_matrix/jit_t04_arith_div.cx   |   7 +
src/tests/verification_matrix/jit_t05_arith_rem.cx   |   7 +
src/tests/verification_matrix/jit_t06_const_return.cx|   7 +
src/tests/verification_matrix/jit_t07_bool_return.cx |   7 +
src/tests/verification_matrix/jit_t08_nested_arith.cx|   7 +
12 files changed, 293 insertions(+), 22 deletions(-)
```

**Verdict:** Real diff — expanded differential harness plus 8 fixture files.

---

### stokowski/cx-36
**Commit:** `05c9d78` CX-36: Add float arithmetic dispatch to Cranelift JIT Binary handler (Phase 15 sub-packet 3)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 283 +++++++++++++++++++++++++++++++--
1 file changed, 270 insertions(+), 13 deletions(-)
```

**Verdict:** Real diff — float arithmetic dispatch in JIT Binary handler.

---

### stokowski/cx-38
**Commit:** `a8504de` CX-38: Implement runtime intrinsics dispatch mechanism (Phase 9 sub-packet 2)

**Diff stat:**
```
src/backend/cranelift/host_boundary.rs | 311 +++++++++++++++++++++++++++++--
src/ir/lower.rs                        | 324 ++++++++++++++++++++++++++-------
src/ir/validate.rs                     |  33 +++-
3 files changed, 588 insertions(+), 80 deletions(-)
```

**Verdict:** Real diff — runtime intrinsics dispatch mechanism across three files.

---

## CX-39 Branch Note

The `stokowski/CX-39` branch itself carries exactly one commit ahead of `submain`:

```
228a06b CX-39: rebase 8 Human Review PRs onto current submain
```

This commit is intentionally a no-op diff — it is an administrative record of the
rebase activity. The actual rebased content lives on the 8 individual branches above.
The empty diff on `stokowski/CX-39` is expected and correct.

---

## Conclusion

All 8 CX-39-rebased branches contain real, substantive diffs. The rebase did not
accidentally empty any branch. The individual branches are ready for review and
merge in dependency order.

**Audit result: CLEAN — no empty rebases found.**
