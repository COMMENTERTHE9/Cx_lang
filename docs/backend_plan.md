# Backend Plan

## Current State
- Interpreter backend is the active/default path and works.
- Cranelift backend is wired but currently stubbed.
- LLVM backend is wired but currently stubbed.
- IR lowering boundary exists and currently returns a placeholder IR module.

## Next Milestone
- Implement Cranelift JIT for a tiny subset of Cx first.

## Implementation Location
- Main implementation target: `src/backend/cranelift/jit.rs`

## Safety Note
- Do not touch interpreter path while implementing JIT.
