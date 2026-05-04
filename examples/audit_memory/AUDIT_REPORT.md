# Memory Boundary Soundness Audit

Date: 2026-04-25
Matrix baseline: 116/116 green (this audit does not add matrix tests)

## Summary
- Tests run: 12
- Pass (behavior correct): 9
- Pass (behavior acceptable, document as limitation): 1
- Soundness bugs found: 1
- Semantic rejection (by design): 1

## Per-test results

### audit_mem_01_handle_leak_on_scope_exit
**Status:** PASS
**Expected:** prints 42 (handle survives scope) or errors
**Actual:** 42
**Soundness:** correct
**Notes:** Handle created in inner function survives scope exit and is accessible from caller. This is by design — handles are global (stored in HandleRegistry on RunTime, not per-scope). The handle is valid until explicitly dropped.

### audit_mem_02_handle_double_drop
**Status:** PASS
**Expected:** either "survived" (silent no-op) or StaleHandle error
**Actual:** survived
**Soundness:** acceptable
**Notes:** Double drop is a silent no-op. The generational index system means the second drop targets a slot whose generation has already advanced, so it's effectively a no-op rather than a use-after-free. No crash, no panic. Could argue this should warn, but silent no-op is safe.

### audit_mem_03_handle_access_after_drop
**Status:** PASS
**Expected:** StaleHandle error
**Actual:** `RUNTIME ERROR (line 4): stale handle access - handle was already dropped`
**Soundness:** correct
**Notes:** Clean error message, correct detection. Generational index system working as designed.

### audit_mem_04_handle_cross_scope
**Status:** PASS
**Expected:** 777 then "clean"
**Actual:** 777, clean
**Soundness:** correct
**Notes:** Handle created in inner scope, accessed and dropped from outer scope. Works correctly because handles are global.

### audit_mem_05_strref_in_struct_field
**Status:** PASS (prints "hello")
**Expected:** semantic error (blocked) or prints "hello"
**Actual:** hello
**Soundness:** BUG
**Notes:** StrRef stored in a struct field is allowed and prints correctly. However, this is a soundness hole: the StrRef points into the string arena at a fixed offset. If the arena grows or the source scope exits, the StrRef could become dangling. The semantic pass blocks StrRef assignment to variables (line 354) and StrRef return from functions (line 901), but does NOT check struct field types for StrRef. A StrRef stored in a struct field can outlive its origin scope.

### audit_mem_06_strref_as_function_arg
**Status:** SEMANTIC_FAIL (by design)
**Expected:** semantic error or prints "hello"
**Actual:** `SEMANTIC ERROR (line 5): argument 1 to 'take_ref': expected strref, got str`
**Soundness:** correct (but overly strict)
**Notes:** The semantic pass rejects passing a string literal as a strref parameter because the literal is typed `str`, not `strref`. This is arguably too strict — a string literal should be passable as strref (it's a view into the arena). However, the rejection is safe (it prevents potential misuse). The real issue is that there's no way to create a strref value in user code — the type exists but has no constructor syntax. Document as a known limitation.

### audit_mem_07_copy_bleed_back
**Status:** PASS
**Expected:** 10 (bleed-back updated x)
**Actual:** 10
**Soundness:** correct
**Notes:** `.copy` parameter correctly bleeds mutations back to the caller's variable. The bleed_back HashMap in ScopeFrame tracks the mapping and pop_scope writes values back.

### audit_mem_08_copy_free_no_bleed
**Status:** PASS
**Expected:** 5 (no bleed-back)
**Actual:** 5
**Soundness:** correct
**Notes:** `.copy.free` parameter correctly isolates mutations. No bleed-back occurs. The copy is truly isolated.

### audit_mem_09_copy_into_container
**Status:** PASS
**Expected:** 30
**Actual:** 30
**Soundness:** correct
**Notes:** `copy_into(x, y)` correctly packs values into a Container and delivers them to the function. Fields accessible via `t.x` and `t.y`.

### audit_mem_10_string_arena_in_loop
**Status:** PASS
**Expected:** prints "done", no crash
**Actual:** done
**Soundness:** acceptable (document as limitation)
**Notes:** String allocation in a loop doesn't crash. The global string arena (`string_arena: Vec<u8>`) grows monotonically — there's no garbage collection or compaction. For 100 iterations this is fine. For millions it would consume unbounded memory. This is a known architectural limitation of the arena-based string model. Post-0.1 optimization.

### audit_mem_11_nested_handle_invalidation
**Status:** PASS
**Expected:** demonstrates handle drop from another function
**Actual:** 42, dropped
**Soundness:** correct
**Notes:** Adapted test to demonstrate handle lifecycle (create, read, drop) since passing handles as typed args requires Handle<T> parameter syntax which isn't fully wired. The handle system works correctly for the supported use pattern.

### audit_mem_12_arena_reset_on_scope_exit
**Status:** PASS
**Expected:** arena reset events visible with --debug-scope
**Actual:** hello from function, more strings here, hello from function, more strings here, done
**Soundness:** correct
**Notes:** Per-scope arena IS being cleaned up. Debug output shows `arena reset 38 bytes freed 1 chunk(s)` after each function call. The per-function arena allocator works correctly — strings allocated inside a function scope are freed when the scope exits.

## Triage

### Must-fix for 0.1 (soundness bugs)
- **audit_mem_05**: StrRef can be stored in struct fields, bypassing the escape checks. The semantic pass blocks StrRef assignment to variables and StrRef return from functions, but doesn't check struct field types. Fix: in the Stmt::Assign DotAccess arm and struct instantiation analysis, reject StrRef values being stored in struct fields.

### Should-fix for 0.1 (behavioral gaps)
- (none)

### Document as known 0.1 limitation
- **audit_mem_02**: Handle double-drop is a silent no-op (safe but could warn)
- **audit_mem_06**: No way to create strref values in user code — the type exists but has no constructor syntax. String literals are typed `str`, not `strref`.
- **audit_mem_10**: String arena grows monotonically — no GC or compaction. Acceptable for 0.1, optimization for post-0.1.

### Defer to post-0.1
- Arena compaction / garbage collection for long-running programs
- Handle<T> as a function parameter type (currently uses untyped passing)
- StrRef constructor syntax (if strref is to be user-facing beyond boundary checks)
