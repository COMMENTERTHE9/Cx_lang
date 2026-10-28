# Native numeric module state

## Behavior

Functions can read and write declared numeric scalar bindings at module scope:
`t8`, `t16`, `t32`, `t64`, `t128` and `f64`, including immutable numeric
constants. The interpreter remains the reference. Source syntax and function
calling conventions are unchanged; no hidden module-context parameter is added.

A binding captured by a lowered function receives module storage. Other top-level
values keep their existing SSA path, preserving established local and aggregate
copy behavior. Every load/store uses the resolved BindingId, including imported
modules, and a native call observes earlier calls' writes.

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
The interpreter's separate binding-owner correction is a prerequisite for these
new paths, rather than redefining reference behavior around a lost write-back.

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

Same-named top-level variables from different modules remain refused. Their IDs
are distinct, but the interpreter still shares a by-name declaration index and
rejects the second name. Executing them only in native code would create a new
backend disagreement, so namespace correction must precede enabling that case.

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
IR/native unit tests cover shared state, fresh executions, invalid layouts and
references, ABI preservation, stable printing and user-symbol collision safety.
Use both builds and test feature sets, the parity harness, and Linux CI for the
complete gate results; the restart report records dated totals separately.
