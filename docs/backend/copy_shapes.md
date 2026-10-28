# Copy argument shapes

Native lowering resolves implicit copy parameter shapes after explicit generic
monomorphization and before building signatures or layouts. Each concrete call
shape gets a private specialization, with a limit of 64 shapes per function.
Declared ordinary parameter types remain authoritative, including polymorphic
`Err` arguments. Unresolved shapes produce a structured lowering refusal.

`.copy` retains the existing caller-slot convention: arguments are snapshotted
left to right, the callee works on its local value, and every return writes the
final value back through the slot. `.copy.free` remains deprecated and passes an
isolated value. Aggregate value arguments are copied when evaluated so that a
later argument's method call cannot mutate an earlier argument. Method receiver
parameters retain their established mutation behavior.

`copy_into` creates a private struct layout from the ordered field names and
their concrete types. Reads and local writes use ordinary inline aggregate
storage; the caller's variables retain their values. This matches the reference
runtime and the original t12/t13 fixtures. Older roadmap text describing bundle
mutation as bleed-back was inaccurate. The implementation does not deprecate or
change the source syntax of `copy_into`.

Resolved fields must also resolve their enclosing expressions. Refinement checks
operator operands and refreshes cast source types; sharing a pointer, enum tag
or packed Result representation never grants semantic compatibility. Fixed copy
storage refuses changes in width, category or aggregate/Result shape. Literal `Err` remains
polymorphic. Unknown bool aggregate storage is explicitly refused until its
three-state representation survives storage and all consumers; it must never
silently become true through a Bool cast.

Nested declarations are lifted from statements and expression branches using
their existing unique semantic names. Capture-free functions use ordinary native
calls. Outer-local captures still require capture conversion and remain a
separate lowering gap. Lexical blocks keep updates to outer bindings, drop their
local SSA bindings on exit, and restore the outer interpolation name environment.

Regression fixtures cover mixed scalar/string/nested aggregate bundles, isolated
mutation, several instantiations of the same bundle function, Handle copy
parameters, sequential numeric type refinement and argument-time snapshots.
IR units pin refusal of invalid refined operators, stale numeric casts,
shape-changing writes and unknown bool storage. Dated gate and replay results
are recorded in the restart report.
