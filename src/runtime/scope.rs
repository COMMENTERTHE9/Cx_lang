use super::runtime::*;
use crate::frontend::{ast::*, diagnostics, types::*};
use crate::frontend::semantic_types::*;

impl RunTime {
    fn resolve_assigned_value(&mut self, value: Value, pos: usize) -> Result<Value, RuntimeError> {
        match value {
            Value::Str(off, len) => {
                let expanded = expand_template(self, self.resolve_str(off, len), pos)?;
                let (off, len) = self.alloc_str(&expanded);
                Ok(Value::Str(off, len))
            }
            other => Ok(other),
        }
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(ScopeFrame::new());
        if self.debug_scope {
            diagnostics::print_scope_event(&ScopeEvent::Open(format!(
                "scope#{}",
                self.scopes.len() - 1
            )));
        }
    }

    pub fn push_function_scope(&mut self, names: std::sync::Arc<std::collections::HashMap<String, BindingId>>) {
        self.name_frames.push((self.scopes.len(), self.lexical_names.replace(names)));
        self.scopes.push(ScopeFrame::new());
        if self.debug_scope {
            diagnostics::print_scope_event(&ScopeEvent::Open(format!(
                "scope#{}",
                self.scopes.len() - 1
            )));
        }
    }

    pub fn pop_scope(&mut self) {
        if self.scopes.is_empty() {
            self.scopes.pop();
            return;
        }

        let (bleed_values, debug_info) = {
            let frame = self.scopes.last().unwrap();
            let bleed_values: Vec<(usize, BindingId, Value)> = frame.bleed_back.iter()
                .filter_map(|(parameter, (owner, binding, _))| {
                    frame.vars.get(parameter).and_then(|entry| entry.val.clone())
                        .map(|value| (*owner, *binding, value))
                }).collect();

            let debug_info = if self.debug_scope {
                let bleed_events: Vec<(String, Value)> = frame.bleed_back.iter()
                    .filter_map(|(parameter, (_, _, name))| {
                        frame.vars.get(parameter).and_then(|entry| entry.val.clone())
                            .map(|value| (name.clone(), value))
                    }).collect();
                let close_label = format!("scope#{}", self.scopes.len() - 1);
                Some((bleed_events, close_label))
            } else {
                None
            };

            (bleed_values, debug_info)
        };

        if self.name_frames.last().is_some_and(|(index, _)| *index + 1 == self.scopes.len()) {
            self.lexical_names = self.name_frames.pop().unwrap().1;
        }
        self.scopes.pop();

        for (outer_idx, outer_binding, val) in bleed_values {
            if let Some(outer_frame) = self.scopes.get_mut(outer_idx) {
                if let Some(entry) = outer_frame.vars.get_mut(&outer_binding) {
                    entry.val = Some(val);
                }
            }
        }

        if let Some((bleed_events, close_label)) = debug_info {
            for (name, val) in &bleed_events {
                diagnostics::print_scope_event(&ScopeEvent::BleedBack(name.clone(), val.clone()));
            }
            diagnostics::print_scope_event(&ScopeEvent::Close(close_label));
        }
    }

    pub fn declare(
        &mut self,
        binding: BindingId,
        name: String,
        ty: Option<Type>,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let local = self.scopes.len() > 1;
        let frame = self.scopes.last_mut().unwrap();

        if frame.vars.contains_key(&binding) || (local && frame.contains_name(&name)) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        frame.seen.insert(name.clone());
        frame.insert_var(binding, &name, VarEntry { ty, val: None });
        Ok(())
    }

    /// Refuse to reach a variable that is the receiver of a method still
    /// running (amendment V5; `RunTime::lent_receivers`). Called by every read
    /// and write below, once the variable's frame and binding are known.
    fn check_not_lent(&self, frame: usize, binding: BindingId, name: &str, pos: usize) -> Result<(), RuntimeError> {
        if !self.lent_receivers.is_empty() && self.lent_receivers.contains(&(frame, binding)) {
            return Err(RuntimeError::ReceiverInUse { pos, name: name.to_string() });
        }
        Ok(())
    }

    /// Shared write path: type-check `value` against the entry at `binding` in
    /// frame `i`, write it, emit debug events. The caller has already located the
    /// owning frame (by binding on the hot path, by name on the cold path).
    fn write_var_at(
        &mut self,
        i: usize,
        binding: BindingId,
        name: &str,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let frame = &mut self.scopes[i];
        let entry = frame.vars.get_mut(&binding).unwrap();

        let was_initialized = entry.val.is_some();
        if entry.ty.is_none() || declared_with_type_parameter(entry.ty.as_ref(), &value) {
            entry.ty = Some(type_of_value(&value));
        }

        let expected = entry.ty.clone().unwrap();
        let got = type_of_value(&value);
        if !value_matches_type(&value, &expected) {
            return Err(RuntimeError::TypeMismatch { pos, expected, got });
        }

        entry.val = Some(value);

        if self.debug_scope {
            let logged = entry.val.clone().unwrap();
            if was_initialized {
                diagnostics::print_scope_event(&ScopeEvent::Mutate(name.to_string(), logged));
            } else {
                diagnostics::print_scope_event(&ScopeEvent::Add(name.to_string(), logged));
            }
        }
        Ok(())
    }

    /// Hot-path write: assign to the variable identified by `binding`. `name` is
    /// used only for the const-protection check and diagnostics.
    pub fn set_var_by_id(
        &mut self,
        binding: BindingId,
        name: &str,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        if self.const_bindings.contains(&binding) {
            return Err(RuntimeError::BadAssignTarget { pos });
        }
        let value = self.resolve_assigned_value(value, pos)?;
        for i in (0..self.scopes.len()).rev() {
            if self.scopes[i].vars.contains_key(&binding) {
                self.check_not_lent(i, binding, name, pos)?;
                return self.write_var_at(i, binding, name, value, pos);
            }
        }
        let was_seen = self.scopes.last().unwrap().seen.contains(name);
        Err(diagnostics::unresolved_var_error(pos, name.to_string(), was_seen))
    }

    /// Cold-path write by name (string-interp targets, while-in arrays, method
    /// write-back). Resolves name -> BindingId via the per-frame index.
    pub fn set_var(&mut self, name: String, value: Value, pos: usize) -> Result<(), RuntimeError> {
        let value = self.resolve_assigned_value(value, pos)?;
        if let Some((i, binding)) = self.resolve_name(&name) {
                if self.const_bindings.contains(&binding) {
                    return Err(RuntimeError::BadAssignTarget { pos });
                }
                self.check_not_lent(i, binding, &name, pos)?;
                return self.write_var_at(i, binding, &name, value, pos);
        }
        let was_seen = self.scopes.last().unwrap().seen.contains(&name);
        Err(diagnostics::unresolved_var_error(pos, name, was_seen))
    }

    pub fn set_var_typed(
        &mut self,
        binding: BindingId,
        name: String,
        ty: Type,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let value = self.resolve_assigned_value(value, pos)?;
        let logged = value.clone();
        let got = type_of_value(&value);
        // `y: T = x` inside a generic function: see `declared_with_type_parameter`.
        let ty = if declared_with_type_parameter(Some(&ty), &value) { got.clone() } else { ty };
        if !value_matches_type(&value, &ty) {
            return Err(RuntimeError::TypeMismatch {
                pos,
                expected: ty,
                got,
            });
        }

        {
            let local = self.scopes.len() > 1;
            let frame = self.scopes.last_mut().unwrap();
            if frame.vars.contains_key(&binding) || (local && frame.contains_name(&name)) {
                return Err(RuntimeError::AlreadyDeclared { pos, name });
            }

            frame.seen.insert(name.clone());
            frame.insert_var(
                binding,
                &name,
                VarEntry {
                    ty: Some(ty),
                    val: Some(value),
                },
            );
            if self.debug_scope {
                diagnostics::print_scope_event(&ScopeEvent::Add(name, logged.clone()));
            }
        }

        Ok(())
    }

    pub fn set_container_field(
        &mut self,
        container: &str,
        field: &str,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let logged = value.clone();
        if let Some((owner, binding)) = self.resolve_name(container) {
            self.check_not_lent(owner, binding, container, pos)?;
            if let Some(entry) = self.scopes[owner].vars.get_mut(&binding) {
                match &mut entry.val {
                    Some(Value::Container(map)) => {
                        map.insert(field.to_string(), value);
                        if self.debug_scope {
                            diagnostics::print_scope_event(&ScopeEvent::Mutate(
                                format!("{}.{}", container, field),
                                logged,
                            ));
                        }
                        return Ok(());
                    }
                    Some(Value::Struct(_, map)) => {
                        map.insert(field.to_string(), value);
                        if self.debug_scope {
                            diagnostics::print_scope_event(&ScopeEvent::Mutate(
                                format!("{}.{}", container, field),
                                logged,
                            ));
                        }
                        return Ok(());
                    }
                    _ => {
                        return Err(RuntimeError::NotAContainer {
                            pos,
                            name: container.to_string(),
                        });
                    }
                }
            }
        }
        Err(RuntimeError::UndefinedVar {
            pos,
            name: container.to_string(),
        })
    }

    /// Write a single element of an array variable in scope.
    ///
    /// Traverses scopes from innermost outward, finds the named variable,
    /// and replaces the element at `index` with `value`.  Returns an error
    /// if the variable is not found, is not an array, or the index is
    /// out of bounds.
    /// Write an array element, addressed by a root and a path of indices.
    ///
    /// `[i]` is the one-dimensional case and `[i, j, ...]` descends a nested
    /// array. The interpreter reaches an inner element by walking the owned
    /// value in place through `&mut`, so multidimensional writes need no
    /// reference or pointer concept — just a longer path. One function rather
    /// than a single-index one plus a nested one, so there is a single place
    /// where an out-of-bounds index is detected and reported.
    pub fn set_array_element(
        &mut self,
        arr_name: &str,
        root_field: Option<&str>,
        path: &[usize],
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        if path.is_empty() {
            return Err(RuntimeError::BadAssignTarget { pos });
        }
        if let Some((owner, binding)) = self.resolve_name(arr_name) {
            self.check_not_lent(owner, binding, arr_name, pos)?;
            if let Some(entry) = self.scopes[owner].vars.get_mut(&binding) {
                let Some(mut cur) = entry.val.as_mut() else {
                    return Err(RuntimeError::NotAContainer { pos, name: arr_name.to_string() });
                };
                // The root may be a variable or one of its fields — `a:[i] = v`
                // and `g.cells:[i] = v` differ only in where the descent starts,
                // so the walk below is shared rather than duplicated.
                if let Some(field) = root_field {
                    let (Value::Struct(_, map) | Value::Container(map)) = cur else {
                        return Err(RuntimeError::NotAContainer { pos, name: arr_name.to_string() });
                    };
                    cur = map.get_mut(field).ok_or_else(|| RuntimeError::UndefinedVar {
                        pos,
                        name: format!("{arr_name}.{field}"),
                    })?;
                }
                for (depth, idx) in path.iter().enumerate() {
                    let Value::Array(elems) = cur else {
                        return Err(RuntimeError::NotAContainer { pos, name: arr_name.to_string() });
                    };
                    let length = elems.len();
                    if *idx >= length {
                        return Err(RuntimeError::IndexOutOfBounds {
                            pos,
                            index: *idx as i64,
                            length,
                        });
                    }
                    if depth + 1 == path.len() {
                        elems[*idx] = value;
                        return Ok(());
                    }
                    cur = &mut elems[*idx];
                }
                return Err(RuntimeError::BadAssignTarget { pos });
            }
        }
        Err(RuntimeError::UndefinedVar {
            pos,
            name: arr_name.to_string(),
        })
    }

    pub fn get_field(&self, container: &str, field: &str, pos: usize) -> Result<Value, RuntimeError> {
        if let Some((owner, binding)) = self.resolve_name(container) {
            self.check_not_lent(owner, binding, container, pos)?;
            if let Some(entry) = self.scopes[owner].vars.get(&binding) {
                match &entry.val {
                    Some(Value::Struct(_, map)) | Some(Value::Container(map)) => {
                        return map.get(field).cloned().ok_or_else(|| {
                            RuntimeError::UndefinedVar { pos, name: format!("{}.{}", container, field) }
                        });
                    }
                    _ => return Err(RuntimeError::NotAContainer { pos, name: container.to_string() }),
                }
            }
        }
        Err(RuntimeError::UndefinedVar { pos, name: container.to_string() })
    }

    /// Cold-path read by name (string interpolation, container/array name
    /// resolution, `.copy` args). Resolves name -> BindingId via the per-frame
    /// index, then reads.
    pub fn get_var(&self, name: &str, pos: usize) -> Result<Value, RuntimeError> {
        if let Some((owner, binding)) = self.resolve_name(name) {
            self.check_not_lent(owner, binding, name, pos)?;
            if let Some(entry) = self.scopes[owner].vars.get(&binding) {
                if let Some(value) = &entry.val {
                    return Ok(value.clone());
                }
                return Err(RuntimeError::UninitializedVar {
                    pos,
                    name: name.to_string(),
                });
            }
        }
        let owned = name.to_string();
        let was_seen = self.scopes.last().unwrap().seen.contains(&owned);
        Err(diagnostics::unresolved_var_error(pos, owned, was_seen))
    }

    pub(crate) fn resolve_name(&self, name: &str) -> Option<(usize, BindingId)> {
        let start = if self.lexical_names.is_some() {
            self.name_frames.last().map(|(index, _)| *index).unwrap_or(1)
        } else { 0 };
        for index in (start..self.scopes.len()).rev() {
            if let Some(binding) = self.scopes[index].by_name.get(name) {
                return Some((index, *binding));
            }
        }
        let binding = *self.lexical_names.as_ref()?.get(name)?;
        self.scopes.iter().rposition(|frame| frame.vars.contains_key(&binding))
            .map(|index| (index, binding))
    }

    /// String parameters expand after arguments are evaluated, before the
    /// function body. Retain the caller's lexical names during that binding
    /// phase; already-bound parameters still shadow them in the new frame.
    pub(crate) fn snapshot_names(&self) -> std::sync::Arc<std::collections::HashMap<String, BindingId>> {
        let mut names = self.lexical_names.as_ref().map(|n| n.as_ref().clone()).unwrap_or_default();
        let start = if self.lexical_names.is_some() {
            self.name_frames.last().map(|(index, _)| *index).unwrap_or(1)
        } else { 0 };
        for frame in &self.scopes[start..] {
            names.extend(frame.by_name.iter().map(|(name, binding)| (name.clone(), *binding)));
        }
        std::sync::Arc::new(names)
    }

    /// Hot-path read by BindingId (VarRef, CompoundAssign read). `name` is used
    /// only for diagnostics. Walks frames innermost->outermost exactly like the
    /// name-based path, but matches on the BindingId — no string hashing.
    pub fn get_var_by_id(&self, binding: BindingId, name: &str, pos: usize) -> Result<Value, RuntimeError> {
        for (index, frame) in self.scopes.iter().enumerate().rev() {
            if let Some(entry) = frame.vars.get(&binding) {
                self.check_not_lent(index, binding, name, pos)?;
                if let Some(value) = &entry.val {
                    return Ok(value.clone());
                }
                return Err(RuntimeError::UninitializedVar {
                    pos,
                    name: name.to_string(),
                });
            }
        }
        let was_seen = self.scopes.last().unwrap().seen.contains(name);
        Err(diagnostics::unresolved_var_error(pos, name.to_string(), was_seen))
    }
}

#[cfg(test)]
mod lexical_name_tests {
    use super::*;
    use std::{collections::HashMap, sync::Arc};

    fn source_answer(source: &str) -> String {
        use crate::frontend::{resolver, semantic};
        let id = resolver::ModuleId(0);
        let file = resolver::ResolvedFile { id, path: "lexical_test.cx".into(),
            program: resolver::parse_source(source).unwrap_or_else(|error| panic!("{error}: {source}")), imports: vec![] };
        let program = semantic::analyze_resolved_program(&resolver::ResolvedProgram {
            entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id],
        }).expect("analyzes");
        let mut rt = RunTime::new();
        crate::run_with_interpreter_setup(&mut rt, &program);
        for stmt in &program.stmts { rt.run_semantic_stmt(stmt).expect("executes"); }
        match rt.get_var("answer", 0).unwrap() {
            Value::Str(offset, len) => rt.resolve_str(offset, len).to_owned(),
            other => panic!("expected a string, got {other:?}"),
        }
    }

    #[test]
    fn string_argument_binding_preserves_caller_names_and_parameter_order() {
        assert_eq!(source_answer("fnc: str echo(s: str) { s }\nfnc: str run() { x: t64 = 7; echo(\"{x}\") }\nanswer: str = run()\n"), "7");
        assert_eq!(source_answer("fnc: str echo(x: t64, s: str) { s }\nfnc: str run() { x: t64 = 7; echo(8, \"{x}\") }\nanswer: str = run()\n"), "8");
        assert_eq!(source_answer("count: t64 = 7\nfnc: t64 bump() { count = 9; 0 }\nfnc: str echo(s: str, unused: t64) { s }\nanswer: str = echo(\"{count}\", bump())\n"), "9");
        assert_eq!(source_answer("struct Box { value: t64 }\nbox_methods: impl (b: Box) { fnc: str echo(s: str) { s } }\nfnc: str run() { x: t64 = 7; b: Box = Box { value: 0 }; b.echo(\"{x}\") }\nanswer: str = run()\n"), "7");
    }

    #[test]
    fn cold_names_use_definition_scope_and_restore_the_callers_scope() {
        let mut rt = RunTime::new();
        rt.set_var_typed(BindingId(1), "count".into(), Type::T64, Value::Num(10), 0).unwrap();
        rt.set_var_typed(BindingId(2), "count".into(), Type::T64, Value::Num(100), 0).unwrap();
        let left = Arc::new(HashMap::from([("count".into(), BindingId(1))]));
        let right = Arc::new(HashMap::from([("count".into(), BindingId(2))]));
        rt.lexical_names = Some(right.clone());
        rt.push_function_scope(right);
        rt.set_var_typed(BindingId(3), "count".into(), Type::T64, Value::Num(9000), 0).unwrap();
        rt.push_function_scope(left);
        assert_eq!(rt.get_var("count", 0).unwrap(), Value::Num(10));
        rt.set_var("count".into(), Value::Num(11), 0).unwrap();
        rt.pop_scope();
        assert_eq!(rt.get_var("count", 0).unwrap(), Value::Num(9000));
        rt.pop_scope();
        assert_eq!(rt.get_var("count", 0).unwrap(), Value::Num(100));
        assert_eq!(rt.get_var_by_id(BindingId(1), "count", 0).unwrap(), Value::Num(11));
    }

    #[test]
    fn absent_lexical_name_cannot_leak_from_a_caller_or_other_module() {
        let mut rt = RunTime::new();
        rt.set_var_typed(BindingId(1), "secret".into(), Type::T64, Value::Num(42), 0).unwrap();
        rt.push_function_scope(Arc::new(HashMap::new()));
        assert!(rt.get_var("secret", 0).is_err());
        rt.pop_scope();
        // Identical IDs still reject a repeated declaration in one frame.
        assert!(rt.declare(BindingId(1), "secret".into(), None, 0).is_err());
    }
}

/// Whether a variable was declared with a type parameter — `y: T = x` inside a
/// generic function — and `value` says what that parameter is here.
///
/// The interpreter runs a generic function as it is written, not a copy per
/// type, so at run time `T` names no type a value could be checked against,
/// and every such declaration failed with "expected 'typeparam(\"t\")'"
/// (known-issues §72 i, §74). The variable takes the type of the value it is
/// given instead, exactly as a parameter declared `x: T` and a variable
/// declared without a type already do — so a later assignment of another
/// kind of value is still refused. A value that carries no type of its own
/// (an unknown) decides nothing, and the next one may.
fn declared_with_type_parameter(declared: Option<&Type>, value: &Value) -> bool {
    matches!(declared, Some(Type::TypeParam(_))) && type_of_value(value) != Type::Unknown
}

#[cfg(test)]
mod type_parameter_tests {
    use super::*;

    /// A declaration typed with a type parameter takes the type of a value
    /// that has one, and is left open by a value that has none.
    #[test]
    fn a_type_parameter_declaration_takes_the_type_of_its_value() {
        let t = Type::TypeParam("t".into());
        assert!(declared_with_type_parameter(Some(&t), &Value::Num(5)));
        assert!(declared_with_type_parameter(Some(&t), &Value::Bool(true)));
        assert!(declared_with_type_parameter(Some(&t), &Value::Array(Vec::new())));
        // An unknown carries no type: the declaration stays open for the next value.
        assert!(!declared_with_type_parameter(Some(&t), &Value::Unknown(Type::Unknown)));
        // A concrete declaration is not touched, nor is a variable with no declared type.
        assert!(!declared_with_type_parameter(Some(&Type::T64), &Value::Num(5)));
        assert!(!declared_with_type_parameter(None, &Value::Num(5)));
    }
}


#[cfg(test)]
mod lent_receiver_tests {
    use super::*;
    use std::collections::HashMap;

    /// Run a program; the first run-time error, if any.
    fn run(source: &str) -> Result<(), RuntimeError> {
        use crate::frontend::{resolver, semantic};
        let id = resolver::ModuleId(0);
        let file = resolver::ResolvedFile { id, path: "lent_test.cx".into(),
            program: resolver::parse_source(source).unwrap_or_else(|error| panic!("{error}: {source}")), imports: vec![] };
        let program = semantic::analyze_resolved_program(&resolver::ResolvedProgram {
            entry: id, files: HashMap::from([(id, file)]), edges: vec![], topo_order: vec![id],
        }).expect("analyzes");
        let mut rt = RunTime::new();
        crate::run_with_interpreter_setup(&mut rt, &program);
        for stmt in &program.stmts { rt.run_semantic_stmt(stmt)?; }
        assert!(rt.lent_receivers.is_empty(), "a receiver is still lent after the program");
        Ok(())
    }

    const HEAD: &str = "struct V { x: t64 }\ntop: V = V { x: 1 }\nfnc: t64 bump(v.copy: V) {\n    v.x += 5\n    return v.x\n}\nim: impl (s: V) {\n    fnc: t64 inc() {\n        s.x += 1\n        return s.x\n    }\n}\n";

    /// While a method runs, its receiver's variable is reached only through
    /// the receiver (amendment V5): every way of reaching it by its own name
    /// stops the program, naming the variable.
    #[test]
    fn a_receivers_variable_is_not_reached_by_name_while_its_method_runs() {
        for body in [
            "        return top.x",                      // a field read
            "        top.x = 5\n        return 1",       // a field write
            "        t: V = top\n        return t.x",    // the whole variable read
            "        top = V { x: 5 }\n        return 1", // the whole variable assigned
            "        return bump(top.copy)",              // passed as `.copy`
            "        return top.inc()",                   // bound as a receiver again
            "        print(\"inside {top}\")\n        return 1", // named in a string
        ] {
            let src = format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m() {{\n{body}\n    }}\n}}\nr: t64 = top.m()\n");
            match run(&src) {
                Err(RuntimeError::ReceiverInUse { name, pos }) => {
                    assert_eq!(name, "top", "{src}");
                    assert_ne!(pos, 0, "the error should carry its statement's position: {src}");
                }
                other => panic!("expected ReceiverInUse for {src:?}, got {other:?}"),
            }
        }
        // In a function the method calls, and for a further receiver.
        let through_a_call = format!("{HEAD}fnc: t64 poke() {{\n    top.x += 1000\n    return 1\n}}\nvm: impl (s: V) {{\n    fnc: t64 m() {{\n        return poke()\n    }}\n}}\nr: t64 = top.m()\n");
        assert!(matches!(run(&through_a_call), Err(RuntimeError::ReceiverInUse { .. })));
        let further = format!("{HEAD}other: V = V {{ x: 2 }}\nvm: impl (a: V, b: V) {{\n    fnc: t64 m() {{\n        return other.x\n    }}\n}}\nr: t64 = top.m(other)\n");
        assert!(matches!(run(&further), Err(RuntimeError::ReceiverInUse { .. })));
    }

    /// Through the receiver is the ordinary case, whatever the receiver is
    /// called; a path that is not taken is not an error; and a recursive
    /// function's locals are one variable per call.
    #[test]
    fn a_receivers_variable_is_reached_through_the_receiver() {
        for src in [
            // The receiver has the name of its variable.
            format!("{HEAD}vm: impl (top: V) {{\n    fnc: t64 m() {{\n        top.x += 10\n        return top.inc() + bump(top.copy)\n    }}\n}}\nr: t64 = top.m()\n"),
            // The path that names the variable is not taken.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m(c: bool) {{\n        if c {{ return top.x }}\n        return s.x\n    }}\n}}\nr: t64 = top.m(false)\n"),
            // An argument reads the variable: the method has not started.
            format!("{HEAD}vm: impl (s: V) {{\n    fnc: t64 m(k: t64) {{\n        return s.x + k\n    }}\n}}\nr: t64 = top.m(top.x)\n"),
            // One local per call of a recursive function.
            format!("{HEAD}dm: impl (s: V) {{\n    fnc: t64 depth(n: t64) {{\n        if n > 0 {{ return walk(n - 1) + s.x }}\n        return s.x\n    }}\n}}\nfnc: t64 walk(n: t64) {{\n    v: V = V {{ x: n }}\n    return v.depth(n)\n}}\nr: t64 = walk(3)\n"),
        ] {
            if let Err(error) = run(&src) {
                panic!("{src:?} failed: {error:?}");
            }
        }
    }
}
