# Cx Performance Baseline — 0.1

**Recorded:** 2026-05-24
**Commit:** `22015c420ad40483d0531d1109fa9837d44deca3` (branch `submain`, the 0.1 line; one merge — #282 — past the audit baseline `4d612df`)
**Binary:** `cargo build --release` (optimized, default features / interpreter path)
**Host:** Linux 6.18.5 cloud container; single-threaded interpreter on a 64 MB stack thread.
**Method:** wall time via shell `time`; per-phase time via the in-tree `--debug-phase` instrumentation (`PhaseTimer`, `Instant::now()` at phase boundaries). Numbers are single representative runs unless a range is given; runtime figures are steady-state (each program loops millions of times within one process, so first-iteration warmup is amortized to noise).

> Every number here is a baseline to be **measured against**, not a target. Re-run with the same commit + `--release` to compare.

---

## 1. Compile-time, per phase (frontend)

The interpreter pipeline is LEXER → PARSER → RESOLVE → SEMANTIC. RESOLVE is **not** instrumented by `--debug-phase` (see report/performance.md §"instrumentation gaps"); for these single-file programs it is empty. All times in milliseconds.

| Program | tokens | LEXER | PARSER | SEMANTIC |
|---|---:|---:|---:|---:|
| arith_loop | 36 | 0.02 | 0.27 | 0.03 |
| fib_recursive | 40 | 0.02 | 0.27 | 0.03 |
| fib_iterative | 97 | 0.03 | 0.40 | 0.04 |
| float_math | 47 | 0.03 | 0.31 | 0.03 |
| nested_loops | 49 | 0.03 | 0.26 | 0.04 |
| array_ops | 572 | 0.05 | 0.65 | 0.06 |
| struct_methods | 94 | 0.04 | 0.31 | 0.06 |
| when_tbool | 79 | 0.03 | 0.38 | 0.03 |
| result_chain | 108 | 0.05 | 0.40 | 0.05 |
| string_interp | 33 | 0.03 | 0.28 | 0.03 |
| mixed_sim | 160 | 0.05 | 0.49 | 0.11 |
| **compile_stress** | **17299** | **1.33** | **22.41** | **4.41** |

**Observations**
- For every hand-written program the entire frontend is **sub-millisecond**. Parsing is the largest frontend phase by a wide margin.
- There is a ~0.27 ms **parser floor** independent of input size (6-statement programs still cost 0.27 ms). This is the one-time cost of constructing the chumsky combinator graph (40 `.boxed()` allocations, 4 `recursive` closures in `src/frontend/parser.rs`); it is rebuilt on every process start and never cached.
- At scale (compile_stress: 400 functions, 17.3 k tokens) the split is PARSER 22.4 ms : SEMANTIC 4.4 ms : LEXER 1.3 ms ≈ **17 : 3.4 : 1**. Parsing dominates and is the phase that scales most steeply.

## 2. Runtime (interpreter, steady-state)

`RUNTIME` is the `--debug-phase` runtime figure; `wall` is end-to-end process time. Derived per-op cost = RUNTIME / work-unit.

| Program | work | RUNTIME (ms) | wall (s) | derived |
|---|---|---:|---:|---|
| arith_loop | 5.0 M loop iters | 4 085 | 4.09 | **0.82 µs / iter** |
| nested_loops | 4.0 M inner iters | 2 568 | 2.57 | **0.64 µs / iter** |
| float_math | 3.0 M iters | 3 126 | 3.14 | **1.04 µs / iter** |
| when_tbool | 2.0 M range-matches | 2 336 | 2.34 | **1.17 µs / match** |
| struct_methods | 2.0 M method calls | 4 557 | 4.56 | **2.28 µs / method call** |
| fib_recursive | fib(30) ≈ 2.69 M calls | 7 866 | 7.87 | **2.93 µs / call** |
| array_ops | 5.12 M indexed loads | 18 248 | 18.35 | **3.56 µs / indexed load** |
| result_chain | 1.0 M Ok/`?` round-trips | 6 281 | 6.29 | **6.28 µs / round-trip** |
| mixed_sim | 1.0 M update ticks | 7 825 | 7.83 | **7.83 µs / tick** (2 method calls + branches) |
| fib_iterative | 200 k calls × ~30-iter loop | 6 343 | 6.38 | call + loop mix |
| string_interp | 50 k interpolated prints (stdout → /dev/null) | 127 | 0.14 | **2.54 µs / print** |
| compile_stress | trivial (12 calls) | 3.34 | 0.01 | n/a (compile-bound) |

**Observations**
- 100 % of wall time on every benchmark is in RUNTIME; the frontend never registers. **For 0.1, "performance" means interpreter eval-loop performance.**
- Cost ladder (cheapest → dearest per operation): bare loop iter 0.6–0.8 µs < float/when 1.0–1.2 µs < method call 2.3 µs ≈ function call 2.9 µs < indexed array load 3.6 µs < Result/`?` round-trip 6.3 µs.
- **Function/method calls cost ~3–4× a loop iteration.** fib_recursive vs the bare loop isolates call overhead.
- **Indexed array access (3.6 µs) is ~4–5× a plain arithmetic iteration** — surprising for what should be a pointer + offset.
- The interpreter requires a **64 MB stack thread** (`src/main.rs:76-88`) because native Rust recursion is used for Cx calls; the default 1 MB stack cannot run `fib(8)`. Recursion depth is bounded by host stack, not a Cx-level limit.

## 3. Debug vs release (interpreter)

| Program | debug wall | release wall | speedup |
|---|---:|---:|---:|
| arith_loop (5 M) | 26.4 s | 4.09 s | 6.5× |
| fib_recursive fib(30) | 19.6 s | 7.87 s | 2.5× |

Release optimization helps the arithmetic loop more than the call-heavy path, consistent with call overhead being dominated by allocation/hashing work that optimizes less.

## 4. Tooling

- **Per-phase compile timing:** in-tree `--debug-phase` (`Instant::now()` at boundaries). Chosen over tracing/tracy because the pipeline has ~5 coarse phases — boundary timing is sufficient and zero-dependency. Gap: RESOLVE and the IR-lowering/codegen (JIT) phases are uninstrumented.
- **Hotspot profiling:** `valgrind --tool=callgrind` (3.22). `perf`/`samply` are unavailable in the container and `perf_event_paranoid=2` blocks `perf` without privilege; callgrind is deterministic, needs no privilege, and resolves Rust symbols. See `bench/flamegraphs/`.
- **Flamegraph rendering:** `inferno` (0.12.6, `cargo install`).
- See `report/performance.md` for the full tooling rationale and analysis.
