# Cx CLI Flags

## Supported Flags

- `--debug`
  - Enables all debug modes (`--debug-tokens`, `--debug-ast`, `--debug-scope`, `--debug-step`, `--debug-phase`).

- `--debug-tokens`
  - Prints the lexer token table.

- `--debug-ast`
  - Prints the parsed AST tree.

- `--debug-scope`
  - Prints runtime scope events (open/close/add/mutate/free/bleed-back).

- `--debug-step`
  - Runs in step mode and pauses before each top-level statement.

- `--debug-phase`
  - Prints phase timing for lexer, parser, semantic, and runtime passes.

## Usage

- Default file (no path provided):
  - `cargo run --`

- Run a specific file:
  - `cargo run -- src/tests/func_test.cx`

- Run with flags:
  - `cargo run -- --debug`
  - `cargo run -- src/tests/func_test.cx --debug-scope`
