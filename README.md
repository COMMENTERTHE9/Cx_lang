# Cx

Cx is a systems language for game engines with deterministic memory, no garbage collector, and no borrow checker.

## The Problem It Solves

Engine code often needs predictable allocation, explicit lifetime control, and data movement you can reason about frame to frame. Teams building gameplay, tooling, and runtime systems usually end up mixing high-level ergonomics with low-level ownership concerns by hand. Cx is aimed at that gap: a language where memory behavior is visible, stable, and intentional without turning every feature into allocator plumbing. The goal is to make engine-facing code easier to reason about under load, at boundaries, and over long-lived runtime sessions.

## The Approach

Cx is built around arenas, handles, and explicit value movement. Owned values stay owned, handles give you stable indirection with stale-handle detection, and scoped arena cleanup keeps teardown deterministic. Unknown state is part of the language model rather than an afterthought, which lets control-flow rules stay explicit.

## A Code Taste

```cx
fnc spawn_enemy(kind: EnemyKind) {
    let h;
    h = Handle.new(kind)

    when kind {
        EnemyKind::Grunt => print("grunt"),
        EnemyKind::Elite => print("elite"),
        _ => print("unknown"),
    }

    print(h.val)
    h.drop()
}
```

```cx
fnc: t64 sum_range(n: t64) {
    let total: t64 = 0
    for i in 0..n {
        total += i
    }
    total
}
```

## Current Status

Cx 0.1 is at release candidate stage. Both execution paths are active.

### Interpreter — Release Candidate

The tree-walk interpreter is the reference implementation. All 0.1 language constructs are supported and tested.

- 155 verification matrix tests, all passing
- 8 examples passing (`bash examples/run_all.sh`)
- All 9 hard blockers resolved — syntax frozen, no breaking changes after 0.1
- Two full audits completed: parser/semantic/interpreter agreement (12 programs) and memory boundary soundness (12 programs)

### Cranelift JIT Backend — Phase 15 Active

The Cranelift JIT backend compiles Cx programs to native machine code. It is the 0.1 backend target and is in active development.

- 155 fixtures run through the differential harness
- **0 PARITY_FAILs** — every supported construct produces output identical to the interpreter
- 67 PASS / 88 SKIP across 16 feature categories
- ABI and data layout locked for x86-64 (scalar types, struct alignment, calling convention)
- Determinism tested and guaranteed on valid IR

**JIT parity by category (current baseline):**

| Category       | PASS | SKIP | PARITY_FAIL |
|----------------|------|------|-------------|
| Arithmetic     | 8    | 9    | 0           |
| VariableDecl   | 5    | 3    | 0           |
| IfElse         | 4    | 2    | 0           |
| WhileLoop      | 5    | 3    | 0           |
| ForLoop        | 4    | 0    | 0           |
| InfiniteLoop   | 2    | 1    | 0           |
| DirectCall     | 7    | 4    | 0           |
| Struct         | 6    | 5    | 0           |
| Array          | 3    | 2    | 0           |
| CompoundAssign | 2    | 2    | 0           |
| Unary          | 0    | 1    | 0           |
| Cast           | 0    | 2    | 0           |
| FloatOps       | 0    | 5    | 0           |
| BuiltinAssert  | 2    | 2    | 0           |
| LogicalOps     | 2    | 0    | 0           |
| Other          | 16   | 48   | 0           |
| **Total**      | **66** | **89** | **0** |

SKIP means the construct is not yet lowered to JIT codegen — it exits cleanly with an unsupported-construct error rather than producing wrong output. PARITY_FAIL (semantic divergence from the interpreter) is the hard gate: it must stay at zero.

**Constructs JIT-lowered and working:**
- Integer arithmetic, comparisons, compound assign (`+=`, `-=`, `*=`, `/=`, `%=`)
- Logical AND/OR with short-circuit
- Variable declarations, typed and inferred
- Control flow: `if`/`else`, `while`, `for` ranges (`0..n`, `1..=n`), `loop`/`break`/`continue`
- Direct function calls — arity/type validation, return value handling
- Struct literals, field read/write, struct-in-function
- Fixed-size arrays: stack allocation (`ArrayAlloca`), element read/write, array-in-function
- Runtime intrinsics: `print`, `println`, `printn` (i64 arguments), `assert`, `assert_eq`
- Integer type casting (numeric cast, target-aware)
- Void function returns, exit code propagation

**Constructs not yet JIT-lowered (SKIP):**
- `when` blocks (three-way TBool branching; design work needed)
- Method calls (`obj.method()`)
- `f64` floating-point operations
- Explicit cast (`as`) — core cast locked; full branch coverage in-flight
- Unary negation
- Enums, generics, `Handle<T>`, string arena operations, `Result<T>`/`?`

## Getting Started

**Build requirements:** Rust toolchain (stable). Cranelift JIT requires the `jit` feature.

```bash
# Build both interpreter and JIT backend
cargo build --features jit

# Run a program with the interpreter (default)
cargo run -- examples/hello.cx

# Run a program with the Cranelift JIT backend
cargo run --features jit -- --backend=cranelift examples/hello.cx

# Run the full test suite
cargo test --features jit

# Run JIT parity gate only
cargo test --features jit jit_parity_by_feature -- --nocapture
```

The interpreter and JIT produce identical output for all PASS fixtures. The differential harness enforces this.

## Examples

Eight working examples are in `examples/`:

| File | Demonstrates |
|------|-------------|
| `hello.cx` | Print and basic syntax |
| `fizzbuzz.cx` | Control flow, modulo |
| `fibonacci.cx` | Recursion, function calls |
| `structs_and_methods.cx` | Struct definitions, impl blocks, methods |
| `error_handling.cx` | `Result<T>`, `Ok`/`Err`, `?` propagation |
| `arrays_and_loops.cx` | Fixed arrays, for loops, range iteration |
| `generics.cx` | Generic functions and structs |
| `tbool_uncertainty.cx` | TBool three-state, unknown propagation, `when` |

Run all examples: `bash examples/run_all.sh`

## Language Features

**Fully supported (interpreter + JIT where noted):**
- Integer types: `t8`, `t16`, `t32`, `t64`, `t128` — signed, wrapping arithmetic
- Float type: `f64`
- Boolean: `bool` (two-state), `tbool` (three-state: true/false/unknown)
- `Handle<T>` — stable indirection with stale-handle detection
- Arenas and scoped cleanup — deterministic teardown
- Structs with `impl` blocks and multi-alias `impl (a: A, b: B)` forms
- Generics — single and multiple type parameters on functions and structs
- Arrays — fixed-size, stack-allocated, index read/write, iteration
- Control flow: `if`/`else`/`else if`, `while`, `for` range, `loop`, `break`, `continue`
- `when` blocks — pattern matching on enums and TBool (interpreter only)
- Functions with explicit return types, `return`, and implicit last-expression return
- `Result<T>` and `?` error propagation
- `assert(cond)` and `assert_eq(a, b)` test builtins
- Multi-file imports via `#![imports]` blocks
- String interpolation `{varname}` in print calls
- `const` declarations
- `#[test]` macro and `--test` mode

**Known 0.1 limitations (documented, not blocking):**
- String arena grows monotonically in the interpreter
- No `strref` constructor syntax — `strref` exists as a boundary type only
- Expression statements still require semicolons (all other statements have optional semicolons)
- Pattern matching named binding (`as v`) and guard clauses (`if n > 5`) are post-0.1

## Data Layout and ABI (Locked for x86-64)

The Cx 0.1 ABI is locked. Changes are breaking.

- Scalars: `t8`/`t16`/`t32`/`t64`/`t128`, `f64`, `bool`, `tbool`, `Ptr` — sizes and alignments defined
- `tbool` wire representation: false=0, true=1, unknown=2 (passed as i8)
- Structs: C-compatible alignment with natural padding
- Arrays: contiguous stack allocation via `ArrayAlloca`, element size derived from type
- Calling convention: C ABI / SystemV on Linux x86-64
- Expression evaluation order: left-to-right, documented and tested

See `docs/backend/cx_abi_v0.1.md` for the full specification.

## Roadmap

**0.1 — In progress**
- Interpreter: release candidate, all hard blockers resolved
- Cranelift JIT: Phase 15 active — expanding PASS coverage, no PARITY_FAILs
- Remaining JIT work: `when` blocks, method calls, `f64` ops, casts, enums, generics

**Post-0.1 — Deferred**
- Cranelift AOT compilation
- LLVM AOT backend
- C FFI surface
- gene + phen trait system and operator overloading
- Minimal stdlib (dynamic array, hashmap, string utilities)
- Filesystem I/O, windowing, GPU system
- LSP and tooling (`cx build`, `cx test`, `cx check`)

## Built With

- [Rust](https://www.rust-lang.org/) — implementation language for the compiler, interpreter, and tooling
- [Logos](https://github.com/maciejhirsz/logos) — tokenization
- [Chumsky](https://github.com/zesterer/chumsky) — parser construction
- [Cranelift](https://cranelift.dev/) — JIT code generation backend (0.115)

## Contributing / Contact

Open an issue or PR to discuss language behavior, runtime semantics, or backend work.

The verification matrix in `src/tests/verification_matrix/` is the clearest picture of what is working today. The JIT parity checklist is in `docs/backend/cx_jit_parity_checklist.md`. The full ABI specification is in `docs/backend/cx_abi_v0.1.md`.

See `CONTRIBUTING.md` for branch policy and merge workflow.
