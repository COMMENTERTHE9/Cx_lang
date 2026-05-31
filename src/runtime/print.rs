use super::runtime::*;
use crate::frontend::types::*;

impl RunTime {
    fn expand_interpolation(&self, s: &str) -> String {
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
                if closed && !var_name.is_empty() {
                    let val = self.scopes.iter().rev()
                        .find_map(|frame| frame.get_by_name(&var_name))
                        .and_then(|entry| entry.val.clone());
                    match val {
                        Some(v) => result.push_str(&value_to_string(self, v)),
                        None => {
                            result.push('{');
                            result.push_str(&var_name);
                            result.push('}');
                        }
                    }
                } else if !closed {
                    result.push('{');
                    result.push_str(&var_name);
                }
            } else {
                result.push(c);
            }
        }
        result
    }

    pub(crate) fn print_value(&self, val: &Value) {
        match val {
            Value::Str(off, len) => {
                let s = self.resolve_str(*off, *len);
                println!("{}", self.expand_interpolation(s));
            }
            _ => println!("{}", value_to_string(self, val.clone())),
        }
    }

    pub(crate) fn print_value_inline(&self, val: &Value) {
        match val {
            Value::Str(off, len) => {
                let s = self.resolve_str(*off, *len);
                print!("{}", self.expand_interpolation(s));
            }
            _ => print!("{}", value_to_string(self, val.clone())),
        }
    }
}
