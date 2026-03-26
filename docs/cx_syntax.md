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
Standard infix form:
```cx
i += 1
i += 2
i -= 1
i *= 2
i /= 2
i %= 2
```

## 2. Built-in Types

Stable, verified surface types:
- `t8`
- `t16`
- `t32`
- `t64`
- `t128`
- `f64`
- `bool`
- `str`
- `char`

Additional types currently present in the surface/type system:
- `strref`
- `Container`
- `unknown`
- `Handle<T>`
- `Enum(Name)` internally

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
```

### Index access
```cx
arr:[0]
```
Colon before bracket distinguishes index access from type annotation.

### Field access
```cx
t.x
p.health
```
Used for struct fields, `copy_into(...)` containers, and other container-like values.

## 5. Printing

### Line print (adds newline)
```cx
print(x)
```

### Inline print (no newline)
```cx
printn(x)
```

Both `print` and `printn` are regular functions, not statements.

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
    i += 1
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
    i += 1
}
```

### Continue
`continue` is part of the language and runtime, though the current matrix focuses more on `break`.

## 12. Arrays and Indexing

Array literals and indexing:
```cx
[1, 2, 3]
arr:[0]
```
Arrays support declaration, initialization, partial init, index read/write, function pass/return, and copy semantics.

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

## 15. If / Else / Else-If

```cx
if x > 10 {
    print("big")
} else if x > 5 {
    print("medium")
} else {
    print("small")
}
```

## 16. Structs

### Definition
```cx
struct Player {
    health: t32,
    speed: t32,
}
```

### Instantiation and field access
```cx
let p = Player { health: 100, speed: 5 }
print(p.health)
```

### Impl blocks
```cx
impl Player {
    fnc: t32 damage(amount: t32) {
        self.health -= amount
        self.health
    }
}
```

## 17. Const Declarations

```cx
const MAX_HP: t32 = 100
const GRAVITY: f64 = 9.8
```
Literal-only initializers. The semantic pass rejects reassignment.

## 18. Generic Functions

```cx
fnc: <T> identity(x: T) {
    x
}
```
Single and multiple type parameters supported.

## 19. Generic Structs

```cx
struct Pair<T> {
    first: T,
    second: T,
}
```

## 20. Imports

Multi-file programs use `#![imports]` blocks:
```cx
#![imports]
math: use "./math"
player: use "./player"
```
Only `pub`-marked declarations in the imported file cross module boundaries.

## 21. Input

### Prompted input
```cx
input("Enter name: ", name)
```

### Raw read
```cx
read(var)
```
Both read from stdin and fill the target variable.

## 22. String Interpolation

```cx
print("name: {name}")
```
Variables inside `{}` are expanded at print time.

## 23. For Loops

```cx
for i in 0..10 {
    print(i)
}
```
Inclusive ranges:
```cx
for i in 0..=10 {
    print(i)
}
```

## 24. Current Known Syntax Gaps

These are present in the type/runtime surface but not fully settled as end-user syntax yet:
- `strref` exists and is semantically enforced at boundaries, but the language surface around producing real `strref` values is still thin
- `Container` is split from `str` in the type system, but is mostly an internal/runtime-facing concept right now

## 25. Verified Source of Truth

If you want the real current language surface, check:
- `src/frontend/lexer.rs`
- `src/frontend/parser.rs`
- `src/tests/verification_matrix/`

The verification matrix is the best quick answer to: �does this syntax actually work today?�
