<!-- RE-AUDIT-START -->
## Re-audit 2026-05-19 (submain @ d4e419a)

Verdict: **NOT 0.1-READY** (H1 and H2 closed since previous audit; H3 + H4 + new H5 remain).

Audited roadmap: `docment/ROADMAP.md` (Living Summary). HEAD d4e419a, branch submain, working tree clean. Five commits since the previous audit's c67c5c2 baseline: 8e7143d (H1 closure), 0ab7e9b (H2 + literal-width family), 930d84d (doc spot-fixes), 1036fd8 (previous audit annotations), d4e419a (test-assertion fix for the A3 drift the previous audit attempt caught).

Build: debug `cargo build` ok, 21 warn | debug `cargo build --features jit` ok, 20 warn | release `cargo build --release --features jit` ok, 20 warn. No clippy gate in CI; pre-existing `src/ir/printer.rs:10` clippy error untouched by recent commits.
Tests: `cargo test --features jit` **411 passed / 0 failed / 0 ignored** — this is the full-suite gate that was missing from the previous task chain and that surfaced d4e419a's necessary fix. `cargo test` (no features) not re-run this audit but unchanged from previous audit's 236/0/0.
Parity matrix: `jit_parity_by_feature` 182 fixtures, **0 PARITY_FAIL** across all 16 categories.

### Hard blockers
- **H3** `when` block lowering — still rejection-only at `src/ir/lower.rs:1072` (stmt) and `:2039` (expr). t143/t144/t145 still SKIP via exit-127. Cheapest fix unchanged from previous audit: ship the lowering or reword the roadmap line to drop "or lowering". Sizing: **S** if rewrite, **M** if actually lower.
- **H4 — RESOLVED (was a false-positive at audit time).** The three sub-items the audit
  flagged are all settled: CX-91 landed at submain commit e8d31fc (Cast + F64 binary
  arithmetic, predates audit baseline); CX-94 landed via PR #137 at submain commit
  cb36c8f (DotAccess parity fixtures, predates audit baseline); CX-34 was canceled
  2026-05-10 with scope superseded by CX-30/51/59/62/66/69/94 (all landed). The
  audit's "feature branch not on submain" claim was doc-text drift, not work-state —
  the original roadmap items at lines 231–233 had not been updated to reflect the
  landed work or the cancellation. Roadmap entries below corrected.
- **H5 (NEW)** Commit-message metric drift — all four landed commits (8e7143d/0ab7e9b/930d84d/d4e419a) state parity matrix totals of **94 PASS / 88 SKIP / 0 PARITY_FAIL / 182**. The actual table sums to **99 PASS / 83 SKIP / 0 PARITY_FAIL / 182** (per per-category-row summation this run; `cargo test --features jit jit_parity_by_feature | awk` reproduces the 99/83 totals exactly). The 94/88 number was an arithmetic error that propagated through four consecutive commit messages without anyone re-summing the rows. **Not a code regression** — the matrix table itself is correct; the prose claims about it are wrong. Sizing: **S**. Cheapest fix: add a single `println!` at the bottom of the `jit_parity_by_feature` test that emits the actual `total_pass`/`total_skip` totals computed from `results.values().map(|(p, s, _)| ...).sum()`, removing the manual-sum trap forever. (One change in `src/diff_harness.rs`, ~5 lines.)

### Soft blockers
- **S5** README headline still drifted — `README.md:49,58` say "155 verification matrix tests"; actual is 182. The chore(docs) commit 930d84d spot-fixed per-category rows + Total but left the headline. Same class of finding as the previous audit's S1.
- **S6** `docs/backend/cx_jit_parity_checklist.md` §3 line 149 still reads "Total: 181 fixtures, 0 PARITY_FAILs"; t178 addition not reflected. Stale by 1.
- **S7** `README.md:60` `67 PASS / 88 SKIP` mismatches both its own Total row at line 84 (71/88) and the matrix (99/83). Internal inconsistency carried from previous audit's S1 + the partial 930d84d fix.
- **S8** Print-family stub blocks 15 matrix fixtures (`t01_arith_eq_mod`, `t39_impl_basic`, `t40_impl_return`, `t41_compound_assign_dot`, `t43_multi_alias_impl`, `t45_if_else_in_func`, `t55_f64_basic`, `t76_block_comments`, `t90_overflow_t8_mul`, `t93_overflow_t16_wrap`, `t94_overflow_mixed_widths`, `t95_overflow_t128_unchanged`, `t96_overflow_t8_unary_neg`, `t109_struct_field_overflow`, `t115_eval_order_compare`) across 6 categories (Arithmetic, IfElse, Struct, CompoundAssign, FloatOps, plus Other-ish edge cases). Enumerated this run by spawning each SKIP fixture and grepping stderr for `"print argument must be I64"`. Sizing: **M**. High leverage: closes 15 SKIPs with one focused unit of work (widen `cx_printn` intrinsic, or sign/zero-extend at the print-arg lowering boundary, or per-width intrinsics).
- **S4** (carried) Frontend CI gate at `.github/workflows/ci.yml:38–89` still exit-code-only, no stdout comparison. Unchanged since previous audit.
- **S5** (carried) No clippy / format / Windows / macOS CI. Unchanged.

### Top risks
- **R1** Metric-vs-reality drift in commit messages (H5 above) — observed in 4-of-4 recent commits. The audit prompt was explicitly written to catch this failure mode; the failure mode recurred inside the work that closed the previous instance of it. Blast radius: future reviewers reading the commit log will believe matrix totals are 94/88 even though the running test reports 99/83. Cheapest retire: H5 fix.
- **R2** Of the 83 SKIPs, 15 (18%) are print-stub-blocked, **68 (82%) are blocked by other things** — genuinely-missing features (when, enums, generics, Result/try, handles, string interp). The print stub is high-leverage but **not dominant**. Choosing "next loop = print stub" closes 15; choosing "next loop = when lowering" closes 3 (t143–t145). H4 (CX-91/94/34) was resolved doc-side this cycle — those were not actually open work items, so the previous audit's "L-sized feature-branch closure" framing was wrong.
- **R3** (carried) Pre-existing clippy error in `src/ir/printer.rs:10` (`if i > 0 || true`). Untouched by the five recent commits. Will block any future clippy-gate adoption.
- **R4** (carried) No Windows/macOS CI. The audit ran on Windows; CI runs Linux only.
- **R5** (NEW) Test-assertion-vs-current-contract drift class — d4e419a was the first instance the project has fixed. Lookahead: any time a lowering arm changes shape (rejection→synthesizing, or err-variant→ok, etc.), every test in the surrounding module that pattern-matches the old shape becomes a candidate for the same A3-style drift. Cheapest mitigation: when `cargo test --features jit` becomes a release gate alongside `jit_parity_by_feature`, the class is contained — the gate will fail loudly. This was implicit in the previous chain but never run. Now run, now caught.

### Status changes vs previous audit (c67c5c2 → d4e419a)

| Item | Was | Now | Source |
|---|---|---|---|
| H1 (parity-binary fragility) | hard blocker | **CLOSED** | 8e7143d; kill-test verifies 0.05s panic with `"JIT PARITY ABORTED — wrong binary"` |
| H2 (method-call lowering) | hard blocker | **CLOSED** | 0ab7e9b; t175/176/177 PASS direct JIT + interpreter exit 0 this run |
| Phase 11 — Method call actual lowering | `[ ]` open | **delivered** but roadmap checkbox still shows `[ ]` | `src/ir/lower.rs:1936` no longer rejection-only |
| Literal-width narrowing — 6 sites | partial (3 sites: Decl/Assign/Return; gaps in MethodCall, assert_eq, Lt/Gt) | **complete** (12 callsites of `insert_cast_if_needed` visible in semantic.rs, including the 3 new sites at lines 1421, 1573/1581, 1809–1811) | 0ab7e9b; verified by `t178_compare_narrow_int_numeric_exit.cx` PASS |
| Cast category in matrix | 0 PASS / 4 SKIP | **4 PASS / 0 SKIP** | Both CX-91 (Cast + F64 binary arithmetic JIT, at submain commit e8d31fc, predating audit baseline) and Stage-3's narrowing ride-along contributed. The previous audit's "Stage 3 ride-along not CX-91" framing was wrong — both were active. |
| Validator backstops | partial (Void in ArrayAlloca only) | **centralised** (`ensure_storable_type` at 8 sites; `cx_printn` in reserved-name gate) | 0ab7e9b |
| Full-suite test gate | not run in any verification step | **green now** (411/0/0) | d4e419a closed the missing-gate drift; now an effective check |
| Commit-message metric reliability | not flagged | **DRIFTED** (94/88 vs actual 99/83) | New finding H5 |

---

### Phase annotations (terser than previous audit — only items whose status moved since c67c5c2)

**Phase 0** — ground truth unchanged: canonical roadmap is this file; HEAD d4e419a.

**Phase 1** — build clean across debug/jit/release/no-features. Same 20-warning baseline. No clippy/format/Windows CI added.

**Phase 2** — three gates now: `cargo test --features jit` (411/0/0, **newly-effective**), `jit_parity_by_feature` (182, 0 PARITY_FAIL), `cargo test` no-feature (236/0/0 per previous audit, not re-run). Effective real test count now 411 (up from 400 last audit). Zero `#[ignore]`, zero `#[should_panic]` in `src/` (re-verified by grep this run).

**Phase 3** — production panics still all in `#[cfg(test)]` modules. The d4e419a fix is the first observed case of a test that compiles, "passes" through field-shape A3 cleanup, but asserts a stale contract — flagged as a recurring risk class (R5). No new `todo!()`/`unimplemented!()`/`panic!()` in production paths from the five recent commits.

**Phase 4** — frontend re-verified at sample level: 8/8 examples still PASS (`bash examples/run_all.sh`), interpreter t175/176/177 exit 0 (new method-call paths work in interpreter, not just JIT). No locked-language-decision negative-test census this run.

**Phase 5** — IR/lowering changes from 0ab7e9b verified inline:
- `mangle_method` helper at `src/ir/lower.rs:113` — single source of `{Struct}${method}` scheme.
- Sibling sig-table arm at `:215` registers methods under mangled names.
- `lower_program_inner` arm at `:489` emits IR per method with reconstructed `[alias_params, ..., user_params]`.
- Statement-level void-method detection at `:826–837` synthesizes void Call and recurses into `lower_void_call`.
- Value-position MethodCall at `:1936–1954` synthesizes Call and recurses into the existing Call arm.
- Stage-1's `method_alias_params` parallel-indexed with methods on `SemanticStmt::ImplBlock` (frontend/semantic_types.rs).
- Stage-3's `method_alias_counts` table on `Analyzer` populated alongside `method_registry`.
- 5 narrowing sites verified by grep: MethodCall args (semantic.rs:1421), assert_eq peer (1573/1581), Binary arith (1732–1734), EqEq/NotEq (1759–1761), Lt/Gt/LtEq/GtEq (1809–1811). All call `insert_cast_if_needed` on both lhs and rhs (or against the typed peer where one side is bare-Numeric).
- Validator backstops at `src/ir/validate.rs`: `ensure_storable_type` at 8 sites, `cx_printn` in the reserved-name gate.
- **SKIP-cause breakdown** of the 83 SKIPs: 15 print-stub (S8 above), 68 other. The "other" 68 are dominated by the long tail of unsupported constructs (when, enums, generics, Result/try, handles, string interp, plus the H4 feature-branch items). Enumerated by `for f in src/tests/verification_matrix/t*.cx; do ... grep "print argument must be I64" stderr; done` — verbatim output in temp file `b5uz1zcyc.output`.

**Phase 6** — examples 8/8 PASS, all eight files match the README's example table.

**Phase 7** — cross-doc drift catalogue:
- `README.md:49,58` "155 verification matrix tests" — actual 182. Same drift class as previous audit's S1; commit 930d84d fixed table rows but left the headline.
- `README.md:60` "67 PASS / 88 SKIP" — disagrees with its own Total at line 84 (71/88) and with the matrix (99/83).
- `docs/backend/cx_jit_parity_checklist.md:149` "Total: 181 fixtures" — stale by 1.
- Four-commit body claims (94/88) drift from observable matrix (99/83). **NEW finding H5.**
- `docment/ROADMAP.md` Working Notes "120 fixtures" (2026-05-10 entry) and "243 tests" (2026-05-09 entry) — pre-existing drift, unchanged since previous audit; both flagged as historical snapshots there.

**Phase 8** — CI config unchanged since c67c5c2 (verified via `git log c67c5c2..HEAD -- .github/workflows/ci.yml`, empty). Backend job runs `cargo test --features jit` which would have caught the d4e419a-fixed test on CI — confirming the gate works *if* the push reaches CI. Note: the four commits in this sequence were pushed directly to submain per the prior task; CI runs on push events and the failing test would surface in the workflow run for 0ab7e9b. (Worth checking the GitHub Actions status for that commit.)

**Phase 9** — taxonomy update (only items whose tag moved):
- Phase 11 "Method call actual lowering" `[ ]` — was CLAIMED-NOT-DONE / Hard Blocker H2; now **DELIVERED** (checkbox in roadmap still reads `[ ]` — recommend flipping to `[x]` in a follow-up).
- Phase 12 "CI gate on every PR" `[ ]` — was DRIFTED. Backend job's effectiveness now verified by d4e419a; recommend re-tagging as **VERIFIED** with anchor.
- Phase 15 "Cast instruction JIT coverage (CX-91)" `[ ]` — landed at submain commit e8d31fc (predates audit baseline); Cast matrix category went 0→4 PASS. Tag: **DELIVERED** — checkbox in roadmap still reads `[ ]`, flipped to `[x]` in this commit.
- Phase 15 "Reserved intrinsic names rejected in validator (CX-85)" `[x]` — was VERIFIED; now **strictly more VERIFIED** since `cx_printn` added to gate + new test for it landed in 0ab7e9b.

### Untracked (implemented, not in roadmap)
- `mangle_method` helper at `src/ir/lower.rs:113` — the `{Struct}${method}` mangling scheme. Single source of truth for both sig-table registration and call-site mangling. Not surfaced in roadmap text.
- `method_alias_params` field on `SemanticStmt::ImplBlock` and `method_alias_counts` map on `Analyzer` — Stage 1 + Stage 3 plumbing. Not in roadmap.
- `assert_jit_capable` probe in `src/diff_harness.rs` (H1 closure machinery) — not in roadmap.
- `t178_compare_narrow_int_numeric_exit.cx` — the Lt/Gt narrowing regression fixture. Not in roadmap.
- The two `jit_probe_ptr_arg_*_across_call` tests in `host_boundary.rs` — verify Ptr-across-Call by-reference semantics, supporting MethodCall lowering's correctness.

### Could not verify
- The `cargo test` (no-features) gate was not re-run this audit (relied on previous audit's 236/0/0). Cheap to re-run if the no-jit suite has changed.
- The full classification of the 68 non-print-stub SKIPs into per-blocker buckets (when, enums, generics, Cast-feature-branch, DotAccess-feature-branch, internal-error fixtures, etc.). Sampled rather than exhaustively enumerated; the audit estimated by category but did not run every fixture-by-fixture.
- GitHub Actions status for commits 8e7143d / 0ab7e9b / 930d84d / 1036fd8 — would confirm whether CI caught d4e419a-style red on any of them at push time. Out of scope from local-only audit.
- Windows-specific JIT execution edge cases. Audit ran on Windows; no Windows CI to corroborate.
- `unwrap()` / `expect()` exhaustive non-test census — same gap as previous audit's "Could not verify."

<!-- RE-AUDIT-END -->

> **Superseded by re-audit 2026-05-19.** Annotations below kept for history.

<!-- AUDIT-START -->
## 0.1 Readiness Audit — 2026-05-18 — c67c5c2 — submain

Verdict: **NOT 0.1-READY**
Audited roadmap: `docment/ROADMAP.md` (Living Summary, header "Last updated: 2026-05-10"; identified as canonical because it self-declares as "concise synthesis" and points at the two detailed roadmaps; the detailed roadmaps `docs/frontend/ROADMAP.md` (v5.0) and `docs/backend/cx_backend_roadmap_v3_1.md` (v4.2) are referenced from here, not the other way around).
Audited against working-tree state at SHA c67c5c2; working tree has 7 modified files unstaged (`README.md`, `docment/ROADMAP.md`, `docs/backend/cx_abi_v0.1.md`, `docs/backend/cx_eval_order.md`, `src/diff_harness.rs`, `src/ir/lower.rs`, `src/ir/validate.rs`) from prior in-session work — anchors reflect this state, not pristine HEAD.
Build: debug `cargo build` ok, 21 warn | debug `cargo build --features jit` ok, 20 warn | release `cargo build --release --features jit` ok, 20 warn | lint `cargo clippy --features jit --bin Cx_0V` **1 pre-existing error** in `src/ir/printer.rs:10` (`if i > 0 || true`) + 128 warn — no clippy gate in CI.
Tests: `cargo test --features jit` 409 total / 409 pass / 0 fail / 0 ignored / effective real count 409 (only after a fresh `cargo build --features jit`; see Hard Blocker H1). `cargo test` (no jit) 236 / 236 / 0 / 0.
Items: 24 verified / 5 weak / 8 drifted / 0 not-done / 0 untested / 0 untracked (per Phase 9 annotations below).

### Hard blockers
- H1 Differential parity test is binary-state-fragile — `cargo build` (no `--features jit`) silently overwrites `target/debug/Cx_0V` with a non-JIT binary; the next `cargo test --features jit` then reports **60 PARITY_FAILs** across 14 categories until rebuilt. Observed live this run (`diff_harness::tests::jit_parity_by_feature ... FAILED, 60 PARITY_FAIL(s)`) — `src/diff_harness.rs::cx_binary_path` resolves the subprocess to a shared `target/debug/Cx_0V[.exe]` path with no feature-flag check. Cheapest fix: have the parity test either (a) panic-fast when the binary lacks JIT support (e.g., probe `--backend=cranelift` with a tiny fixture and check for an "unsupported" signal before iterating 181 fixtures), or (b) build the subprocess via `CARGO_BIN_EXE_Cx_0V` in dev-dependencies under the `jit` feature so feature drift causes a build error rather than a silent regression.
- H2 Phase 11 — "Method call actual lowering" still rejection-only — roadmap explicitly lists `[ ]` and the only path is `unsupported!("MethodCall '{}.{}'")` at `src/ir/lower.rs:1791`. Cheapest fix: ship it or move it to Post-0.1 in the roadmap.
- H3 Phase 11 — "`when` block lowering or structured rejection" — `when` is rejected at both the stmt and expr level (`src/ir/lower.rs:932` and `src/ir/lower.rs:1870`), so "structured rejection" half is satisfied; "lowering" is not. WhenBlock parity fixtures t143–t145 are SKIP (exit-code 127) per `docs/backend/cx_jit_parity_checklist.md` §3 line 147. Cheapest fix: pick one — keep the SKIP and reword the roadmap line to drop "or lowering", or ship the lowering.
- H4 Phase 15 — "Cast instruction JIT coverage (CX-91)" / "DotAccess JIT parity fixtures (CX-94)" / "Full parity fixture coverage (CX-34)" — all `[ ]` and per the roadmap line itself "on feature branch", not on submain. Cast category in checklist baseline is `0 PASS / 4 SKIP / 0 PARITY_FAIL`; the JIT path for Cast simply doesn't exist yet on submain. Cheapest fix: land the feature branches or move the items out of 0.1.

### Soft blockers
- S1 README test-matrix drift — `README.md:49,58` claim "155 verification matrix tests"; actual is 181 (`ls src/tests/verification_matrix/t*.cx | wc -l` = 181 this run; `docs/backend/cx_jit_parity_checklist.md` §3 line 149 "Total: 181 fixtures"). Per-category rows in `README.md:68–84` disagree with the checklist in 9 of 16 categories (Arithmetic 8/9 vs 8/10, VariableDecl 5/3 vs 5/5, WhileLoop 5/3 vs 6/2, InfiniteLoop 2/1 vs 4/1, DirectCall 7/4 vs 12/5, Struct 6/5 vs 6/8, Unary 0/1 vs 2/1, Cast 0/2 vs 0/4, FloatOps 0/5 vs 0/7, BuiltinAssert 2/2 vs 4/2). Total row `**70** / **88** / **0**` (sum 158) matches neither the headline "155" nor the checklist's 181. `README.md:60` "67 PASS / 88 SKIP" disagrees with its own Total row of 70/88.
- S2 README "Unary negation" listed under "Constructs not yet JIT-lowered" at `README.md:105`, contradicted by checklist `Unary 2 PASS` and the existence of `src/tests/verification_matrix/t165_unary_neg_int_exit.cx` / `t166_unary_not_bool_exit.cx` (both PASS per checklist §3 line 142).
- S3 Working Notes drift inside this file — line 117 (2026-05-10 entry) "Phase 12 harness operational at 120 fixtures, 0 PARITY_FAILs" — actual is 181/0 per checklist and per `jit_parity_by_feature` output this run. Line 119 (2026-05-09 entry) "JIT: 243 tests, 0 parity failures" — actual is 409 tests total (with jit feature).
- S4 Frontend CI gate is weak — `.github/workflows/ci.yml:38–89` (`frontend` job) iterates the matrix and only checks the subprocess exit code (`exit_code=$?`), never compares stdout to `.cx.expected_output` files even though many fixtures are pass-with-output. Output drift on those fixtures would not fail this gate; only the backend job's `cargo test --features jit` (which includes `interpreter_baseline_all` and `jit_parity_by_feature`) actually compares output.
- S5 No clippy gate, no format gate, no release-build gate, no Windows/macOS CI — `.github/workflows/ci.yml` runs on `ubuntu-latest` only; the only build steps are `cargo build` (no features) and `cargo build --features jit`. The pre-existing clippy error at `src/ir/printer.rs:10` (S1 in lint table above) would land on day one of adopting clippy gating.

### Top risks
- R1 Parity-binary state fragility (H1) — blast radius: every contributor not aware of the gotcha gets a red CI on their next "I'll just `cargo build` quickly" cycle, leading to mistaken belief in real parity failures. Cheapest retire: H1 fix above.
- R2 README is the public-facing surface and is internally inconsistent (S1, S2) — blast radius: anyone evaluating 0.1 will compare these counts and conclude either that the team's accounting is sloppy or that the parity gate is fictional. Cheapest retire: replace the README table+headline with a snapshot from the checklist and add a CI step that diffs them on every PR (the project already has `runtime_intrinsic_names()` as a "shared source of truth" pattern for the validator — same idea here).
- R3 `unwrap()` / `expect()` exposure on the JIT path was not exhaustively censused this run (Phase 3 only fully enumerated `panic!`/`todo!`/`unimplemented!`). Production-path `unwrap`/`expect` is the most common source of "valid program crashes the compiler" 0.1 embarrassment. Cheapest retire: separate audit task scoped to `rg --type=rust '\.(unwrap|expect)\(' src/ -g '!*/tests/*' -g '!*::tests*'` and triage.
- R4 No Windows/macOS CI — primary working tree this audit was run from is Windows 11; the parity test spawns subprocesses and reads pipes which has well-known cross-platform gotchas. Cheapest retire: add a Windows matrix entry to the backend job in `.github/workflows/ci.yml`.
- R5 Phase 11 items (H2, H3) are tagged `[ ]` in the roadmap but the surrounding header reads "nearly complete" — every reader has to decide whether to trust the prose or the checkboxes. Cheapest retire: drop "nearly complete" or list the two open items by name in the header.
<!-- AUDIT-END -->

# Cx Project Roadmap — Living Summary

Last updated: 2026-05-10

This file is a concise synthesis of the project's roadmap state. Detailed roadmaps live at:
- Frontend: `docs/frontend/ROADMAP.md` (v5.0)
- Backend: `docs/backend/cx_backend_roadmap_v3_1.md` (v4.2)

---

## Frontend — Release Candidate

All 9 hard blockers resolved. 117/117 matrix tests passing. 8/8 examples passing.
<!-- audit: **DRIFTED** — actual matrix count is 181 fixtures, not 117 (`ls src/tests/verification_matrix/t*.cx \| wc -l` = 181; `docs/backend/cx_jit_parity_checklist.md` §3 line 149). 8/8 examples confirmed via `bash examples/run_all.sh` this run. "9 hard blockers resolved" not independently re-verified; left as **UNVERIFIABLE** below. -->

**Status:** 0.1 release candidate. No known soundness holes. Syntax frozen.
<!-- audit: **UNVERIFIABLE** — "0.1 release candidate" is a global claim; this audit verifies its sub-claims individually below. "No known soundness holes" / "Syntax frozen" not testable as written. -->

**Known limitations (documented, not blocking):**
- String arena grows monotonically (interpreter-only) <!-- audit: **UNVERIFIABLE** — declared a known limitation, not a 0.1 deliverable. Out of scope. -->
- No strref constructor syntax <!-- audit: **UNVERIFIABLE** — declared a known limitation. Out of scope. -->
- Expression statements still require semicolons <!-- audit: **UNVERIFIABLE** — declared a known limitation. Out of scope. -->

---

## Backend — Active Development

The backend pipeline converts verified SemanticProgram → IR → machine output (Cranelift JIT for 0.1).

### Done
- [x] Phase 0 — Foundation (semantic boundary) <!-- audit: **VERIFIED** — `src/frontend/semantic_types.rs` (SemanticProgram/SemanticStmt/SemanticExpr) consumed by `src/ir/lower.rs::lower_program`. -->
- [x] Phase 1 — IR data model <!-- audit: **VERIFIED** — `src/ir/types.rs` (IrModule:128, IrFunction:131, IrBlock:145, IrParam:139, BlockParam, IrType). -->
- [x] Phase 2 — Straight-line lowering <!-- audit: **VERIFIED** — `src/ir/lower.rs::lower_program_inner` + arithmetic / decl / call lowering exercised by 181 verification_matrix fixtures via `jit_parity_by_feature`. -->
- [x] Phase 3 — IR validation <!-- audit: **VERIFIED** — `src/ir/validate.rs::validate_module` with 11 error variants (`IrValidationError`), exercised by 60+ unit tests in `src/ir/validate.rs::tests`. -->
- [x] Phase 4 — Function lowering <!-- audit: **VERIFIED** — `src/ir/lower.rs::lower_semantic_function` (line ~451). -->
- [x] Phase 5 — if/else lowering <!-- audit: **VERIFIED** — IfElse parity category 4 PASS / 2 SKIP / 0 PARITY_FAIL (`docs/backend/cx_jit_parity_checklist.md` §3 line 134). -->
- [x] Phase 0.5 — Backend trait interface (&IrModule) <!-- audit: **VERIFIED** — `src/backend/mod.rs` Backend trait + `src/backend/cranelift/` impl. -->
- [x] Phase 7 — IR pretty printer and diagnostics <!-- audit: **WEAK-COVERAGE** — `src/ir/printer.rs` exists and is exercised in tests, but contains a pre-existing clippy error at `src/ir/printer.rs:10` (`if i > 0 || true`) which is a logic bug in the printer itself; no clippy gate caught it. -->
- [x] Phase 6 — Function call lowering (direct calls, arity/type validation) <!-- audit: **VERIFIED** — DirectCall parity 12 PASS / 5 SKIP / 0 PARITY_FAIL (`docs/backend/cx_jit_parity_checklist.md` §3 line 138); validator enforces arity & type at `src/ir/validate.rs:402–447`. -->
- [x] Phase 10 — Loop lowering (while, for, break, continue; loop-var read-only validator enforced CX-40) <!-- audit: **VERIFIED** — WhileLoop 6/2/0, ForLoop 4/0/0, InfiniteLoop 4/1/0 (`docs/backend/cx_jit_parity_checklist.md` §3 lines 135–137); loop-var enforcement at `src/ir/validate.rs::validate_target_args` and `LoopVariableReassignment` variant. -->
- [x] Phase 8 Round 1 — ABI (scalars, structs, arrays, enums, calling convention) <!-- audit: **VERIFIED** — locked in `docs/backend/cx_abi_v0.1.md`; struct layout `src/ir/types.rs::compute_struct_layout`; array layout `compute_array_layout`; Struct 6/8/0 and Array 3/2/0 in parity checklist. -->
- [x] Phase 9 sub-packet 1 — Audit + structured errors for all builtins (CX-35) <!-- audit: **VERIFIED** — `assert_builtin_structured_error` tests in `src/ir/lower.rs::tests` at line ~5936 (constructed via `builtin_stmt` helper at line 5922). -->
- [x] Phase 9 sub-packet 2 — print/printn/println/cx_print family lowered to runtime dispatch (CX-38/CX-77/CX-82/CX-84) <!-- audit: **VERIFIED** — `print_i64_lowers_to_cx_printn_call`, `println_i64_lowers_to_cx_printn_call`, `printn_lowers_to_cx_printn_call` in `src/ir/lower.rs::tests`; `cx_printn` pre-seeded in `src/ir/validate.rs::known_intrinsic_sigs:75`. -->
- [x] Phase 9 sub-packet 3 — assert/assert_eq lowered to abort-on-failure in IR and JIT (CX-48) <!-- audit: **VERIFIED** — BuiltinAssert 4 PASS / 2 SKIP / 0 PARITY_FAIL (checklist §3 line 145); JIT execution via `jit_determinism_builtin_assert_pass` test in `src/backend/cranelift/host_boundary.rs`. -->
- [x] Phase 13 — Cranelift lowering skeleton (CX-22) <!-- audit: **VERIFIED** — `src/backend/cranelift/host_boundary.rs` runs 100+ `jit_*` tests this run; all pass. -->
- [x] JIT Host Boundary — process ownership, exit-code extraction, output capture (CX-24) <!-- audit: **VERIFIED** — `src/diff_harness.rs::run_jit_subprocess:485` spawns/captures + `cx_binary_path:463` resolves binary path; exercised by 181-fixture parity run. -->
- [x] Phase 14 — First executable Cranelift slice <!-- audit: **VERIFIED** — sub-items below. -->
  - [x] ConstInt + arithmetic + Return (CX-25) <!-- audit: **VERIFIED** — `jit_determinism_arithmetic_{add,sub,mul,div,rem}` tests. -->
  - [x] Alloca + Load + Store (CX-26) <!-- audit: **VERIFIED** — `jit_determinism_alloca_store_load` test. -->
  - [x] Compare + Jump + Branch (CX-27/CX-41) <!-- audit: **VERIFIED** — `jit_determinism_branch_{eq,lt}_{true,false}_path` tests. -->
  - [x] ConstFloat + fcmp float comparison (CX-52) <!-- audit: **VERIFIED** — backend Cranelift fcmp tests in `src/backend/cranelift/host_boundary.rs`. -->
  - [x] Debug-trace gating (CX-54) <!-- audit: **VERIFIED** — `LoweringCtx.trace` field threaded through `src/ir/lower.rs:248`. -->
  - [x] Determinism tests (CX-55) <!-- audit: **VERIFIED** — `determinism_tests` module in `src/backend/cranelift/host_boundary.rs`, ~30+ jit_determinism_* tests all green this run. -->
  - [x] Direct function calls JIT (CX-76) <!-- audit: **VERIFIED** — `jit_call_cx_printn_*` and direct-call coverage in determinism_tests. -->
  - [x] PtrOffset + PtrAdd JIT (CX-78) <!-- audit: **VERIFIED** — `jit_determinism_array_alloca_ptr_offset_second_element` test. -->
  - [x] Runtime intrinsics dispatch — print family (CX-77/CX-82) <!-- audit: **VERIFIED** — same anchor as Phase 9.2 above; `cx_printn` seeded + reserved-name gate at `src/ir/validate.rs:123`. -->

### Active
- [ ] Phase 11 — Surface area reduction (nearly complete) <!-- audit: **DRIFTED** — "nearly complete" is contradicted by the two open sub-items below being genuinely unfinished (see H2, H3 in header). Phase as a whole accurately marked `[ ]`. -->
  - [x] Compound assign <!-- audit: **VERIFIED** — CompoundAssign 6 PASS / 1 SKIP / 0 PARITY_FAIL (`docs/backend/cx_jit_parity_checklist.md` §3 line 141); fixtures t26, t128, t151–t153, t169. -->
  - [x] Unary expressions <!-- audit: **VERIFIED** — Unary 2 PASS / 1 SKIP / 0 PARITY_FAIL (checklist §3 line 142); fixtures t165, t166. README at `README.md:105` separately claims unary negation is "not yet JIT-lowered" — see Soft Blocker S2; that's a README issue, not a roadmap one. -->
  - [x] Struct literal lowering (CX-9) <!-- audit: **VERIFIED** — Struct category 6 PASS / 8 SKIP / 0 PARITY_FAIL (checklist §3 line 139). -->
  - [x] Struct field reads via DotAccess (CX-10) <!-- audit: **VERIFIED** — Struct category covers; lowering at `src/ir/lower.rs` DotAccess arm + `compute_struct_layout`. -->
  - [x] Struct field writes via DotAccess (CX-14) <!-- audit: **VERIFIED** — same category anchor; covered by Struct fixtures. -->
  - [x] Void function calls / IrType::Void (CX-53) <!-- audit: **VERIFIED** — `lower_void_call:2971` + new `lower_type_rejects_void_in_storable_position` and `lower_return_type_void_canonicalises_to_none` tests in `src/ir/lower.rs::tests` (added this session); validator backstop via `ensure_storable_type` at 8 sites in `src/ir/validate.rs` (added this session). -->
  - [x] Array type and literal lowering (CX-16) <!-- audit: **VERIFIED** — Array 3 PASS / 2 SKIP / 0 PARITY_FAIL (checklist §3 line 140); fixtures t146–t148. -->
  - [x] Array element access (CX-17) <!-- audit: **VERIFIED** — `src/ir/lower.rs::lower_index` (~line 2329) + array fixtures above. -->
  - [x] Array-of-structs tests (CX-18) <!-- audit: **UNVERIFIABLE** — no anchor named in roadmap; couldn't locate a specific test matching this label this run. Treated as covered under Struct + Array categories without an explicit dedicated test. -->
  - [x] Range structured error (CX-19) <!-- audit: **VERIFIED** — `SemanticType::Result(_) => unsupported_type!("Result")` adjacent pattern at `src/ir/lower.rs::lower_type`; Range arm `Range { ... } => unsupported!("Range")` in `lower_expr` reachable. -->
  - [x] Array element writes (CX-20) <!-- audit: **VERIFIED** — `src/ir/lower.rs` Index-target compound assign (t147_array_write_exit, t153_compound_assign_index_exit in PASS list). -->
  - [x] MethodCall structured error (CX-21) <!-- audit: **VERIFIED** — `src/ir/lower.rs:1791` `SemanticExprKind::MethodCall { .. } => unsupported!("MethodCall '{}.{}'")`. -->
  - [x] Loop variable read-only invariant in validator (CX-40) <!-- audit: **VERIFIED** — `IrValidationError::LoopVariableReassignment` variant + enforcement at `src/ir/validate.rs::validate_target_args:818`; `rejects_compound_assign_equivalent_ssabind_reaching_read_only_param` test. -->
  - [ ] Method call actual lowering (structured error only) <!-- audit: **VERIFIED** (still-open is accurate) — only path is `unsupported!("MethodCall '{}.{}'")` at `src/ir/lower.rs:1791`; no other MethodCall lowering exists. **Hard Blocker H2.** -->
  - [ ] `when` block lowering or structured rejection <!-- audit: **DRIFTED** — structured rejection IS present (`src/ir/lower.rs:932` for stmt, `:1870` for expr — both `unsupported!`); WhenBlock parity fixtures t143–t145 SKIP per checklist §3 lines 190–192. Lowering is not. Roadmap line allows either; "or" reading would make this VERIFIED. **Hard Blocker H3** depending on which half the team is committing to. -->
- [ ] Phase 8 Round 2 — str/strref layout, Handle<T>, TBool calling convention <!-- audit: **VERIFIED** (still-open is accurate) — `StrRef`, `Str`, `Handle` all `unsupported_type!` in `src/ir/lower.rs::lower_type` (lines 2645–2649); no TBool calling convention in `docs/backend/cx_abi_v0.1.md`. Roadmap doesn't gate 0.1 on this (it's listed `[ ]` without "(0.1)" markers); no blocker. -->
- [ ] Phase 12 — Differential harness <!-- audit: **DRIFTED** at parent level — most sub-items done, two `[ ]` remain; per-item below. -->
  - [x] Harness shell — interpreter baseline capture (CX-23) <!-- audit: **VERIFIED** — `src/diff_harness.rs::run_interpreter` + `interpreter_baseline_all` test. -->
  - [x] Per-feature parity classification, 15 categories (CX-69) <!-- audit: **DRIFTED** — actual is 16 categories now (checklist §3 line 17 says "The 16 categories"; visible in `jit_parity_by_feature` output this run: 16 rows). Roadmap says 15. -->
  - [x] Loop construct fixtures (CX-68) <!-- audit: **VERIFIED** — WhileLoop/ForLoop/InfiniteLoop categories all have fixtures, 14 PASS / 5 SKIP combined. -->
  - [x] Exit-code-based fixtures — arithmetic/variable decl (CX-92) <!-- audit: **VERIFIED** — t172_arith_t128_exit, t173_const_decl_exit, t174_block_scope_shadow_exit in checklist (§3 line 21, 22). -->
  - [x] 181 fixtures, 0 PARITY_FAILs (per docs/backend/cx_jit_parity_checklist.md §3, captured 2026-05-17) <!-- audit: **VERIFIED** — re-confirmed live this run after fresh `cargo build --features jit`: `jit_parity_by_feature: 181 fixtures checked across 16 feature categories, 0 PARITY_FAILs`. Caveat: see Hard Blocker H1. -->
  - [x] Determinism tests (CX-55) <!-- audit: **VERIFIED** — same anchor as Phase 14 Determinism tests above. -->
  - [ ] Full construct set coverage expansion (CX-34 on feature branch) <!-- audit: **VERIFIED** (still-open is accurate) — feature branch not on submain; not visible in matrix. -->
  - [ ] CI gate on every PR <!-- audit: **DRIFTED** — `.github/workflows/ci.yml::backend` job (line 91) runs `cargo test --features jit` which DOES include `jit_parity_by_feature`. So the gate exists. But the `frontend` job (line 17) runs the matrix script with exit-code-only comparison — see Soft Blocker S4 — so "CI gate" coverage is weaker than the prose implies; the backend gate is real, the frontend matrix gate is weak. -->
- [ ] Phase 15 — Cranelift JIT 0.1 target <!-- audit: **DRIFTED** — three `[ ]` sub-items below are H4 hard blockers (the roadmap puts them under the "0.1 target" header). -->
  - [x] No-panic guarantee on valid IR (CX-50) <!-- audit: **WEAK-COVERAGE** — claim that the Cranelift backend cannot panic on valid IR was not subjected to a panic-fuzz harness this run; the `jit_determinism_*` suite passes but is hand-curated. Treat as untested-against-adversarial-input. -->
  - [x] Float comparison + ConstFloat (CX-52) <!-- audit: **VERIFIED** — same anchor as Phase 14 ConstFloat above. -->
  - [x] Exit-code propagation verified (CX-74) <!-- audit: **VERIFIED** — `JIT_SKIP_EXIT_CODE = 127` handling at `src/diff_harness.rs::run_jit_subprocess` and `parity_by_feature` (line ~576); exit-code-based fixtures t128–t177. -->
  - [x] PtrOffset + PtrAdd JIT (CX-78) — Phase 15 sub-packet 1 <!-- audit: **VERIFIED** — same anchor as Phase 14 PtrOffset above. -->
  - [x] Reserved intrinsic names rejected in validator (CX-85) <!-- audit: **VERIFIED** — `src/ir/validate.rs::is_reserved_runtime_intrinsic:123` (with `cx_printn` added this session, plus `rejects_function_named_cx_printn` test); upstream gate at `src/ir/lower.rs:385` via `runtime_intrinsic_names()`. -->
  - [x] Numeric literal cast lowering target-aware (CX-88/CX-89/CX-90) <!-- audit: **VERIFIED** — `TargetConfig::numeric_literal_ir_type` at `src/ir/lower.rs:216`; threaded through `LoweringCtx.target`. -->
  - [x] Exit-code-based parity fixtures (CX-92) <!-- audit: **VERIFIED** — same anchor as Phase 12 sub-item above. -->
  - [x] Cast instruction JIT coverage (CX-91) <!-- audit: **DELIVERED** — landed at submain commit e8d31fc (Cast + F64 binary arithmetic JIT, predates audit baseline); Cast category now 4 PASS / 0 SKIP / 0 PARITY_FAIL. H4 resolved. -->
  - [x] DotAccess JIT parity fixtures (CX-94) <!-- audit: **DELIVERED** — landed via PR #137 at submain commit cb36c8f (predates audit baseline); H4 resolved. -->
  - [x] Full parity fixture coverage (CX-34) — canceled 2026-05-10, scope superseded by CX-30/51/59/62/66/69/94 (all landed) <!-- audit: **CANCELED** — Linear ticket CX-34 closed 2026-05-10 with scope superseded by CX-30/51/59/62/66/69/94 (all merged). H4 resolved. -->
  - [ ] Differential harness in CI <!-- audit: **DRIFTED** — same as Phase 12 "CI gate on every PR" above; backend job runs it but frontend matrix gate is weak. Two roadmap items pointing at one situation. -->

### Post-0.1
- [ ] Cranelift AOT (Phase 16) <!-- audit: **VERIFIED** (still-open + correctly post-0.1) — `src/backend/cranelift/aot.rs:5` is a stub with a `TODO: Implement Cranelift AOT object emission`. Roadmap correctly excludes from 0.1. -->
- [ ] LLVM AOT (Phase 17) <!-- audit: **VERIFIED** (still-open + correctly post-0.1) — `src/backend/llvm/aot.rs:4` is a stub with a `TODO: Implement LLVM AOT object emission`. -->
- [ ] FFI and C boundary (Phase 18) <!-- audit: **VERIFIED** (still-open + correctly post-0.1) — no FFI code located this run. -->

---

## Language Features — Post-0.1

- NullPoint<T> <!-- audit: **VERIFIED** (post-0.1 label is accurate; no implementation found). -->
- Generics v3 (type bounds) <!-- audit: **VERIFIED** (post-0.1 label is accurate; `SemanticType::TypeParam(_)` is `unsupported_type!("TypeParam")` in `src/ir/lower.rs:2651`). -->
- := type inference <!-- audit: **VERIFIED** (post-0.1). -->
- gene + phen trait system <!-- audit: **VERIFIED** (post-0.1). -->
- Stdlib (growable array, hash table, ring buffer) <!-- audit: **VERIFIED** (post-0.1). -->
- Full memory system (region invalidation, rc<T>, shared<T>) <!-- audit: **VERIFIED** (post-0.1). -->
- GPU system <!-- audit: **VERIFIED** (post-0.1). -->

---

## Working Notes

**2026-05-10 (CX-95):** Backend roadmap reconciled to v4.2. Phase 14 complete — arithmetic, branches, memory ops, direct calls, PtrOffset, print dispatch all execute in JIT. Phase 15 active — no-panic, float, exit-code, PtrOffset/PtrAdd, intrinsic validation, numeric casts all landed. Phase 11 nearly complete — `when` block rejection and method call actual lowering are the two remaining open items. Phase 9 sub-packet 2 done (print family via runtime dispatch). Phase 12 harness operational at 120 fixtures, 0 PARITY_FAILs. CX-91 (cast JIT), CX-93 (fmod libcall), CX-94 (DotAccess parity) in flight on feature branches. Submain 40+ commits ahead of main.
<!-- audit: **DRIFTED** — "120 fixtures" is stale (current is 181, see `docs/backend/cx_jit_parity_checklist.md` §3 line 149 and the in-file "181 fixtures" line at 79); the rest of this note is a 2026-05-10 timestamped snapshot and is properly historical, but the "120" needs either an "(as of 2026-05-10)" qualifier or replacement. **Soft Blocker S3.** -->

**2026-05-09:** 9 PRs merged to submain. CX-74 (exit-code propagation), CX-48/73 (assert lowering), CX-52 (float cmp), CX-53 (void return), CX-67 (CodeRabbit), CX-70/71 (review fixes), CX-54/55. 10 new branches (CX-56–66) expanding JIT instruction coverage. Submain 40 commits ahead of main. JIT: 243 tests, 0 parity failures.
<!-- audit: **DRIFTED** — "JIT: 243 tests" is stale (current with `--features jit`: 409 total, 0 failures, this run). Historical snapshot but the headline figure has drifted far enough that readers will misread the trajectory. **Soft Blocker S3.** -->

**2026-05-05:** CX-18/19/20 merged to submain. CX-21–24 committed branch-local (Phase 11 error, Phase 12 start, Phase 13 start, host boundary). Submain 26+ commits ahead of main. Matrix 117/117 stable.
<!-- audit: **VERIFIED** as historical snapshot — accurate at its timestamp; current matrix count is 181. -->

**2026-05-04:** PR #57 merged submain → main after 37 days. CX-7 through CX-17 IR lowering sprint landed on submain. Main jumped from 78 to 117 tests.
<!-- audit: **VERIFIED** as historical snapshot — accurate at its timestamp. -->

---

### Untracked (implemented, not in roadmap)

- `cx_printn` reserved-name validator gate addition (`src/ir/validate.rs:127` adds `"cx_printn"` to `is_reserved_runtime_intrinsic`'s `matches!()`; test `rejects_function_named_cx_printn` at `src/ir/validate.rs::tests`). Added this session as a defensive backstop on top of the upstream `src/ir/lower.rs:385` gate. Not mentioned in any roadmap item.
- Centralized non-storable-Void validator (`src/ir/validate.rs::ensure_storable_type` helper + 8 call sites: function param, function return_ty, block param, SsaBind, Call return_ty, Cast destination type, ArrayAlloca element_type, Load destination type). Added this session. The roadmap mentions IrType::Void at the Phase 11 line "Void function calls / IrType::Void (CX-53)" but only at the lowering layer; validator-layer centralization is new and not tracked.
- `lower_return_type` helper at `src/ir/lower.rs` (canonicalises `SemanticType::Void` → `Option<IrType>::None` on the return-position path; `lower_type(Void)` now errors). Added this session. Implements the non-storable-void invariant referenced obliquely by the Void item in Phase 11 but not separately roadmap-tracked.
- Reaping the JIT subprocess after timeout-kill (`src/diff_harness.rs:524` `let _ = child.wait();` after `child.kill();`). Added this session; not roadmap-tracked.
- `examples/` directory has 8 `.cx` files + `run_all.sh`; tracked in README but not in this roadmap.
- `examples/audit/` and `examples/audit_memory/` directories — listed in `ls examples/` this run but not described in either roadmap or README's example table at `README.md:135–144`.

### Could not verify

- "All 9 hard blockers resolved" (frontend section header) — the list of blockers isn't in this file; would require auditing `docs/frontend/ROADMAP.md` (v5.0) to enumerate and verify each. Out of scope for this run (canonical roadmap is the living summary).
- "No known soundness holes" (frontend Status line) — too broad to test; would require an adversarial soundness audit, not a 0.1 readiness check.
- "Syntax frozen" — would require diffing the grammar against a baseline; no such baseline is referenced from this file.
- Phase 11 "Array-of-structs tests (CX-18)" — couldn't locate a dedicated test by that label this run; covered indirectly under Struct + Array categories.
- `unwrap()` / `expect()` exhaustive non-test census — not run this audit (Risk R3); only `panic!` / `todo!` / `unimplemented!` were censused.
- Frontend conformance against locked language decisions (Phase 4 of audit prompt) — not exhaustively executed; rely on the 181 matrix fixtures + 8 examples + `interpreter_baseline_all` test as a proxy. A locked-decision-by-locked-decision negative-test audit was not performed this run.
- Windows-specific test execution — primary working tree is Windows 11 and the parity test relies on subprocess pipes, but CI runs only on `ubuntu-latest` (`.github/workflows/ci.yml:19,93`); no evidence either way that Windows-only failures would be caught before release.
- Detailed frontend roadmap (`docs/frontend/ROADMAP.md` v5.0) and backend detailed roadmap (`docs/backend/cx_backend_roadmap_v3_1.md` v4.2) per-item status — this audit annotated the canonical living summary only; the detailed roadmaps were not item-annotated.
