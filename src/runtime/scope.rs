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

    pub fn push_function_scope(&mut self) {
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
            let bleeds: Vec<(String, usize, String)> = frame
                .bleed_back
                .iter()
                .filter(|(param_name, _)| !frame.freed.contains(*param_name))
                .map(|(param_name, (outer_idx, outer_name))| {
                    (param_name.clone(), *outer_idx, outer_name.clone())
                })
                .collect();

            let bleed_values: Vec<(usize, String, Value)> = bleeds
                .iter()
                .filter_map(|(param_name, outer_idx, outer_name)| {
                    frame.get_by_name(param_name)
                        .and_then(|entry| entry.val.clone())
                        .map(|val| (*outer_idx, outer_name.clone(), val))
                })
                .collect();

            let debug_info = if self.debug_scope {
                let bleed_events: Vec<(String, Value)> = bleeds
                    .iter()
                    .filter_map(|(param_name, _, outer_name)| {
                        frame.get_by_name(param_name)
                            .and_then(|entry| entry.val.clone())
                            .map(|val| (outer_name.clone(), val))
                    })
                    .collect();
                let free_names: Vec<String> = frame.by_name.keys()
                    .filter(|name| !frame.freed.contains(*name))
                    .cloned()
                    .collect();
                let close_label = format!("scope#{}", self.scopes.len() - 1);
                Some((free_names, bleed_events, close_label))
            } else {
                None
            };

            (bleed_values, debug_info)
        };

        self.scopes.pop();

        for (outer_idx, outer_name, val) in bleed_values {
            if let Some(outer_frame) = self.scopes.get_mut(outer_idx) {
                if let Some(entry) = outer_frame.get_by_name_mut(&outer_name) {
                    entry.val = Some(val);
                }
            }
        }

        if let Some((free_names, bleed_events, close_label)) = debug_info {
            for name in &free_names {
                diagnostics::print_scope_event(&ScopeEvent::Free(name.clone()));
            }
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
        let frame = self.scopes.last_mut().unwrap();

        if frame.contains_name(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        frame.seen.insert(name.clone());
        frame.insert_var(binding, &name, VarEntry { ty, val: None });
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
        if entry.ty.is_none() {
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
        if !self.consts.is_empty() && self.consts.contains_key(name) {
            return Err(RuntimeError::BadAssignTarget { pos });
        }
        let value = self.resolve_assigned_value(value, pos)?;
        for i in (0..self.scopes.len()).rev() {
            if self.scopes[i].vars.contains_key(&binding) {
                return self.write_var_at(i, binding, name, value, pos);
            }
        }
        let was_seen = self.scopes.last().unwrap().seen.contains(name);
        Err(diagnostics::unresolved_var_error(pos, name.to_string(), was_seen))
    }

    /// Cold-path write by name (string-interp targets, while-in arrays, method
    /// write-back). Resolves name -> BindingId via the per-frame index.
    pub fn set_var(&mut self, name: String, value: Value, pos: usize) -> Result<(), RuntimeError> {
        if !self.consts.is_empty() && self.consts.contains_key(&name) {
            return Err(RuntimeError::BadAssignTarget { pos });
        }
        let value = self.resolve_assigned_value(value, pos)?;
        for i in (0..self.scopes.len()).rev() {
            if let Some(&binding) = self.scopes[i].by_name.get(&name) {
                return self.write_var_at(i, binding, &name, value, pos);
            }
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
        if !value_matches_type(&value, &ty) {
            return Err(RuntimeError::TypeMismatch {
                pos,
                expected: ty,
                got,
            });
        }

        {
            let frame = self.scopes.last_mut().unwrap();
            if frame.contains_name(&name) {
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
        for frame in self.scopes.iter_mut().rev() {
            if let Some(entry) = frame.get_by_name_mut(container) {
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
    pub fn set_array_element(
        &mut self,
        arr_name: &str,
        index: usize,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        for frame in self.scopes.iter_mut().rev() {
            if let Some(entry) = frame.get_by_name_mut(arr_name) {
                match &mut entry.val {
                    Some(Value::Array(elems)) => {
                        if index < elems.len() {
                            elems[index] = value;
                            return Ok(());
                        } else {
                            return Err(RuntimeError::IndexOutOfBounds {
                                pos,
                                index: index as i64,
                                length: elems.len(),
                            });
                        }
                    }
                    _ => {
                        return Err(RuntimeError::NotAContainer {
                            pos,
                            name: arr_name.to_string(),
                        });
                    }
                }
            }
        }
        Err(RuntimeError::UndefinedVar {
            pos,
            name: arr_name.to_string(),
        })
    }

    pub fn get_field(&self, container: &str, field: &str, pos: usize) -> Result<Value, RuntimeError> {
        for frame in self.scopes.iter().rev() {
            if let Some(entry) = frame.get_by_name(container) {
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
        for frame in self.scopes.iter().rev() {
            if let Some(entry) = frame.get_by_name(name) {
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

    /// Hot-path read by BindingId (VarRef, CompoundAssign read). `name` is used
    /// only for diagnostics. Walks frames innermost->outermost exactly like the
    /// name-based path, but matches on the BindingId — no string hashing.
    pub fn get_var_by_id(&self, binding: BindingId, name: &str, pos: usize) -> Result<Value, RuntimeError> {
        for frame in self.scopes.iter().rev() {
            if let Some(entry) = frame.vars.get(&binding) {
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
