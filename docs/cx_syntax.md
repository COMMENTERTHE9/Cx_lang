# Cx Syntax Reference

This document describes the Cx surface syntax that is working in the current tree-walk interpreter today. It is intentionally practical: if a form is listed here, it is based on the parser and the verification matrix, not on planned features.

## 1. Declarations and Assignment

### Untyped declaration, inferred on assignment
```cx
let x;
x = 56
```

### Typed declaration with immediate assignment
```cx
score: t64 = 10
name: str = "Zara"
flag: bool = true
```

### Typed declaration without initializer
```cx
let hp: t64;
hp = 100
```

### Assignment
```cx
x = x + 1
```

### Compound assignment
Current syntax is compact and nonstandard:
```cx
i +1=
i +2=
i -1=
i *2=
i /2=
i %2=
```
It is **not** `i += 1` today.

## 2. Built-in Types

Stable, verified surface types:
- `t8`
- `t16`
- `t32`
- `t64`
- `t128`
- `bool`
- `str`
- `char`

Additional types currently present in the surface/type system:
- `strref`
- `Container`
- `unknown`
- `Handle<T>`
- `Enum(Name)` internally
- arrays exist in the AST surface as `[N]Type`, but they are not documented here as stable user-facing syntax yet

## 3. Literals

### Numbers
```cx
10
3.14
```

### Strings
```cx
"hello"
"hello {name}"
```
String templates expand through the runtime.

### Chars
```cx
'a'
'\n'
```

### Booleans
```cx
true
false
```

### Unknown literal
```cx
?
```

### Enum variants
```cx
Direction::North
```

## 4. Expressions and Operators

### Arithmetic
```cx
a + b
a - b
a * b
a / b
a % b
```

### Comparison
```cx
a == b
a < b
a > b
a <= b
a >= b
```

### Logical
```cx
a && b
a || b
```

### Unary
```cx
-x
*value
```
`*` currently behaves as an identity-style dereference placeholder. Full pointer/array dereference semantics are not finished.

### Field access
```cx
t.x
```
Used for `copy_into(...)` containers and other container-like values.

## 5. Printing

### Line print
```cx
print(x)
```

### Inline print
```cx
print!(x)
```

## 6. Functions

### Definition
```cx
fnc: t64 add(a: t64, b: t64) {
    a + b
}
```

### Void-style function
```cx
fnc: greet(name: str) {
    print(name)
}
```

### Explicit return
```cx
fnc: t64 pick(a: t64, b: t64) {
    return a;
}
```

### Implicit return
The final bare expression in a function body is the implicit return value.
```cx
fnc: t64 double(x: t64) {
    x + x
}
```

### Forward declaration
Top-level forward calls work today.
```cx
outer()

fnc: outer() {
    print(99)
}
```

### Nested functions
Nested functions are scoped to their containing function and do not leak globally.

## 7. Copy Semantics

### `.copy`
Bleeds back into the caller.
```cx
fnc: t64 inner(x.copy) {
    x = x + 5;
    x
}
```

### `.copy.free`
Isolated copy, no bleed-back.
```cx
fnc: t64 inner(x.copy.free) {
    x = x + 5;
    x
}
```

### `copy_into(...)`
Builds a container-like parameter with named fields.
```cx
fnc: t64 inner(t: copy_into(x, y, z)) {
    t.x + t.y + t.z
}

result: t64 = inner(copy_into(x, y, z));
```

## 8. Handles

### Allocate a handle
```cx
let h;
h = Handle.new(99)
```

### Read a handle value
```cx
print(h.val)
```

### Drop a handle
```cx
h.drop()
```

Handles use slot + generation semantics internally, so stale access can be detected.

## 9. `when` Matching

`when` is the primary branching form in Cx today.

### Basic value match
```cx
when x {
    1 => print(10),
    2 => print(20),
    _ => print(99),
}
```

### Unknown match
```cx
when x {
    true    => print(1),
    false   => print(0),
    unknown => print(2),
}
```

### Range match
```cx
when x {
    1..=5   => print(10),
    6..=10  => print(20),
    _       => print(99),
}
```

### Enum variant match
```cx
when d {
    Direction::North => print(1),
    Direction::South => print(2),
    _ => print(99),
}
```

### Grouped enum match
```cx
when e {
    alive    => print(1),
    combat   => print(2),
    inactive => print(3),
}
```

### Super-group shorthand
Current parser/runtime also supports grouped handler forms used in the verification matrix, for example:
```cx
when e {
    User_states => print(1), print(2), {_},
    inactive    => print(3),
}
```
This area is more specialized and still evolving.

## 10. Enums

### Flat enum
```cx
enum Direction { North, South, East, West }
```

### Grouped enum
```cx
enum EntityState {
    group ::alive [
        Running,
        Walking,
        Idle,
    ],
    group ::combat [
        Attacking,
        Blocking,
        Stunned,
    ],
}
```

### Super-group enum layout
```cx
enum EntityState {
    group ::User_states [
        alive [ Running, Walking, Idle ],
        combat [ Attacking, Blocking, Stunned ],
    ]
}
```

## 11. Loops

### While
```cx
let i;
i = 0
while (i < 5) {
    print(i)
    i +1=
}
```

### For range
```cx
for i in 0..5 {
    print(i)
}
```
Inclusive ranges also exist:
```cx
for i in 0..=5 {
    print(i)
}
```

### Infinite loop with break
```cx
loop {
    when i {
        5 => break,
        _ => print(i),
    }
    i +1=
}
```

### Continue
`continue` is part of the language and runtime, though the current matrix focuses more on `break`.

## 12. Arrays and Indexing

The AST and parser surface currently include array literals and indexing:
```cx
[1, 2, 3]
arr:[0]
```
This part of the language is not yet documented as stable because the current matrix is still centered on the core runtime features above.

## 13. Unknown and State Checks

### Unknown propagation
```cx
let x;
x = ?
let y;
y = 5 + x
print(y)  // ?
```

### Logical short-circuit with unknown
```cx
let z;
z = false
let w;
w = z && x
print(w)  // false
```

### Known-state primitive
```cx
print(is_known(a))
```
Current verified behavior shows `is_known(...)` working for bool, numeric, and string values.

## 14. Semicolon Rules

Cx is currently a little strict here.

### Required
- `let x;`
- `return x;`
- many expression statements inside function bodies, for example:
```cx
greet(hero);
describe(weapon);
```

### Commonly omitted in current tests
- plain assignments:
```cx
x = 3
```
- some top-level expression statements
- final implicit return expression in a function body

The safe rule today: if a call is not the final expression in a function body, terminate it with `;`.

## 15. Current Known Syntax Gaps

These are present in the type/runtime surface but not fully settled as end-user syntax yet:
- `strref` exists and is semantically enforced at boundaries, but the language surface around producing real `strref` values is still thin
- `Container` is split from `str` in the type system, but is mostly an internal/runtime-facing concept right now
- `TBool` exists in runtime/type plumbing but is not fully surfaced as a stable language feature
- arrays are scaffolded in the parser/AST but not yet part of the documented stable feature set

## 16. Verified Source of Truth

If you want the real current language surface, check:
- `src/frontend/lexer.rs`
- `src/frontend/parser.rs`
- `src/tests/verification_matrix/`

The verification matrix is the best quick answer to: �does this syntax actually work today?�
