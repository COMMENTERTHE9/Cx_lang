# CX-44: Per-Branch Rebase Verification

**Submain tip audited:** `abb71a5` (post CX-40 + CX-41)
**Date:** 2026-05-06
**Method:** Each branch checked out and `cargo build --features jit` + `cargo test --features jit` run independently.

## Submain Baseline Fix

`cargo test --features jit` was broken on `submain` because CX-41 introduced
3 `BlockParam { value, ty }` struct initializers in test code without the
`read_only: bool` field added by CX-40.  Fixed in this branch with `read_only: false`.

File: `src/backend/cranelift/host_boundary.rs` (lines 1202, 1464, 1470)

## Branch Verification Results

| Branch              | PR  | Commit    | Base Commit | Build | Tests Passed | Status |
|---------------------|-----|-----------|-------------|-------|--------------|--------|
| stokowski/cx-30     | #81 | `dd33b3c` | `abb71a5`   | PASS  | 200          | PASS ✓ |
| stokowski/cx-32     | #83 | `36037fa` | `abb71a5`   | PASS  | 198          | PASS ✓ |
| stokowski/cx-33     | #84 | `75f3f2b` | `abb71a5`   | PASS  | 200          | PASS ✓ |
| stokowski/cx-34     | #85 | `6496c88` | `abb71a5`   | PASS  | 196          | PASS ✓ |
| stokowski/cx-36     | #87 | `edb9881` | `abb71a5`   | PASS  | 201          | PASS ✓ |
| stokowski/cx-38     | #89 | `23827ad` | `abb71a5`   | PASS  | 201          | PASS ✓ |

All 6 branches carry exactly 1 commit on top of `abb71a5` (behind=0, ahead=1).
No branch is empty relative to submain.

## Submain Baseline

```
cargo build --features jit  → Finished (21 warnings, 0 errors)
cargo test --features jit   → test result: ok. 194 passed; 0 failed
```

(After applying the `read_only: false` fix to `host_boundary.rs` test fixtures.)
