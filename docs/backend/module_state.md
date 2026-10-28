# Native scalar module state

## Behavior

Functions can read and write these declared scalar bindings at module scope:
`t8`, `t16`, `t32`, `t64`, `t128` and `f64`, plus tag-only enum values and
constants. The interpreter remains the reference. Source syntax and function
calling conventions are unchanged; no hidden module-context parameter is added.

A binding captured by a lowered function receives module storage. Other top-level
values keep their existing SSA path, preserving established local and aggregate
copy behavior. Every load/store uses the resolved BindingId, including imported
modules, and a native call observes earlier calls' writes.

Enums retain their existing one-byte tag representation. A captured enum uses
two bytes: the value tag followed by the separate lifecycle byte. Enum type and
variant names still come from semantic metadata for printing, matching and
function signatures. Values with tags 128 and 255 retain their bit patterns.
Existing refusals for duplicate enum type names across modules and declarations
with more than 256 variants remain in force.

Typed boundaries also retain semantic enum identity. Generic checks are
permissive before specialization, so an I8 number or a different enum can
otherwise enter a binding, parameter, return value, field or element under an
incorrect enum type. Lowering checks these boundaries before representation
erasure and refuses incompatible specializations before code generation.
Same-enum generic calls/returns and copy arguments remain supported. Source
regressions and an IR unit pin each refusal until shared analysis can diagnose
these invalid specializations earlier.

Each value has a private lifecycle byte:

| State | Meaning | Allowed access |
|---|---|---|
| 0 | Declaration/initializer has not executed | Neither read nor ordinary write |
| 1 | Typed declaration executed without a value | Assignment, then read |
| 2 | Initialized | Read and permitted assignment |

Initializers execute once in source order. A function cannot read a zero-filled
placeholder or assign before the binding exists. Guards use the existing native
runtime-trap channel (126); the interpreter renders a diagnostic (1). Rejection
sidecars record both shapes. Const and readonly capabilities are checked during
shared semantic analysis, including `.copy` write-back.

`.copy` still snapshots arguments in order into caller-owned slots. A later
argument may change the module binding without changing the earlier snapshot;
copy-out writes the returned parameter value to the original module binding.
The interpreter's separate binding-owner correction landed through PR #468 and
is included here, preserving the specified write-back to the original binding.

## Representation

`IrModule.globals` declares zero-initialized storage by stable `GlobalId`, name,
size and alignment. `GlobalAddr` returns its address; ordinary loads/stores and
control flow implement values and lifecycle guards. IR printing identifies each
storage object and address use. Validation rejects undefined/duplicate IDs,
empty names, zero/oversized layouts and unsupported alignment.

Cranelift uses anonymous data objects, so internal storage does not reserve legal
user function names. Alignment is bounded by the locked x64 ABI's maximum of
16 bytes; accepting larger alignments can overrun Cranelift 0.115's page allocator.
The backend independently checks malformed input before allocation. Storage is
shared by compiled functions and starts fresh for each execution, rather than
pointing into an initializer or callee's expired stack frame.

This is a storage/identity implementation, not an allocator redesign. Cranelift
0.115 deliberately retains allocated memory on ordinary drop unless its explicit
freeing API is called. Existing host comments claiming drop frees all JIT memory
are inaccurate; reclamation remains follow-up work. AOT also needs actual module
data emission and relocations. Existing static string lowering still embeds
process-specific addresses and is not solved by numeric globals.

## Scope and remaining limits

Bool, strings, handles, aggregates and untyped bindings accessed from functions
remain explicit lowering gaps. Generic numeric conversion and normal aggregate
parameters/returns retain their established behavior. Bare-global-name string
interpolation inside functions is not newly enabled; its name/scope resolution
needs a separate semantic representation.

Top-level variables from different modules may share a spelling. Storage and
ordinary reads/writes use distinct BindingIds; cold runtime lookups use the
module's or function's lexical name environment. A library's `count` cannot
resolve to an entry-file variable or caller parameter named `count`. Function
and method exits restore the caller's environment, including error paths.

Native print interpolation uses the current lexical scope and captured module
bindings. Branch/loop/pattern names do not leak after their scope ends. String
argument templates retain their established binding timing: after evaluating
arguments, preceding parameters can shadow the caller's visible names; the
callee's body then uses its own definition-site environment.

## Verification

The acceptance example is `examples/tooling_metrics.cx`, with its library and
`src/tests/verification_matrix/t_module_tooling_metrics.cx`. It combines persistent
module state, an immutable module constant, independent entry state, arrays,
imports, fallible calls and useful output:

```text
Ok(6)
Ok(16)
Err(negative sample)
8
100
```

Additional fixtures cover declaration order (reads and writes), declared but
uninitialized storage, first assignment from a function, once-only initializers,
all ordinary signed widths and float updates, and argument-time copy snapshots.
Enum regressions cover persistent mutation, constants, imported state, copy-in/
copy-out snapshots, declared-first assignment, upper byte-tag values, lifecycle
failures and rejection of const write-back.
IR/native unit tests cover shared state, fresh executions, invalid layouts and
references, ABI preservation, stable printing and user-symbol collision safety.
Use both builds and test feature sets, the parity harness, and Linux CI for the
complete gate results; the restart report records dated totals separately.
