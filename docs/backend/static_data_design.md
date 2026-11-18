# Static data in the IR — design for M1.1

**Status: approved by the owner on 2026-11-18, including the JIT-path check of
§3.5, and IMPLEMENTED on `backend/m1.1-static-data` (four commits). Not
reviewed independently yet, and not integrated. §11 says what was built, what
was measured, and where the result differs from this design — one of its own
performance criteria is not met.**

- Date: 2026-11-18
- Base as designed: `submain` @ `d4b2c06` plus the M0 fixes. Base as built: the
  PR #476 revision (`backend/result-content-equality`), which changed string
  equality and so two premises of §4 — see §11.
- Scope: one packet. The other M1 parts (runtime library, validator in the JIT
  path, symbol naming, target description) stay separate and are listed in §9.

---

## 1. The problem

Compiled programs contain addresses from inside the compiler process.

`construct_static_str` (`src/ir/lower.rs`) leaks a string's bytes and a
`{byte_ptr, byte_len}` descriptor with `Vec::leak`, then emits the descriptor's
address as a number:

```
v10 = const i64 139947281560512
v11 = cast i64 v10 -> ptr
call cx_print_str(v11)
```

Every static string in a compiled program takes this path. `construct_static_str`
has four callers — string literals, the constant-folded `+`, interpolation
chunks, and the inline-print helper that emits enum names, `Ok(` / `Err(` / `)`
and the `?` of an unknown bool — and nothing else in lowering manufactures an
address.

Consequences, each observed on this tree:

| Consequence | Evidence |
|---|---|
| The IR cannot be written to an object file: the number means nothing outside the process that produced it | Blocks Cranelift AOT and LLVM equally |
| The IR is not reproducible | Two `--backend=validate` dumps of one program differ in that constant |
| The 0.1 determinism gate ("same IR always prints the same string") does not hold for any program with a string | Same |
| Each lowering leaks the bytes and the descriptor for the life of the process | `Vec::leak`, twice per string |

Nothing else in the IR has this property. Module globals already use relocatable
storage (`IrGlobal` + `GlobalAddr`, Cranelift data objects); host functions are
reached by name.

## 2. Goals and non-goals

**Goals**

1. No compiler-process address appears in any `IrModule`.
2. Printing the IR of one program twice gives byte-identical text.
3. The JIT runs every program exactly as it does today: same output, same exit
   codes, same refusals.
4. The representation is what an object-file backend needs, so M2 and M3 add an
   emitter and no further IR change for strings.

**Non-goals** — each is deliberately left for its own packet:

- Emitting object files or executables (M2, M3).
- Moving the runtime functions out of the compiler binary (M1.3).
- Dynamic strings, runtime concatenation, or any heap string (E1 / E4).
- Changing what string or `Result` equality means (`docs/known_issues.md` §55).
- Initial values for module globals. `IrGlobal` stays zero-initialised with its
  lifecycle byte.
- Merging identical descriptors. See §4.3 — it would change behaviour.

## 3. Design

### 3.1 IR additions (`src/ir/types.rs`, `src/ir/instr.rs`)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DataId(pub u32);

/// Read-only module data with fixed contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrData {
    pub id: DataId,
    pub align: usize,
    /// Initial contents. A slot named by a relocation holds zero here.
    pub bytes: Vec<u8>,
    /// Pointer-sized slots that hold the address of another data object.
    pub relocs: Vec<IrDataReloc>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrDataReloc {
    pub offset: usize,   // byte offset of the slot inside `bytes`
    pub target: DataId,  // object whose address is stored there
}

pub struct IrModule {
    pub debug_name: String,
    pub globals: Vec<IrGlobal>,
    pub data: Vec<IrData>,        // new
    pub functions: Vec<IrFunction>,
}

// IrInst
/// Address of read-only module data.
DataAddr { dst: ValueId, data: DataId },
```

`IrData` is separate from `IrGlobal` on purpose. A global is writable,
zero-initialised and carries a lifecycle byte that every access checks; data is
read-only, has contents, and is never checked. Folding the two together would
put a flag on every global access path for no gain.

A relocation with no addend is enough for descriptors. An addend can be added
later without changing existing data.

### 3.2 A string literal

`"hello"` becomes two objects and one instruction:

```
data @d0 align 1 bytes "hello"
data @d1 align 8 { addr @d0, i64 5 }
...
v11 = data_addr @d1
call cx_print_str(v11)
```

`@d1` is the existing two-word descriptor, byte for byte: pointer, then length.
The descriptor ABI does not change, so `cx_print_str`, `cx_str_eq` and the packed
`Result` word are untouched.

### 3.3 Lowering (`src/ir/lower.rs`)

One function changes. `construct_static_str` stops leaking and instead:

1. interns the bytes in a per-module table keyed by content (one bytes object
   per distinct string);
2. appends a **new** descriptor object for this call (see §4.3);
3. emits `DataAddr` for the descriptor.

`LoweringCtx` is per function, so the table is module-level state owned by
`lower_program_inner` and passed by `&mut` into `lower_semantic_function` and
`lower_top_level_main`, next to `module_bindings`. IDs are assigned in order of
first use during lowering, which is already deterministic: functions lower in
program order.

The interpolation refusal at the top of `construct_static_str` stays as it is.

### 3.4 Printer (`src/ir/printer.rs`)

```
data @d0 align 1 bytes "he said \"hi\"\n"
data @d1 align 8 { addr @d0, i64 14 }
```

- A bytes-only object prints as an escaped string when it is valid UTF-8, and as
  `hex 68 65 6c …` otherwise.
- An object with relocations prints as a list of words: `addr @dN` for a
  relocated slot, `i64 N` for the others.
- `v = data_addr @dN` for the instruction.

Data prints after globals and before functions.

### 3.5 Validator (`src/ir/validate.rs`)

New checks:

| Check | Rejects |
|---|---|
| Data IDs are unique | Two objects with one ID |
| Alignment is a non-zero power of two, at most `MAX_GLOBAL_ALIGNMENT` | Bad alignment |
| Each relocation's `offset + 8 <= bytes.len()`, is 8-aligned, and its slot bytes are zero | Out-of-range or overlapping slots |
| Each relocation target exists and is not the object itself | Dangling or self reference |
| `DataAddr` names an existing object and defines a `Ptr` | Dangling reference |
| **No address constants:** a `Cast` from `I64` to `Ptr` whose operand is defined by a `ConstInt` | The pattern this packet removes |

The last rule is narrow on purpose. Lowering legitimately casts a *run-time*
word to a pointer in three places — each unpacking a `Result` payload — and
those operands come from a load, never from a constant.

The standalone validator still does not know `cx_str_eq` or `cx_handle_*`, so it
rejects programs the JIT runs. Fixing that is M1.2. To make the guarantee real
before then, the address-constant rule is also exposed as its own function and
called on the JIT path before code generation, returning the existing
"unsupported" exit (127) with a message naming the function. It is one linear
pass over the instructions.

### 3.6 Cranelift JIT (`src/backend/cranelift/host_boundary.rs`)

Mirrors what the backend already does for `IrGlobal`:

1. For each `IrData`: `declare_anonymous_data(writable = false, tls = false)`.
2. `DataDescription::define(bytes)`, `set_align(align)`.
3. For each relocation: `declare_data_in_data(target, &mut desc)` then
   `write_data_addr(offset, that_reference, 0)`.
4. `define_data`.
5. `DataAddr` lowers like `GlobalAddr`: `declare_data_in_func` +
   `global_value`.

All objects are declared first and defined second, so a descriptor can refer to
a bytes object regardless of order. The data belongs to the `JITModule`, which
ends the per-lowering leak.

*(Correction, on implementation: this paragraph first said the data "is
released with" the module. It is not. Cranelift's `JITModule` leaves its code
and data mapped when dropped unless `free_memory` is called, and this backend
does not call it — true before this packet for code and globals, and unchanged
by it. One program runs per process, so nothing accumulates; what the packet
removes is the leak in LOWERING, which did accumulate.)*

## 4. Why behaviour does not change

### 4.1 What compiled code can observe about a static string

1. Its contents and length, through `cx_print_str`, `cx_print_str_inline` and
   `cx_str_eq`. Unchanged: the descriptor has the same layout and the same bytes.
2. The descriptor's address, in exactly one place: `assert_eq` on two `Result`
   values compares the packed 128-bit words, and a string payload's word is its
   descriptor address. (`when` compared addresses too, until M0 fixed it.)

### 4.2 Contents

Byte-identical by construction, and checked by the corpus replay in §6.

### 4.3 Addresses, and why descriptors are not merged

Today each call to `construct_static_str` makes a fresh descriptor, and it is
called once per literal *site* at lowering time. So two evaluations of one site
see the same address, and two sites with equal text see different addresses.
That is observable:

```cx
fnc: Result<str> make() { return Ok("same") }
assert_eq(make(), make())            // JIT: passes (one site)

let a
a = Ok("same")
let b
b = Ok("same")
assert_eq(a, b)                      // JIT: traps (two sites)
```

Whether either result is *right* is the open question in known-issues §55. This
packet must not answer it by accident, so it keeps the rule exactly: **one
descriptor object per site, bytes shared**. Merging descriptors with equal text
would flip the second case from trap to pass.

Sharing the bytes object is safe because nothing compares byte pointers:
`cx_str_eq` compares length and contents.

## 5. Predicted fallout

| File | Change |
|---|---|
| `src/ir/types.rs` | `DataId`, `IrData`, `IrDataReloc`, `IrModule.data` |
| `src/ir/instr.rs` | `IrInst::DataAddr` |
| `src/ir/lower.rs` | `construct_static_str`; the module-level table; `IrModule` construction |
| `src/ir/printer.rs` | data section, `data_addr` |
| `src/ir/validate.rs` | checks in §3.5; `define_value` arm |
| `src/backend/cranelift/host_boundary.rs` | data declaration/definition, `DataAddr` arm, the pre-codegen rule call |
| `src/backend/cranelift/mod.rs` | `DataAddr` arm in the dead `lower_instruction` skeleton (its match is exhaustive) |
| Every hand-built `IrModule { .. }` | add `data: vec![]` — 164 sites, 135 of them in `host_boundary.rs` tests and 21 in `validate.rs`. Mechanical, but the largest part of the diff by line count |
| `src/ir/lower.rs` tests that inspect the instructions of a string literal | expect `DataAddr` instead of `ConstInt` + `Cast` — about 7 mention descriptors |
| `docs/backend/cx_abi_v0.1.md` | the string section says `(*const u8, u32)`; correct it to the implemented `{i64 ptr, i64 len}` and reference this document |
| `docs/known_issues.md` | new entry for the address constants and their removal |

Not touched: the frontend, the interpreter, `call_shapes.rs`, `monomorphize.rs`,
any fixture's expected output.

Test expectations change only where a test asserts the *instructions* a string
lowers to. No test that asserts behaviour changes. That distinction is the line
between updating a test and weakening one.

## 6. Validation and parity requirements

All of these must hold before the packet is reported complete. Numbers are the
M0 baseline.

**Existing gates, unchanged or higher**

| Gate | Baseline | Requirement |
|---|---|---|
| `cargo test` | 320 / 0 | all pass; count rises only by new tests |
| `cargo test --features jit` | 499 / 0 | all pass; count rises only by new tests |
| Parity | 610 PASS / 48 SKIP / 0 PARITY_FAIL (658 fixtures) | PASS ≥ 610, SKIP = 48 with the **same 48 fixtures**, 0 PARITY_FAIL |
| `run_matrix.sh` | 658 / 658 | all pass |
| Clippy | recorded count on both feature sets | no new lint |

**Behaviour preservation**

- Replay every fixture, example and benchmark (694 programs plus the 8 from M0)
  on both backends with the pre-change and post-change binaries. Requirement:
  zero differences in exit code or stdout. Stderr may differ only where it
  contains an IR dump.
- The two programs in §4.3 give the same result before and after, on both
  backends.

**New guarantees, each with a test**

1. *No address constants.* For every fixture that lowers, the address-constant
   rule passes. Enforced on the JIT path and asserted by a corpus test.
2. *Reproducible IR.* For every fixture that lowers, two `--backend=validate`
   dumps are byte-identical. A corpus test spawns the binary twice per fixture
   and compares.
3. *Data validity.* Unit tests for each new validator check: one accepting case
   and one rejecting case per row of the table in §3.5.
4. *Round trip through the JIT.* Unit tests on hand-built IR: a bytes object read
   back through a descriptor; two descriptors sharing one bytes object; a
   descriptor defined before its target; an empty string (zero-length bytes
   object).
5. *Leak removed.* No `leak()` call remains in `construct_static_str`.

**Performance**

- The benchmark programs in `bench/` and a loop that binds and compares a string
  20 million times, before and after, same session. Requirement: no program more
  than 10% slower (the standing within-session rule in `bench/BASELINE.md`), and
  the string loop's peak memory does not grow with iteration count.

**Review**

- Focused independent review of the diff before integration, with the reviewer
  given the code and not the conclusion.

## 7. Work breakdown

Four commits, each passing the gates on its own:

1. IR types, instruction, printer, validator checks, and their unit tests. No
   producer yet, so behaviour cannot change.
2. Cranelift support for `IrData` and `DataAddr`, with the hand-built-IR tests.
   Still no producer.
3. `construct_static_str` switches to data objects; the leak is removed; the
   address-constant rule is enforced on the JIT path; instruction-level test
   expectations are updated. This is the commit that must show zero behaviour
   change.
4. Corpus tests for reproducible IR and no address constants; documentation.

Size: medium — several days of work, most of it tests.

## 8. Interaction with open work

- **PR #476** changes the same function: it splits it into
  `construct_static_descriptor` and `copy_runtime_string`. This design replaces
  only how the raw descriptor is produced, so it composes with either outcome of
  that PR; whichever lands second rebases across one function. This packet does
  not depend on the PR and does not include any of it.
- **M1.3 (runtime library).** The descriptor layout this packet fixes in data is
  the layout the runtime functions already read. Nothing here needs to change
  when they move.
- **M3 (LLVM).** `IrData` maps directly: a bytes object is a private constant
  array, a descriptor is a private constant struct whose first field is the
  bytes object's address. That mapping was checked by hand during the audit.

## 9. What M1 still needs after this packet

Kept as separate tasks, in the order I would take them:

| Packet | Content |
|---|---|
| M1.2 | One intrinsic table shared by lowering, the validator and the backend; the full validator on the JIT path under test; checks for return types, legal casts and load/store types |
| M1.3 | The runtime functions as a linkable library used by both the JIT and AOT; entry shim |
| M1.4 | Recursion-depth guard, trap and `fmod` made backend-neutral |
| M1.5 | Target description for layout; symbol naming that cannot collide with the runtime or libc |

## 10. Decisions asked of the owner

1. Approve implementing this packet as designed.
2. Confirm the minimal JIT-path check in §3.5 may land here, ahead of the full
   validator work in M1.2. The alternative is to do M1.2 first.

## 11. As built (2026-11-18)

Branch `backend/m1.1-static-data`, four commits, in the order of §7:

| Commit | Content |
|---|---|
| 1 | `IrData`, `IrDataReloc`, `IrInst::DataAddr`, printer, validator checks, `find_address_constants`; 22 unit tests |
| 2 | Cranelift JIT support; the data rules as `validate::data_errors`, run by the JIT before anything is placed; 8 tests on hand-built IR |
| 3 | `construct_static_str` produces data objects; the address-constant rule is on in the validator and on the JIT path; 6 lowering tests and a leak measurement |
| 4 | The corpus test; this section; `docs/known_issues.md` §71 |

### What changed from the design

- **§4.1 and §4.3 are out of date, harmlessly.** They say a descriptor's address
  is observable through `assert_eq` on two `Result`s, and keep one descriptor
  per literal site so as not to change that. The owner has since decided
  equality is by content (`docs/backend/value_equality.md`), and nothing can
  observe a descriptor's address any more. The rule was kept anyway — one
  descriptor per site, bytes shared — because it was the approved design and
  merging descriptors is now a pure optimisation that can be made on its own.
- **§3.5.** The data rules had to be callable without the rest of the
  validator, because the JIT must not place data it has not checked: a
  relocation outside its object is a write outside an allocation. They are
  `validate::data_errors`, used by both.
- **§3.6**, corrected in place above.
- **§5 predicted test fallout that did not happen.** No existing test asserted
  the address-constant instruction pair, so none changed. The 166 hand-built
  `IrModule` literals gained `data: vec![]` as predicted.

### Requirements of §6

| Requirement | Result |
|---|---|
| `cargo test` / `--features jit` pass, count rises only by new tests | 359 / 546 at commit 3 (1 ignored each); see the commit messages for each step |
| Parity: same PASS and SKIP sets, 0 PARITY_FAIL | 631 PASS / 55 SKIP / 0 at commits 1–3, identical to the base |
| Replay, zero differences in exit code or stdout | 730 programs (686 fixtures, examples, benchmarks), both backends, before and after commit 3: none, stderr included |
| No address constants, enforced and tested | JIT path refuses (exit 127); `ir_is_reproducible_and_holds_no_compiler_address_across_the_corpus` |
| Reproducible IR | Same test: 469 fixtures lower, 71 with string data, each identical across two processes. Before the switch a one-string program's two dumps differed |
| Data validity, one accepting and one rejecting test per rule | `validate::data_tests` |
| Round trip through the JIT: relocations, order, identity, empty string, alignment, repeated compilation, invalid references | `module_data_tests`; fifteen kinds of damage each refused without a panic |
| Leak removed | No `leak()` in lowering. 4000 lowerings of a 60-literal program: 39,060 KB grown before, 232 KB after |
| String loop's peak memory does not grow with iteration count | 15 MB at 20 million iterations, before and after |
| Benchmarks no more than 10% slower | The eight `bench/` programs: unchanged |
| **A loop that binds and compares a string, no more than 10% slower** | **Not met in every variant — see below** |
| Independent review before integration | **Not done yet** |

### The performance result

Optimized build, Cranelift JIT, a loop whose body binds a string and compares
it, one billion iterations, six variants that differ only in what precedes the
loop (which moves the loop's machine code by a few bytes):

| Variant | Before | After |
|---|---|---|
| as is | 3.18 ns | 3.43 ns |
| one addition before | 2.77 | 3.17 |
| two additions before | 2.68 | 3.04 |
| three additions before | 2.99 | 3.17 |
| a multiplication before | 2.92 | 3.38 |
| an array before | 3.63 | 3.33 |

Slower in five (+6% to +16%), faster in one (−8%), +7.5% taken together; a
second run of the same six gave +10.9%. That is about one CPU cycle per
comparison on a loop of about ten, on top of position effects of ±15% that the
variants show by themselves.

What it is not, each measured:

- the instruction that materialises the address — emitting it as an absolute
  immediate, the form the constants used, gives the same times;
- memory protection — placing the data in writable memory gives the same times;
- the stack — thirteen frame sizes give the same times;
- the debug-build figure — a debug build cannot show an effect this small: the
  same code measures 19.8–20.8 ns per iteration in two builds and 23.7–24.3 in a
  third that differs only in unrelated compiler code, and builds that still use
  address constants range from 19.2 to 23.3.

What is left is where the data is: beside the code in the JIT's memory rather
than on the heap. The cause is not identified.

**This needs the owner's decision, not more tuning.** The criterion was written
before the measurement and one kind of program misses it by a few percent. The
alternative to data objects is the address constants this packet exists to
remove; an object file cannot hold them. An optimizing backend (M3) places data
itself, so the figure is a property of the unoptimized JIT.

