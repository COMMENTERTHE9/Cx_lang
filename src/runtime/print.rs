use super::runtime::*;
use crate::frontend::types::*;

/// A `{...}` segment is a valid bare-variable reference iff its trimmed content
/// is a non-empty identifier: first char alphabetic or `_`, the rest
/// alphanumeric or `_`. Anything else (`fib(i)`, `a + b`, `x.y`, empty) is a
/// non-variable expression that interpolation does not yet support (#038).
fn is_bare_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

impl RunTime {
    // Tracker #038: an unresolved `{...}` is now an error (`pos` points at the
    // print call) instead of silently re-emitting the literal text (audit F2).
    fn expand_interpolation(&self, s: &str, pos: usize) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '{' {
                let mut var_name = String::new();
                let mut closed = false;
                for inner in chars.by_ref() {
                    if inner == '}' {
                        closed = true;
                        break;
                    }
                    var_name.push(inner);
                }
                if closed {
                    let trimmed = var_name.trim();
                    let is_identifier = is_bare_identifier(trimmed);
                    let val = if is_identifier {
                        self.scopes.iter().rev()
                            .find_map(|frame| frame.get_by_name(trimmed))
                            .and_then(|entry| entry.val.clone())
                    } else {
                        None
                    };
                    match val {
                        Some(v) => result.push_str(&value_to_string(self, v)),
                        // Not a bound variable: a mistyped/undefined name or a
                        // non-variable expression. Either way, error instead of
                        // lying with literal `{...}` output.
                        None => return Err(RuntimeError::BadInterpolation {
                            pos,
                            content: var_name.clone(),
                            is_identifier,
                        }),
                    }
                } else {
                    // No closing `}` — a literal brace, not an interpolation.
                    result.push('{');
                    result.push_str(&var_name);
                }
            } else {
                result.push(c);
            }
        }
        Ok(result)
    }

    pub(crate) fn print_value(&self, val: &Value, pos: usize) -> Result<(), RuntimeError> {
        match val {
            Value::Str(off, len) => {
                let s = self.resolve_str(*off, *len);
                println!("{}", self.expand_interpolation(s, pos)?);
            }
            _ => println!("{}", value_to_string(self, val.clone())),
        }
        Ok(())
    }

    pub(crate) fn print_value_inline(&self, val: &Value, pos: usize) -> Result<(), RuntimeError> {
        match val {
            Value::Str(off, len) => {
                let s = self.resolve_str(*off, *len);
                print!("{}", self.expand_interpolation(s, pos)?);
            }
            _ => print!("{}", value_to_string(self, val.clone())),
        }
        Ok(())
    }
}
