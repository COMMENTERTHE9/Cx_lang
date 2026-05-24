# Pillar 1 — Performance: Where Does the Time Go

**Audit base:** `submain` @ `22015c420ad40483d0531d1109fa9837d44deca3` (the 0.1 line; one merge past the documented baseline `4d612df`).
**Date:** 2026-05-24.
**Scope:** the default build (tree-walking **interpreter**). The Cranelift JIT path is observation-only here — see §8.
**Companion data:** `bench/baseline_0_1.md` (all numbers), `bench/programs/` (12 programs), `bench/flamegraphs/` (SVGs + callgrind text).

---

## 0. Headline

Two findings dominate everything else. Both are in the interpreter, both have a clear fix, both belong at the top of the 0.2 roadmap:

1. **Every Cx function call allocates and zero-fills a 64 KB arena chunk.** On call-heavy code this is **90% of all instructions executed** — pure `memset` of memory that is then mostly unused and freed. `bench/flamegraphs/fib_recursive.svg`.
2. **Every variable read/write hashes the variable's name string with SipHash.** On loop/variable-heavy code this is **~26% of instructions** (`hash_one` + `Hasher::write`), before counting the `set_var`/`get_var`/clone/drop work it drives. `bench/flamegraphs/arith_loop.svg`.

The compiler frontend (lex/parse/semantic) is **not** a performance concern at 0.1 scale — it is sub-millisecond for every hand-written program. For 0.1, *performance means interpreter performance.*

---

## 1. Method and tooling

| Need | Tool chosen | Why |
|---|---|---|
| Per-phase compile time | in-tree `--debug-phase` (`PhaseTimer`, `Instant::now()` at boundaries, `src/main.rs:52-74`) | The pipeline has ~5 coarse phases; boundary timing is sufficient and zero-dependency. `tracing`/`tracy-client` would be over-instrumentation for five phases. |
| Function-level hotspots | **`valgrind --tool=callgrind` 3.22** | `perf` and `samply` are unavailable in the container and `perf_event_paranoid=2` blocks `perf` without privilege. Callgrind is deterministic (instruction counts, reproducible run to run), needs no privilege, and resolves Rust symbols. Trade-off: counts instructions retired (Ir), not wall-clock, and runs ~30× slower — so profiles are taken on scaled-down inputs (fib(22), 150 k-iter loop) with the same hot path. |
| Flamegraph rendering | **`inferno` 0.12.6** (`cargo install`) fed from `callgrind_annotate --inclusive=no` self-costs via `bench/callgrind_to_folded.py` | No `perf` → no sampled stacks. We render an honest *self-cost* flat flamegraph; folded totals match callgrind program totals exactly (4,210,260,227 and 1,126,336,921 Ir). |

**Tooling added to the repo (this pillar):** `inferno` (installed into the toolchain, not vendored); `bench/callgrind_to_folded.py` (committed). No production dependencies were added. `criterion`/`dhat` were evaluated but not adopted this pass — callgrind already answered the "where" question decisively; see §9 for where they'd add value next.

All measurements use `cargo build --release`. Debug-build numbers (≈2.5–6.5× slower) are in `bench/baseline_0_1.md §3` for reference only.

---

## 2. Where time goes: frontend vs. runtime

Across all 12 benchmarks, **100% of wall time is in RUNTIME**; the frontend never registers above noise.

- Hand-written programs: lex+parse+semantic total **< 1 ms** each.
- Even the synthetic worst case (`compile_stress.cx`: 400 functions, 17,299 tokens, 3,614 lines) compiles in **PARSER 22.4 ms : SEMANTIC 4.4 ms : LEXER 1.3 ms** and then runs its trivial body in 3.3 ms.

So the frontend is fast and scales acceptably; parsing is the steepest-scaling phase and the natural place to look *if/when* compile time ever matters (e.g. an LSP doing repeated compiles). Two minor frontend notes:

- **Parser construction floor (~0.27 ms):** even a 6-statement program spends ~0.27 ms in PARSER. This is the one-time cost of building the chumsky combinator graph (40 `.boxed()` allocations, 4 `recursive` closures in `src/frontend/parser.rs`), rebuilt fresh on every process start and never cached. Irrelevant for a batch CLI; relevant for any future long-lived/incremental compiler.
- The combinator graph is the reason PARSER ≫ LEXER/SEMANTIC at every size.

## 3. Runtime hot path A — per-call 64 KB arena zeroing (the big one)

**Evidence.** `callgrind_annotate` on fib(22) (≈57 k calls):

```
3,806,635,641 (90.41%)  __memset_avx2_unaligned_erms        [libc]
  ... reached via call_semantic_func'2 -> __rust_alloc_zeroed
      -> calloc -> memset, 58,067 times (once per call)
```

**Root cause.** `RunTime::push_function_scope` (`src/runtime/runtime.rs:122-136`) builds a `ScopeFrame` with `arena: Some(Arena::new())`. `Arena::new()` (`src/runtime/arena.rs:40-45`) eagerly creates one `Chunk::new(65536)`, and `Chunk::new` does `data: vec![0u8; size]` (`arena.rs:11-16`) — a **zeroed 64 KB heap allocation per function call**. The arena only ever services `alloc_str` / `track_in_arena` size-accounting (`runtime.rs:49-84`); a function like `fib` that allocates no strings touches none of it, so the full 64 KB is allocated, zeroed, never read, and freed on `pop_scope`.

This single fact explains the entire call-cost column of the cost ladder (§5): function calls 2.9 µs, method calls 2.3 µs, Result/`?` round-trips 6.3 µs (two calls + Ok/Err), mixed_sim 7.8 µs/tick (two method calls). It is also why release optimization helps the arithmetic loop (6.5×) far more than fib (2.5×) — `memset` doesn't optimize away.

**Fix directions (in increasing reward / effort).**
1. **Lazy first chunk.** `Arena::new()` starts with `chunks: Vec::new()`; `alloc` allocates the first chunk on first real use. Functions that never allocate pay nothing. ~5 lines, removes ~90% of call cost for non-allocating functions. *Cheapest high-impact win.*
2. **Don't zero.** The arena hands out raw `*mut u8` that callers fully initialize before reading (`alloc_str` copies over every byte). The `vec![0u8; n]` zeroing is unnecessary; a `Vec::with_capacity` + bump (or `Box::new_uninit_slice`) avoids the `memset` even when the chunk *is* used.
3. **Pool / reuse arenas** across calls instead of alloc-on-push / free-on-pop, so steady-state recursion reuses one buffer.

> This is a hot-path redesign, not a one-line bug, so per the audit rules it is **reported, not changed**. It is the strongest single 0.2 candidate. Recommend prototyping fix #1 behind a benchmark; expect a multiple-× speedup on all call-heavy programs.

## 4. Runtime hot path B — variable access via String-keyed HashMap + SipHash

**Evidence.** `callgrind_annotate` on the bare arithmetic loop (no function calls):

```
164,701,807 (14.62%)  core::hash::BuildHasher::hash_one
127,801,310 (11.35%)  <sip::Hasher as Hasher>::write     # => ~26% SipHash
124,200,000 (11.03%)  RunTime::set_var
 82,800,174 ( 7.35%)  RunTime::get_var
 68,400,263 ( 6.07%)  drop_in_place<Value>
 45,900,170 ( 4.08%)  <Value as Clone>::clone
 27,450,135 ( 2.44%)  __memcmp_avx2_movbe                 # String key compare
```

**Root cause.** The variable environment is `ScopeFrame.vars: HashMap<String, VarEntry>` (`src/runtime/runtime.rs:24-25`), keyed by the variable's *name string*, using the std default hasher (**SipHash**, DoS-resistant and slow). Every `i += 1` hashes `"i"` (read + write), compares string keys on collision (`memcmp`), and clones/drops `Value`s. For a language with statically known locals this is the wrong data structure: the work is spent re-deriving, every access, information the semantic phase already has.

**Fix directions.**
1. **Resolve variables to slot indices at semantic time.** The semantic phase already walks every scope; have it assign each local a frame slot, and store locals in a `Vec<VarEntry>` indexed by `u32`. Variable access becomes an array index — no hashing, no string compare, no per-access clone of the key. *This is the real fix and compounds with the JIT story.*
2. **Cheap interim:** swap the hasher (`FxHashMap`/`ahash`) — kills most of the 26% SipHash cost with a one-line type change, no semantics change. Good stopgap before slot resolution lands.
3. Reduce `Value` clone/drop churn (6% + 4%): `get_var` returns a cloned `Value`; many uses only need a read. A `Copy`-friendly small-value representation or borrow-returning accessor would cut this.

## 5. Per-operation cost ladder (release, steady-state)

| Operation | µs | Notes |
|---|---:|---|
| nested-loop inner iter | 0.64 | cheapest; few var touches |
| arith loop iter | 0.82 | dominated by SipHash on locals (§4) |
| float iter | 1.04 | + numeric cast path |
| `when` range match | 1.17 | range compare dispatch |
| struct method call | 2.28 | call cost (§3) + impl lookup |
| function call (recursive) | 2.93 | call cost (§3) |
| array indexed load | 3.56 | **~4–5× an arith iter — surprising** (see §7) |
| Result + `?` round-trip | 6.28 | two calls + Ok/Err alloc |
| mixed sim tick | 7.83 | two method calls + branches |

## 6. Instrumentation gaps found

`--debug-phase` does **not** time three things; closing these is cheap and worth doing before the next perf pass (all are call-site wraps in `src/main.rs`, frontend territory):
- **RESOLVE** (`resolver::resolve`, `src/main.rs:157-164`) — no `PhaseTimer`. Empty for single-file programs but will matter once multi-file imports are exercised.
- **IR lowering** (`prepare_ir`, `src/main.rs:246/280`) and **codegen/execute** (backend `.execute`) — untimed, so the JIT pipeline's compile cost is currently invisible.

## 7. What surprised me

- **Indexed array access (3.56 µs) costs ~4–5× a plain arithmetic iteration.** A bounds-checked load + offset should be cheap; this suggests the index path re-walks the scope to fetch `arr`, hashes the name, clones the array `Value`, *then* indexes — i.e. it inherits hot-path B and possibly clones the whole array. Worth a dedicated profile in Pillar 2/3.
- The arena exists to make Cx "arena-allocated per function scope," but for the common case (no string/container allocation) it is **pure overhead** — it makes calls dramatically *slower*, the opposite of its intent.
- The frontend is so fast that **compile time is a non-issue at 0.1**; I expected semantic analysis to show up and it never did.

## 8. Backend coordination (JIT)

Measuring the Cranelift JIT's *compile* cost and *generated-code* runtime is out of scope for observation-only work: the IR-lowering and codegen phases are uninstrumented (§6), and meaningful JIT perf numbers need the backend owner's input on what to measure (codegen time vs. execution time, warmup, etc.). **Flagging for Zara → backend dev:** if 0.2 wants JIT perf baselines, I need (a) agreement to add phase timers around `prepare_ir`/`execute` in `src/main.rs`, and (b) a short conversation on which JIT metrics matter. The interpreter findings above are independent of the JIT and stand on their own.

## 9. Recommendations for 0.2 (evidence-ranked)

1. **Lazy / non-zeroing arena** (§3, fix #1/#2). Highest impact, lowest effort. Kills the 90% memset on all call-heavy code.
2. **Slot-indexed local variables** (§4, fix #1). Removes per-access SipHash + string compare + key clone; also the right foundation for faster JIT locals. Larger change; design with the semantic phase.
3. **Swap the hasher to FxHash/ahash** (§4, fix #2) as an immediate stopgap landing before #2.
4. **Profile the array-index path** (§7) — likely a quick win once hot path B is understood.
5. **Add RESOLVE + IR-lowering + codegen phase timers** (§6) so the next perf pass — and any JIT work — has data.
6. **Adopt `criterion`** for a small set of phase-level micro-benchmarks (lex/parse/semantic on fixed inputs) and **`dhat`** (behind a feature) to get per-phase *allocation counts* — the one dimension callgrind didn't quantify and the task explicitly asks about. Defer until after #1–#2 so we measure the improved baseline.
