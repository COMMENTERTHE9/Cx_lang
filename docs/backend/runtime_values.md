# Native runtime values

Native strings keep the established two-word `{byte_ptr, byte_len}` descriptor
ABI. Literal evaluation and named string binding each create an execution-owned
descriptor and record an arena `(offset, length)` identity. Function parameters
bind after argument evaluation. Ordinary aggregate and Result value copies keep
the inner string identity; `.copy` writes its final descriptor back directly.
An empty allocation advances the arena by zero, matching the interpreter.

Plain string equality compares bytes. Equality inside a Result follows the
reference `Value` equality, which compares embedded strings by arena identity.
Result arrays and structs compare their stored values recursively rather than
their storage addresses. Internal print fragments do not allocate arena strings;
an interpolated source literal does. Length folding retains operand evaluation.

The packed i128 Result ABI remains unchanged. An aggregate Ok payload points to
an aligned, execution-owned snapshot. Unwrapping it with `?` creates a local
snapshot, preserving isolation from the stored Result and other unwrapped
copies. Early Err returns perform ordinary `.copy` writeback. The execution
scope releases descriptors and aggregate allocations, including on normal
backend error returns, and restores enclosing state for nested executions.

Lowering checks semantic payload kinds before interpreting packed words.
Completed SSA proof propagation includes binding merges, branch values and loop
backedges. Proven Err remains polymorphic; a numeric Result alias preserves its
packed word rather than narrowing the payload. Unclassified values cannot prove
that an integer is a descriptor. These checks supplement layout validation.

This is still the current JIT representation. Compile-time literal bytes and raw
format descriptors retain the older process-lifetime R6 allocation; relocatable
literal data is required for standalone executables. Dynamic stored
interpolation, nested packed Results and wider payload coverage remain work.
