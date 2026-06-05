# Cx

Cx is a compiled, GC-free systems programming language for game engines, tools, and systems programmers.

The goal of Cx is to give engine-facing code predictable memory behavior, explicit value movement, deterministic teardown, and low-level control without forcing every feature to become allocator plumbing.

**Status: 0.2.0 — released 2026-06-05.** Last tagged release: **v0.2.0**.

This README describes the current `submain` branch. The reference interpreter is the source of truth for language semantics; every code sample below was compiled and run against it.

Cx is not production-ready. It is a working compiler foundation under active development.

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

## The Pipeline

Source code moves through a full compiler pipeline:

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

The **interpreter** is the reference semantics backend — it defines what Cx programs mean.

The **Cranelift JIT** is the native execution backend.

A differential harness checks that every JIT-supported feature produces the same observable behavior as the interpreter.

You can inspect the IR for any program with the `validate` backend, which lowers to IR, checks it, and prints it:

```bash
cargo run -- --backend=validate examples/fibonacci.cx
```

---

## Current Verification Status

As of `v0.2.0`:

- **243 unit tests passing** (`cargo test`)
- **418 unit tests passing** with the JIT enabled (`cargo test --features jit`)
- **230 verification fixtures**
- **JIT parity: 140 PASS / 90 SKIP / 0 PARITY_FAIL** across all 230 fixtures
- **zero Clippy errors**

A fixture is **SKIP** when it exercises a language feature the JIT does not lower to native code yet (the interpreter still runs it). **PARITY_FAIL** means the interpreter and JIT disagree on observable behavior — that number must stay zero.

---

## Code Taste

```cx
fnc: t64 sum_range(n: t64) {
    total: t64 = 0

    for i in 0..n {
        total += i
    }

    total
}

print(sum_range(5))            // 10
```

```cx
fnc: t64 tier(n: t64) {
    when n {
        0       => 0,
        1..=9   => 1,
        10..=99 => 2,
        _       => 3,
    }
}

print(tier(7))                 // 1
```

```cx
fnc: bool is_large(n: t64) {
    n > 100
}

print(is_large(200))           // true
```

A note on the samples above:

- Declarations with a value are written `name: T = value`. Bare `let x;` declares an **uninitialized** binding (assigned later); it does not take an initializer.
- Both `if` and `when` are **expressions**: the chosen branch (or arm) is the value, so either can be returned or assigned directly. Use `when` for multi-way matching and `if` for a two-way choice — `is_large` above uses a bare trailing expression, the tersest form. `if` and `when` also work as plain statements when their value isn't used.

---

## Language by Example

Each snippet here is a complete program; the inline comments show the exact output. Several are backed by files in `examples/`.

### Integers are signed, with declared-width wrapping

Integer types are `t8`, `t16`, `t32`, `t64`, `t128` — all **signed**. Arithmetic wraps at the declared width, and an out-of-range literal is rejected at compile time (both signs).

```cx
x: t8 = 127
x += 1
print(x)                       // -128   (wraps at the t8 width)
```

```cx
x: t8 = 200                    // compile error:
// integer literal 200 out of range for t8 (valid range: -128..127)
```

### Enums (and they work in function signatures)

Enums declare named variants, referenced with `::`. Since 0.2, an enum type can be used in parameter and return positions. See `examples/enums.cx`.

```cx
enum Light { Red, Green, Yellow }

fnc: Light next(cur: Light) {
    when cur {
        Light::Red    => Light::Green,
        Light::Green  => Light::Yellow,
        Light::Yellow => Light::Red,
        _             => Light::Red,
    }
}

c: Light = next(Light::Red)
when c { Light::Green => print(1), _ => print(0) }   // 1
```

### `when` matching

`when` matches integer literals, ranges with positive bounds, enum variants, and a `_` catch-all. A `when` must be exhaustive: if it does not cover every case it needs a `_` arm. See `examples/when_match.cx`.

```cx
size: str = when 7 {
    0     => "none",
    1..=9 => "small",
    _     => "big",
}
print(size)                    // small
```

### `if` as an expression

`if` produces a value, just like `when` — use it for a two-way choice as an implicit return, on the right of an assignment, or nested in a larger expression.

```cx
fnc: t64 grade(n: t64) {
    if n >= 90 { 4 } else { 3 }
}
print(grade(95))               // 4
```

```cx
limit: t64 = 100
x: t64 = if limit > 50 { 10 } else { 20 }
print(x)                       // 10
```

Two rules for the value form: it must have an `else` (a consumed `if` always produces a value), and each branch is a single expression — the same shape as a `when` arm. As a plain statement, where the value is not used, `if` needs no `else`.

### Three-state `bool` and the `?` unknown literal

`bool` is three-state: `true`, `false`, and **unknown**, written with the literal `?`. Because an unknown value cannot choose a branch, using it as an `if` condition is an error that directs you to `when`. See `examples/tbool_uncertainty.cx`.

```cx
let x;
x = ?
when x {
    true    => print("yes"),
    false   => print("no"),
    unknown => print("maybe"),
}
// maybe
```

```cx
b: bool = ?
if b { print(1) } else { print(2) }   // runtime error:
// `if` condition is unknown; an unknown TBool can't choose a branch —
// use `when` to handle true, false, and unknown explicitly
```

`unknown` is a `when`-pattern keyword, not a value. Writing it in value position is an error that points you at `?`:

```cx
b: bool = unknown              // parse error:
// `unknown` is a pattern keyword, not a value — use `?` for the unknown literal
```

### String interpolation

A `str` literal interpolates **bare variables** in `{...}`:

```cx
name: str = "Cx"
n: t32 = 3
print("{name} v0.{n}")         // Cx v0.3
```

Interpolation supports bare variable names only. A non-variable form (e.g. a call) is a compile-checked error rather than silent literal output:

```cx
print("{f(2)}")                // error: string interpolation supports bare
                               // variables only; compute `{f(2)}` into a
                               // variable first
```

### Strings: concatenation and length

Concatenate strings with `+`. Get a string's **byte** length — or an array's element count — with `len`.

```cx
greeting: str = "Hello, " + "Cx"
print(greeting)                // Hello, Cx
print(len(greeting))           // 9
```

`len` is a byte count, not a character count: `len("é")` is `2` (one character, two UTF-8 bytes). Mixing types in a concatenation is a compile error, not an implicit conversion — use interpolation instead:

```cx
n: t32 = 3
s: str = "v" + n               // error: cannot concatenate `str` and `t32` with
                               // `+` — use string interpolation, e.g. "v{n}"
```

### Results and the `?` operator

Fallible functions return `Result<T>` with `Ok` / `Err`, and the postfix `?` operator propagates errors. See `examples/error_handling.cx`.

```cx
fnc: Result<t32> safe_divide(a: t32, b: t32) {
    if b == 0 {
        return Err("division by zero")
    }
    return Ok(a / b)
}

fnc: Result<t32> compute(a: t32, b: t32) {
    let q;
    q = safe_divide(a, b)?
    return Ok(q * 2)
}

print(compute(10, 2))          // Ok(10)
print(compute(10, 0))          // Err(division by zero)
```

---

## What Works

### Language Core (interpreter — reference semantics)

- signed integer types: `t8`, `t16`, `t32`, `t64`, `t128`, with declared-width wrapping and compile-time range checking
- `f64`
- three-state `bool` (`true` / `false` / unknown via `?`)
- `char`, `str` (with `{var}` interpolation of bare variables, and `+` concatenation)
- enums — variants, `::` access, and use in function signatures
- structs and `impl` methods
- fixed-size arrays (`arr:[i]` indexing)
- generic structs and functions over types
- free functions; implicit last-expression return; explicit `return`
- `if` / `else`, `while`, `for`, infinite `loop`, `break` / `continue`
- `when` matching (literals, positive ranges, enum variants, bool/TBool, catch-all) — usable as an expression
- `Result<T>` with `Ok` / `Err` and the `?` propagation operator
- comparisons, logical short-circuiting, compound assignment, dot access
- built-ins: `print`, `println`, `printn`, `assert`, `assert_eq`, `read`, `input`, `exit`, `len`

### Memory Model

Cx includes the foundation for explicit value movement:

- stack-allocated structs
- stack-allocated arrays
- explicit copy semantics: `.copy`, `.copy.free`, `copy_into`
- no garbage collector
- no borrow checker

Longer-term memory features such as a richer `Handle<T>` surface, string arenas, and broader ownership tools are planned.

---

## Backend Coverage

The tree-walking **interpreter** implements the full language surface above and is the comparison target for JIT parity.

The **Cranelift JIT** compiles a growing subset of Cx IR to native machine code. Currently JIT-lowered areas include:

- integer arithmetic and declared-width wrapping
- comparisons and logical AND/OR short-circuiting
- typed and inferred variable declarations
- `if` / `else`, `while`, `for` ranges, infinite `loop`, `break` / `continue`
- direct function calls and method dispatch
- `when` blocks for literal / range / bool / catch-all / TBool patterns
- struct literals, field reads/writes; fixed-size arrays and element access
- unary negation, boolean NOT, integer and float casts
- `f64` arithmetic and comparison
- runtime intrinsics for print/assert; void returns and exit-code propagation

All currently JIT-lowered fixtures match interpreter behavior (0 PARITY_FAIL). A fixture whose feature is not yet lowered is reported as SKIP, not as a failure.

### JIT Parity Baseline

As of `v0.2.0`:

| Status | Count |
|--------|-------|
| PASS | 140 |
| SKIP | 90 |
| PARITY_FAIL | 0 |
| **Total fixtures** | **230** |

(Authoritative totals from the parity harness. Run `cargo test --features jit jit_parity_by_feature -- --nocapture` for the live per-category breakdown.)

---

## Not Yet Lowered / Future Work

These features **work in the interpreter** but are **not yet lowered to the JIT** (they show up as parity SKIP):

- `if`-expression lowering (the `if`/`else` *statement* form is lowered; the *expression* form is interpreter-only for now)
- enum IR lowering and `EnumVariant` arms in `when`
- `Result<T>` / `?` operator lowering
- string interpolation lowering
- `f64` / `t128` native print formatting (ABI extension)
- `WhileIn` source-to-IR lowering
- full TBool unknown propagation through arithmetic, comparison, and logical ops

These are **not implemented in any backend yet**:

- Cranelift AOT (object-file) compilation
- LLVM AOT backend
- richer `Handle<T>` ownership surface and string arena
- generic standard library

---

## Getting Started

### Requirements

- Rust toolchain (stable works; the project also builds on nightly)
- The Cranelift JIT requires the `jit` feature

### Build

```bash
cargo build --features jit
```

### Run a program with the interpreter

```bash
cargo run -- examples/hello.cx
```

### Run with the Cranelift JIT

```bash
cargo run --features jit -- --backend=cranelift examples/fibonacci.cx
```

### Inspect the IR

```bash
cargo run -- --backend=validate examples/fibonacci.cx
```

### Run the test suite

```bash
cargo test --features jit
```

### Run the JIT parity gate

```bash
cargo test --features jit jit_parity_by_feature -- --nocapture
```

---

## Examples

Runnable examples live in `examples/`:

| File | Shows |
|------|-------|
| `hello.cx` | printing and string interpolation |
| `enums.cx` | enums, `::` variants, enum types in signatures |
| `when_match.cx` | `when` literals/ranges/catch-all, `when` as an expression |
| `tbool_uncertainty.cx` | three-state `bool`, the `?` literal, `when` over TBool |
| `structs_and_methods.cx` | structs and `impl` methods |
| `arrays_and_loops.cx` | fixed-size arrays, indexing, loops |
| `generics.cx` | generic structs over types |
| `error_handling.cx` | `Result<T>`, `Ok` / `Err`, the `?` operator |
| `fibonacci.cx`, `fizzbuzz.cx` | classic small programs |

Run them all:

```bash
bash examples/run_all.sh
```

Some examples exercise interpreter features the JIT does not lower yet; use the parity harness rather than assuming every example is JIT-ready.

---

## Data Layout and ABI

The Cx ABI target is x86-64.

Highlights:

- scalar sizes and alignments are defined
- `bool` wire representation is fixed: `false = 0`, `true = 1`, `unknown = 2`
- structs use C-compatible alignment with natural padding
- arrays use contiguous stack allocation through `ArrayAlloca`
- expression evaluation order is left-to-right
- current calling-convention target: C ABI / SystemV on Linux x86-64

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

Near-term priorities:

- freeze and stabilize 0.2
- expand JIT lowering coverage toward the interpreter surface (enums, `Result`/`?`, string interpolation, `f64`/`t128` printing)
- broaden examples and documentation
- continue ownership and memory tooling

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
