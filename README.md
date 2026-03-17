# Cx

Cx is a systems language for game engines with deterministic memory, no garbage collector, and no borrow checker.

## The Problem It Solves
Engine code often needs predictable allocation, explicit lifetime control, and data movement you can reason about frame to frame. Teams building gameplay, tooling, and runtime systems usually end up mixing high-level ergonomics with low-level ownership concerns by hand. Cx is aimed at that gap: a language where memory behavior is visible, stable, and intentional without turning every feature into allocator plumbing. The goal is to make engine-facing code easier to reason about under load, at boundaries, and over long-lived runtime sessions.

## The Approach
Cx is built around arenas, handles, and explicit value movement. Owned values stay owned, handles give you stable indirection with stale-handle detection, and scoped arena cleanup keeps teardown deterministic. Unknown state is part of the language model rather than an afterthought, which lets control-flow rules stay explicit.

## A Code Taste
```cx
fnc spawn_enemy(kind: EnemyKind) {
    let h;
    h = Handle.new(kind)

    when kind {
        EnemyKind::Grunt => print("grunt"),
        EnemyKind::Elite => print("elite"),
        _ => print("unknown"),
    }

    print(h.val)
    h.drop()
}
```

## Current Status
Cx currently runs through a tree-walk interpreter written in Rust. The frontend has a Logos lexer, a Chumsky parser, semantic analysis, enums, `when`, loops, handles, unknown propagation, `.copy` / `.copy.free` / `copy_into`, and scoped runtime cleanup. There is no compiled output yet; backend work is scaffolded but still stubbed. Some type-surface pieces such as `StrRef`, grouped enum paths, and future control-flow features are in progress and not fully exercised across the entire language yet.

## Roadmap
- Cranelift JIT backend
- Arrays and contiguous container features
- Expanded Unknown-state semantics
- Structs and richer user types
- Standard library and runtime utilities

## Built With
- Rust: implementation language for the compiler, interpreter, and tooling
- Logos: tokenization
- Chumsky: parser construction

## Contributing / Contact
Open an issue or a PR in this repo if you want to discuss language behavior, runtime semantics, or backend work. If you are following progress, the verification matrix in `src/tests/verification_matrix/` is the clearest picture of what is actually working today.
