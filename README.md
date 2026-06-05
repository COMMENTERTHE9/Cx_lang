# Cx

Cx is a compiled, GC-free systems programming language for game engines, tools, and systems programmers.

The goal of Cx is to give engine-facing code predictable memory behavior, explicit value movement, deterministic teardown, and low-level control without forcing every feature to become allocator plumbing.

**Current release: v0.1.0**

Cx 0.1.0 is the first end-to-end language milestone: source code now moves through parsing, semantic analysis, IR, a reference interpreter, and a Cranelift JIT backend with parity verification.

0.1.0 is not production-ready yet.  
It is the first working compiler foundation.

---

## Why Cx Exists

Engine and systems code often needs:

- predictable allocation
- explicit lifetime control
- stable data layout
- deterministic cleanup
- value movement that can be reasoned about frame to frame
- runtime behavior that does not depend on hidden garbage collection

Most teams end up mixing high-level ergonomics with low-level ownership rules by hand.

Cx is aimed at that gap.

It is designed around explicit memory behavior, stable handles, arena-oriented allocation, declared-width arithmetic, and a language model where unknown state is represented directly instead of being hidden behind ad hoc runtime conventions.

---

## What 0.1.0 Proves

Cx 0.1.0 establishes the first working compiler pipeline:

```text
source code
→ lexer/parser
→ AST
→ semantic analysis
→ IR
→ interpreter
→ Cranelift JIT
→ parity verification
```

The interpreter is the reference semantics backend.

The Cranelift JIT is the native execution backend.

The differential harness checks that supported JIT features produce the same observable behavior as the interpreter.

---

## Current Verification Status

Cx 0.1.0 closed with:

- **411 unit tests passing**
- **182 verification fixtures**
  - 120 PASS
  - 62 SKIP
  - 0 PARITY_FAIL
- **zero compiler warnings**
- **zero Clippy errors**

`SKIP` means a fixture covers a language feature that is not yet lowered to JIT codegen.  
`PARITY_FAIL` means semantic divergence between interpreter and JIT. That number must stay zero.

---

## Code Taste

```cx
fnc: t64 sum_range(n: t64) {
    let total: t64 = 0

    for i in 0..n {
        total += i
    }

    total
}
```

```cx
fnc: t64 abs_or_zero(x: t64) {
    when x {
        0 => 0,
        -10..=-1 => -x,
        _ => x,
    }
}
```

```cx
fnc: bool is_large(n: t64) {
    if n > 100 {
        true
    } else {
        false
    }
}
```

---

## What Works in 0.1.0

### Language Core

- integer types: `t8`, `t16`, `t32`, `t64`, `t128`
- `f64`
- `bool`
- `tbool` with wire values:
  - `false = 0`
  - `true = 1`
  - `unknown = 2`
- `char`
- `str`
- structs
- arrays
- free functions
- struct methods
- generic functions over types
- implicit last-expression return
- explicit `return`
- `if` / `else`
- `while`
- `for`
- infinite `loop`
- `break` / `continue`
- `when` matching for literal, range, bool, catchall, and TBool wire-value patterns
- declared-width wrapping arithmetic
- comparisons
- logical short-circuiting
- compound assignment
- dot access
- built-ins:
  - `print`
  - `println`
  - `printn`
  - `assert`
  - `assert_eq`
  - `read`
  - `input`

### Memory Model

Cx 0.1.0 includes the foundation for explicit value movement:

- stack-allocated structs
- stack-allocated arrays
- explicit copy semantics
- `.copy`
- `.copy.free`
- `copy_into`
- no garbage collector
- no borrow checker

Longer-term memory features such as `Handle<T>`, string arenas, and richer ownership tools are planned post-0.1.

---

## Interpreter

The tree-walking interpreter is the reference implementation for Cx semantics.

It supports the 0.1 language surface and is used as the comparison target for JIT parity testing.

---

## Cranelift JIT Backend

The Cranelift backend compiles supported Cx IR to native machine code.

Current JIT-supported areas include:

- integer arithmetic
- declared-width wrapping behavior
- comparisons
- logical AND/OR short-circuiting
- typed and inferred variable declarations
- `if` / `else`
- `while`
- `for` ranges
- infinite `loop`
- `break` / `continue`
- direct function calls
- method dispatch through mangled-name lowering
- `when` blocks for literal/range/bool/catchall/TBool wire-value patterns
- struct literals
- struct field reads and writes
- fixed-size arrays
- array element reads and writes
- unary negation
- boolean NOT
- integer and float casts
- `f64` arithmetic and comparison
- runtime intrinsics for print/assert behavior
- void returns and exit-code propagation

The JIT is still under active expansion, but all currently supported JIT fixtures match interpreter behavior.

---

## JIT Parity Baseline

| Category       | PASS | SKIP | PARITY_FAIL |
|----------------|------|------|-------------|
| Arithmetic     | 14   | 4    | 0           |
| VariableDecl   | 5    | 5    | 0           |
| IfElse         | 6    | 0    | 0           |
| WhileLoop      | 6    | 2    | 0           |
| ForLoop        | 4    | 0    | 0           |
| InfiniteLoop   | 5    | 0    | 0           |
| DirectCall     | 12   | 5    | 0           |
| Struct         | 13   | 1    | 0           |
| Array          | 3    | 2    | 0           |
| CompoundAssign | 7    | 0    | 0           |
| Unary          | 3    | 0    | 0           |
| Cast           | 4    | 0    | 0           |
| FloatOps       | 6    | 1    | 0           |
| BuiltinAssert  | 4    | 2    | 0           |
| LogicalOps     | 2    | 0    | 0           |
| Other          | 26   | 40   | 0           |
| **Total**      | **120** | **62** | **0** |

---

## Deferred Post-0.1

The following are intentionally not part of the 0.1 completion claim:

- `f64` and `t128` print formatting runtime ABI extension
- `?` expression literal lowering
- full TBool unknown propagation through arithmetic, comparison, and logical operations
- enum IR lowering
- `EnumVariant` arms in `when`
- Cranelift AOT compilation
- LLVM AOT backend
- `WhileIn` source-to-IR lowering
- `Result<T>` / `?` operator
- string interpolation
- string arena
- `Handle<T>`
- generic standard library

---

## Getting Started

### Requirements

- Rust stable toolchain
- Cranelift JIT support requires the `jit` feature

### Build

```bash
cargo build --features jit
```

### Run with the interpreter

```bash
cargo run -- examples/hello.cx
```

### Run with the Cranelift JIT

```bash
cargo run --features jit -- --backend=cranelift examples/fibonacci.cx
```

### Run the full test suite

```bash
cargo test --features jit
```

### Run the JIT parity gate

```bash
cargo test --features jit jit_parity_by_feature -- --nocapture
```

---

## Examples

Working examples are in `examples/`.

Run all examples:

```bash
bash examples/run_all.sh
```

Some examples demonstrate interpreter-only or post-0.1 features. The JIT backend should be tested through the parity harness rather than assuming every example is JIT-ready.

---

## Data Layout and ABI

The Cx 0.1 ABI is locked for x86-64.

Highlights:

- scalar sizes and alignments are defined
- `tbool` wire representation is fixed:
  - `false = 0`
  - `true = 1`
  - `unknown = 2`
- structs use C-compatible alignment with natural padding
- arrays use contiguous stack allocation through `ArrayAlloca`
- expression evaluation order is left-to-right
- current calling convention target: C ABI / SystemV on Linux x86-64

See:

```text
docs/backend/cx_abi_v0.1.md
```

---

## Project Status

Cx is early-stage and experimental.

Use it for:

- compiler development
- language design validation
- backend/JIT experimentation
- systems-language research
- test-driven expansion of the Cx feature set

Do not use it for production applications yet.

---

## Roadmap

Near-term post-0.1 priorities:

- stabilize 0.1.1
- improve examples and documentation
- expand JIT PASS coverage
- add missing runtime ABI extensions
- continue enum, string, result, and standard-library work

Longer-term goals:

- Cranelift AOT compilation
- LLVM AOT backend
- C FFI surface
- minimal standard library
- language server tooling
- `cx build`, `cx test`, and `cx check`
- game-engine-oriented runtime and memory tooling

---

## Built With

- [Rust](https://www.rust-lang.org/) — compiler, interpreter, tooling
- [Logos](https://github.com/maciejhirsz/logos) — lexer
- [Chumsky](https://github.com/zesterer/chumsky) — parser
- [Cranelift](https://cranelift.dev/) — JIT backend

---

## Contributing / Contact

Open an issue or PR to discuss language behavior, runtime semantics, backend work, or documentation.

Useful files:

- `src/tests/verification_matrix/` — feature verification fixtures
- `docs/backend/cx_jit_parity_checklist.md` — JIT parity status
- `docs/backend/cx_abi_v0.1.md` — ABI specification
- `CONTRIBUTING.md` — branch policy and merge workflow
