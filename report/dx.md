# Pillar 4 — Developer Experience: Where the Language Fights the User

**Audit base:** `submain` @ `22015c420ad40483d0531d1109fa9837d44deca3`.
**Date:** 2026-05-24.
**Method:** wrote real, non-trivial programs (a 2D vector library, a struct-returning functional variant, an array-backed stack, an enum traffic-light state machine, a 3×3 grid with manual 2D indexing — all in `report/pillar4_programs/`, all runnable) and logged every point where expressing something normal was awkward, surprising, or impossible. This is the "found it while writing code" mode, not the systematic probing of Pillar 2. Findings that are also defects are cross-referenced to Pillars 1/2.

---

## Friction, by impact

### High — blocks or forces awkward versions of normal patterns

**DX1 — `let x = expr` is a parse error; type-inferred bindings take two statements.**
`let` is declaration-only. To bind with inference you must write:
```cx
let c;
c = vadd(a, b)
```
`let c = vadd(a, b)` fails to parse. The only one-line form requires a type annotation (`c: Vec2 = vadd(a, b)`), so **inference and single-line initialization are mutually exclusive**. This appears in essentially every program written for this pillar — the single most pervasive friction. *Cheap fix:* allow an initializer on `let`. (The syntax doc shows the two-step pattern but never says the combined form is forbidden — DX13.)

**DX2 — enums cannot be function parameter or return types.**
A traffic-light `next(s: Light) -> Light` is impossible:
```
return type mismatch: expected Light, got enum Light
argument 1 to 'next': expected Light, got enum Light
```
The annotation `Light` does not unify with the variant's inferred type `enum Light`, and `enum Light` will not parse as an annotation (`expected ... found KeywordEnum`). Enums work fine as **locals** and **struct fields** (`Car { light: Light::Red }` runs), but any function operating on an enum is blocked, so the transition logic in `state_machine.cx` had to be inlined. No verification-matrix fixture covers an enum-typed parameter/return, so this slipped through. *Also a correctness bug — cross-ref Pillar 2's "what we didn't know to test".* High impact: state machines, dispatch, and most enum-centric code want this.

**DX3 — no string concatenation and no string introspection.**
`a + b` on two `str` → `arithmetic requires numeric operands, got str and str`. `len(a)` → `call to undefined function 'len'`. The *only* way to build a string is interpolation (`"{a}{b}"`), and there is no way to ask a string its length or inspect it. Any real text-processing task (tokenizing, formatting tables, building output) fights this immediately.

### Medium — verbosity and noise

**DX4 — array declaration is triply redundant and demands a full literal.**
A capacity-8 stack:
```cx
data: [8: t64] = [0, 0, 0, 0, 0, 0, 0, 0]
```
The size appears in the type (`8`), must be matched element-for-element in the literal, and there is no zero-init or fill shorthand (`[0; 8]`-style). For the 256-element benchmark array in Pillar 1 this meant generating a 256-element literal. Hits every array and every hand-rolled data structure. *Cross-ref Pillar 1 (logged while authoring benchmarks).*

**DX5 — impl blocks require a throwaway name.**
```cx
vec2_methods: impl (v: Vec2) { ... }
```
`vec2_methods` is mandatory but never referenced anywhere. Pure ceremony on every method block.

**DX6 — no dynamic collections.**
There is no growable list/vector/map. Every collection is a fixed array plus manual index bookkeeping (the `top` counter in `stack.cx`). Defensible for a systems/engine language, but it shapes every program and pairs badly with DX4.

**DX7 — f64 whole numbers print without a decimal.**
`Vec2 { x: 3.0, y: 4.0 }` after `add` prints `4` and `6`, not `4.0`/`6.0`. Output gives no signal whether a value is an integer or a float, which is confusing when debugging numeric code.

### Error messages, from the user's seat

**DX8 — parse errors leak raw internal token names and Rust debug formatting (worst single DX issue).**
A trivial mistake like `let c = f()` produces:
```
PARSE ERROR (line 2): ExpectedFound { expected: ['PunctColon', 'PunctSemicolon',
'MacroInnerOpen', 'KeywordConst', 'KeywordStruct', ... 'OpMul', 'OpSub', 'OpBang',
'QuestionMark', ... end of input], found: Some(OpAssign) }
```
The user is shown a Rust `Debug` dump of a chumsky error, internal token enum names (`PunctColon`, `KeywordFnc`, `OpBang`), and a 25-item expected-set. Every syntax error in the language looks like this. It is the highest-frequency, lowest-quality message a Cx user encounters. *Cross-ref: Pillar 1 located the parser at `src/frontend/parser.rs`; the mapping happens in `src/main.rs:381-401` where `format!("{:?}", e.reason())` is used verbatim.* *Fix:* map token kinds to human names and render "expected `;` or `:`, found `=`".

**DX9 — misleading semantic errors.** Bare enum variant `Red` → `'Red' is not a known group or super-group name` (should be "unknown identifier; enum variants are written `Light::Red`"). Plus the Pillar-2 D1/D2 offenders (array OOB and missing-field routed through "variable not declared — declare it with 'index 5: TYPE = value'"). See `report/correctness.md` §Error messages.

### Tooling gaps

**DX10 — no `--help` / `--version`.** `cx --help` prints `error: no input file specified` (the `--help` is treated as an unknown flag and ignored). A newcomer's first instinct gets a cryptic error instead of usage.

**DX11 — the in-language test runner is undocumented.** `--test` runs `#[test]` functions with clean `PASS`/`FAIL` reporting (`assert_eq` etc.) — a genuinely nice feature — but it is absent from `docs/cx_flags.md`, as is `--backend=llvm`. Good feature, hidden.

**DX12 — no REPL, formatter, installable binary, or scaffolding.** All docs invoke the compiler as `cargo run --` despite the usage string advertising `cx <file.cx>`; there is no `cx` on `PATH`, no `cx fmt`, no `cx init`. Fine for 0.1, but worth tracking.

### Documentation gaps

**DX13 — "Known Syntax Gaps" (`docs/cx_syntax.md` §24) omits the real gaps.** It lists only `strref` and `Container` thinness — not no-string-concat, no-`len`, enums-not-in-signatures, or `let`-can't-combine-with-init. A newcomer hits all four within an hour and finds nothing in the doc.

**DX14 — the docs defer to source as the source of truth.** §26 literally says "If you want the real current language surface, check `src/frontend/lexer.rs`, `parser.rs`, `verification_matrix`." Honest, but it means the reference isn't authoritative — a new contributor must read the compiler to know what works. (§26 also contains a mojibake character where smart-quotes were corrupted.)

**DX15 — the Strings section documents only what exists, never what's absent**, so readers assume `+`/`len` are available.

---

## Quick wins — cheap, disproportionate impact

Ordered by (impact ÷ effort). All are small relative to the Pillar-3 risks.
1. **Allow `let x = expr`** (DX1). Removes the most pervasive daily friction in the language. Parser-local change.
2. **Human-readable parse errors** (DX8). Map token kinds → friendly names; stop printing `format!("{:?}", …)`. One function in `main.rs`/`diagnostics.rs`; improves *every* syntax error.
3. **Array fill/zero-init shorthand** (DX4), e.g. `[0; 8]` or default-zeroed `[8: t64]`. Removes the worst array boilerplate; also simplifies real code that Pillar 1 had to generate.
4. **`--help` / `--version`** (DX10). A few lines in arg parsing; fixes the first-run experience.
5. **`len()` for `str` and arrays** (DX3, partial). Unblocks basic text/collection code.
6. **Fix the bare-enum-variant error message** (DX9) to point at `Enum::Variant`.

## Documentation sections to write

- **"Limitations / Not Yet Supported"** — rewrite §24 to actually list: no string `+`/`len`, no dynamic collections, enums not usable in function signatures (DX2), `let` cannot combine with initialization. This is the single highest-value doc change: it sets expectations before the user wastes an hour.
- **"Writing and Running Tests"** — document `#[test]` + `--test` (DX11), with the PASS/FAIL output shown.
- **"Reading Error Messages"** — once DX8 is fixed, a short guide; until then, at least a note that parser errors are currently raw.
- Fix the §26 mojibake and either make the docs authoritative or label them clearly as "best-effort, see matrix".

## What works well (worth keeping)

Structs, methods, generics (`fnc: <T> identity`), `when` matching, Result/`?`, struct-returning functions, and string **interpolation** all worked first-try and read cleanly. The `--test` runner and the `--debug-phase`/`--debug-ast`/`--debug-trace` introspection flags are a real asset for anyone working *on* the language. The 2D-vector and grid programs came together quickly once the array/enum friction was navigated — the core expression and control-flow surface is pleasant.
