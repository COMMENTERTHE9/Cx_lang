# Cx Examples

Each file demonstrates a specific language feature. Run any example with:

```
cargo run --quiet -- examples/<file>.cx
```

Run all examples at once:

```
bash examples/run_all.sh
```

## Files

| File | Demonstrates | Expected Output |
|------|-------------|----------------|
| `hello.cx` | Variables, types, `print()`, string interpolation `{var}` | Greeting text with interpolated values |
| `fizzbuzz.cx` | `while` loop, `if/else if/else`, modulo `%` | FizzBuzz sequence 1-20 |
| `fibonacci.cx` | Functions (`fnc:`), iterative computation, `while` loop | Fibonacci numbers fib(0) through fib(10) |
| `structs_and_methods.cx` | `struct`, `impl` block, method calls, field access | Player state changes after damage/heal |
| `error_handling.cx` | `Result<T>`, `Ok()`, `Err()`, `?` try operator | Success and error propagation results |
| `arrays_and_loops.cx` | Array `[N: Type]`, `for` loop, `while in` cursor iteration | Array elements and computed values |
| `generics.cx` | Generic `struct Pair<T>`, type-parameterized instantiation | Pair fields with different types |
| `tbool_uncertainty.cx` | `tbool`, unknown literal `?`, `is_known()`, propagation | Uncertainty state transitions |

All examples exit 0 and produce deterministic output. None are intentional failure cases.
