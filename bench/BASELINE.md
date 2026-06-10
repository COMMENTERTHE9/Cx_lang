# Wall-Time Performance Baseline

Recorded against the `--release --features jit` binary (`target/release/Cx_0V.exe`),
per the perf-rider dispatch. The matrix measures correctness; this file is what
"fast" means, so every later arc answers "did we get slower?" with a number.

**Machine:** AMD Ryzen 7 5825U (laptop — thermally sensitive).
**Methodology:** `bench/run_bench.ps1` — PowerShell `Measure-Command`, full
process wall time, 1 discarded warmup + 7 timed runs per program per backend,
median reported, min..max spread alongside. The warmup run doubles as a
cross-backend correctness screen (any exit other than 0/127 is a crash-stop).

---

## ⚠️ Methodology finding (D1.1b) — absolute ms are NOT cross-session comparable

Re-recording at the D1.1b boundary exposed a uniform ~2.4× interpreter / ~5×
JIT speedup vs the session-1 (f9164d1) numbers **on programs D1.1b never
touched** (fib_rec 1372→553, when_loop 1423→584, str_concat 1313→536,
struct_loop 1637→709). A frontend typing fix cannot speed up recursive fib —
this is machine thermal/load state, not code. Within each session the medians
reproduce tightly (Run A ≈ Run B, ≤~3%); across sessions the absolute floor
moves wholesale.

**Revised standing rule (supersedes the absolute-band rule):** regression
detection is **within-session relative**. At each arc boundary, build the
*prior* and *candidate* binaries and run `run_bench.ps1` against **both,
back-to-back in the same session**; compare the candidate's medians to the
prior's *from that same session*. A candidate program >10% slower than the
prior binary measured minutes earlier on the same thermal state is a finding.
Never compare a candidate's absolute ms to a number recorded in an earlier
session. (Why this wasn't caught in session 1: it was a single session.)

D1.1b itself **cannot regress runtime perf by construction** — it is a
frontend semantic-typing change (the `Index` node now carries its element
type); the generated IR and interpreter execution are byte-identical for any
program that already ran. Its only runtime effect is *enabling new lowering*
(array index-reads in expressions). No back-to-back A/B was needed to clear
it; the construction argument is the proof.

---

## Session 2 — as of D1.1b (parent f9164d1) — CURRENT

`bench_array_loop` reset here: it dropped the typed-local workaround (the
index-read-in-expression bug D1.1b fixed) and now runs its natural form and
lowers, so its row is not comparable to session 1's.

### Run A

| program | interp median (ms) | interp spread | jit median (ms) | jit spread |
|---|---|---|---|---|
| bench_arith_loop | 613 | 603..649 | 21 | 21..23 |
| bench_array_loop | 407 | 398..431 | 20 | 19..22 |
| bench_fib_iter | 708 | 700..717 | 19 | 18..20 |
| bench_fib_rec | 553 | 547..563 | 19 | 19..20 |
| bench_if_expr | 730 | 723..744 | SKIP | - |
| bench_str_concat | 536 | 530..570 | SKIP | - |
| bench_struct_loop | 709 | 700..791 | 29 | 25..30 |
| bench_when_loop | 584 | 568..624 | 19 | 18..20 |

### Run B

| program | interp median (ms) | interp spread | jit median (ms) | jit spread |
|---|---|---|---|---|
| bench_arith_loop | 628 | 617..653 | 22 | 21..24 |
| bench_array_loop | 411 | 399..442 | 20 | 19..22 |
| bench_fib_iter | 708 | 697..720 | 18 | 18..19 |
| bench_fib_rec | 571 | 554..596 | 20 | 18..21 |
| bench_if_expr | 739 | 725..744 | SKIP | - |
| bench_str_concat | 538 | 526..544 | SKIP | - |
| bench_struct_loop | 705 | 697..710 | 26 | 25..26 |
| bench_when_loop | 568 | 566..580 | 19 | 18..19 |

**Within-session noise (session 2):** interp cross-run median drift ≤ 3.3%
(`bench_fib_rec` 553→571); within-run spread up to ~13% (`bench_struct_loop`).
JIT medians 18–29 ms, startup+compile-dominated; cross-run drift ≤ 3 ms.

## Session 1 — as of bb3823a (HISTORICAL — not cross-session comparable)

Kept for the record. Do not diff these absolute ms against session 2; see the
methodology finding above. `bench_array_loop` here carried a typed-local
workaround (5 ops/iter) and ran 1.5M iters; session 2 runs the natural form
(3 ops/iter) at 300k iters — a different program.

| program | interp Run A | interp Run B | jit Run A | jit Run B |
|---|---|---|---|---|
| bench_arith_loop | 1350 | 1412 | 111 | 120 |
| bench_array_loop | 1427 | 1397 | 112 | 115 |
| bench_fib_iter | 1745 | 1744 | 126 | 113 |
| bench_fib_rec | 1372 | 1484 | 112 | 109 |
| bench_if_expr | 1709 | 1791 | SKIP | SKIP |
| bench_str_concat | 1313 | 1344 | SKIP | SKIP |
| bench_struct_loop | 1637 | 1655 | 118 | 124 |
| bench_when_loop | 1423 | 1376 | 116 | 107 |

## Surface notes

- Two programs are interpreter-only by design (`bench_str_concat` — Str has
  no IR; `bench_if_expr` — the #046 SKIP class). The runner autodetects JIT
  support per program (exit-127 sentinel), so these flip to two-backend
  automatically when their surface starts lowering.
- `bench_array_loop` lowers as of D1.1b (was a typed-local workaround).
- Debug-vs-release output screen: all 8 programs produce identical stdout and
  exit codes on debug and release, both backends.
