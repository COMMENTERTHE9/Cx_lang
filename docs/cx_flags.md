# Cx CLI Flags

## Supported Flags

- `--debug`
  - Enables all debug modes (`--debug-tokens`, `--debug-ast`, `--debug-scope`, `--debug-phase`, `--debug-trace`).

- `--debug-tokens`
  - Prints the lexer token table.

- `--debug-ast`
  - Prints the parsed AST tree.

- `--debug-scope`
  - Prints runtime scope events (open/close/add/mutate/free/bleed-back).

- `--debug-phase`
  - Prints phase timing for lexer, parser, semantic, and runtime passes.

- `--debug-trace`
  - Prints each IR instruction as it is emitted during lowering.

- `--backend=interp`
  - Runs the program through the tree-walk interpreter (default).

- `--backend=cranelift`
  - Runs the program through the Cranelift JIT backend.

- `--backend=validate`
  - Performs IR lowering and validation, then pretty-prints the IR. No codegen or execution.

## Usage

- Default file (no path provided):
  - `cargo run --`

- Run a specific file:
  - `cargo run -- src/tests/func_test.cx`

- Run with flags:
  - `cargo run -- --debug`
  - `cargo run -- src/tests/func_test.cx --debug-scope`
  - `cargo run -- src/tests/func_test.cx --backend=validate`
