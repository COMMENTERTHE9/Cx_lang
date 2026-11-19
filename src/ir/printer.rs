use std::fmt::Write;

use crate::ir::instr::{BinaryOp, CompareOp, IrInst, IrTerminator};
use crate::ir::types::{BlockId, IrBlock, IrData, IrFunction, IrModule, IrType, ValueId, DATA_ADDR_SIZE};

pub fn print_module(module: &IrModule) -> String {
    let mut out = String::new();
    writeln!(out, "module {}", module.debug_name).unwrap();
    for global in &module.globals {
        writeln!(out, "global @g{} {} size {} align {}", global.id.0, global.name, global.size, global.align).unwrap();
    }
    for data in &module.data {
        writeln!(out, "{}", print_data(data)).unwrap();
    }
    for (i, function) in module.functions.iter().enumerate() {
        if i > 0 {
            writeln!(out).unwrap();
        }
        write!(out, "{}", print_function(function)).unwrap();
    }
    out
}

/// One line per data object. Nothing in it depends on where the object will
/// live, so printing a module twice gives the same text.
///
/// - contents only, valid UTF-8: `data @d0 align 1 bytes "hello\n"`
/// - contents only, otherwise:   `data @d0 align 1 hex ff 00 7f`
/// - with address slots:         `data @d1 align 8 { addr @d0, i64 5 }` — the
///   object read as 8-byte words, `addr @dN` where a relocation sits.
pub fn print_data(data: &IrData) -> String {
    let head = format!("data @d{} align {}", data.id.0, data.align);
    if data.relocs.is_empty() {
        return match std::str::from_utf8(&data.bytes) {
            Ok(text) => format!("{head} bytes \"{}\"", text.escape_default()),
            Err(_) => {
                let hex: Vec<String> = data.bytes.iter().map(|b| format!("{b:02x}")).collect();
                format!("{head} hex {}", hex.join(" "))
            }
        };
    }
    let mut words = Vec::new();
    for (index, chunk) in data.bytes.chunks(DATA_ADDR_SIZE).enumerate() {
        let offset = index * DATA_ADDR_SIZE;
        if let Some(reloc) = data.relocs.iter().find(|r| r.offset == offset) {
            words.push(format!("addr @d{}", reloc.target.0));
        } else if chunk.len() == DATA_ADDR_SIZE {
            let mut word = [0u8; DATA_ADDR_SIZE];
            word.copy_from_slice(chunk);
            words.push(format!("i64 {}", i64::from_le_bytes(word)));
        } else {
            // A tail shorter than a word. Lowering does not produce one in an
            // object with relocations, and the validator does not reject it;
            // print it rather than hide it.
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            words.push(format!("hex {}", hex.join(" ")));
        }
    }
    format!("{head} {{ {} }}", words.join(", "))
}

pub fn print_function(function: &IrFunction) -> String {
    let mut out = String::new();
    let params: Vec<String> = function
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, print_type(&p.ty)))
        .collect();
    write!(out, "fn {}({})", function.name, params.join(", ")).unwrap();
    if let Some(ref ty) = function.return_ty {
        write!(out, " -> {}", print_type(ty)).unwrap();
    }
    writeln!(out, " {{").unwrap();
    for block in &function.blocks {
        write!(out, "{}", print_block(block)).unwrap();
    }
    writeln!(out, "}}").unwrap();
    out
}

fn print_block(block: &IrBlock) -> String {
    let mut out = String::new();
    write!(out, "  {}:", print_block_id(block.id)).unwrap();
    if !block.params.is_empty() {
        let params: Vec<String> = block
            .params
            .iter()
            .map(|p| format!("{}: {}", print_value_id(p.value), print_type(&p.ty)))
            .collect();
        write!(out, "({})", params.join(", ")).unwrap();
    }
    writeln!(out).unwrap();
    for inst in &block.insts {
        writeln!(out, "    {}", print_inst(inst)).unwrap();
    }
    writeln!(out, "    {}", print_terminator(&block.term)).unwrap();
    out
}

pub fn print_inst(inst: &IrInst) -> String {
    match inst {
        IrInst::GlobalAddr { dst, global } => format!("{} = global_addr @g{}", print_value_id(*dst), global.0),
        IrInst::DataAddr { dst, data } => format!("{} = data_addr @d{}", print_value_id(*dst), data.0),
        IrInst::ConstInt { dst, ty, value } => {
            format!("{} = const {} {}", print_value_id(*dst), print_type(ty), value)
        }
        IrInst::ConstFloat { dst, value } => {
            format!("{} = const f64 {}", print_value_id(*dst), value)
        }
        IrInst::SsaBind { dst, ty, src } => {
            format!(
                "{} = bind {} {}",
                print_value_id(*dst),
                print_type(ty),
                print_value_id(*src)
            )
        }
        IrInst::Binary { dst, op, ty, lhs, rhs } => {
            format!(
                "{} = {} {} {}, {}",
                print_value_id(*dst),
                print_binary_op(op),
                print_type(ty),
                print_value_id(*lhs),
                print_value_id(*rhs)
            )
        }
        IrInst::Compare { dst, op, lhs, rhs } => {
            format!(
                "{} = {} {}, {}",
                print_value_id(*dst),
                print_compare_op(op),
                print_value_id(*lhs),
                print_value_id(*rhs)
            )
        }
        IrInst::Call { dst, callee, args, return_ty } => {
            let args_str: Vec<String> = args.iter().map(|a| print_value_id(*a)).collect();
            let call_str = match return_ty {
                Some(ty) => format!("call {}({}) -> {}", callee, args_str.join(", "), print_type(ty)),
                None => format!("call {}({})", callee, args_str.join(", ")),
            };
            match dst {
                Some(d) => format!("{} = {}", print_value_id(*d), call_str),
                None => call_str,
            }
        }
        IrInst::Cast { dst, from, to, value } => {
            format!(
                "{} = cast {} {} -> {}",
                print_value_id(*dst),
                print_type(from),
                print_value_id(*value),
                print_type(to)
            )
        }
        IrInst::Alloca { dst, size, align } => {
            format!("{} = alloca size {} align {}", print_value_id(*dst), size, align)
        }
        IrInst::ArrayAlloca { dst, element_type, count } => {
            format!(
                "{} = array_alloca {} * {}",
                print_value_id(*dst),
                print_type(element_type),
                count
            )
        }
        IrInst::PtrOffset { dst, base, offset } => {
            format!(
                "{} = ptr_offset {} + {}",
                print_value_id(*dst),
                print_value_id(*base),
                offset
            )
        }
        IrInst::PtrAdd { dst, base, offset } => {
            format!(
                "{} = ptr_add {} + {}",
                print_value_id(*dst),
                print_value_id(*base),
                print_value_id(*offset)
            )
        }
        IrInst::Load { dst, ty, ptr } => {
            format!("{} = load {} {}", print_value_id(*dst), print_type(ty), print_value_id(*ptr))
        }
        IrInst::Store { ptr, value } => {
            format!("store {} {}", print_value_id(*ptr), print_value_id(*value))
        }
    }
}

pub fn print_terminator(term: &IrTerminator) -> String {
    match term {
        IrTerminator::Jump { target, args } => {
            if args.is_empty() {
                format!("jump {}", print_block_id(*target))
            } else {
                let args_str: Vec<String> = args.iter().map(|a| print_value_id(*a)).collect();
                format!("jump {}({})", print_block_id(*target), args_str.join(", "))
            }
        }
        IrTerminator::Branch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let then_args_str = if then_args.is_empty() {
                String::new()
            } else {
                let a: Vec<String> = then_args.iter().map(|v| print_value_id(*v)).collect();
                format!("({})", a.join(", "))
            };
            let else_args_str = if else_args.is_empty() {
                String::new()
            } else {
                let a: Vec<String> = else_args.iter().map(|v| print_value_id(*v)).collect();
                format!("({})", a.join(", "))
            };
            format!(
                "branch {}, {}{}, {}{}",
                print_value_id(*cond),
                print_block_id(*then_block),
                then_args_str,
                print_block_id(*else_block),
                else_args_str
            )
        }
        IrTerminator::Return { value } => match value {
            Some(v) => format!("ret {}", print_value_id(*v)),
            None => "ret".to_string(),
        },
        IrTerminator::Trap => "trap".to_string(),
    }
}

fn print_type(ty: &IrType) -> &'static str {
    match ty {
        IrType::I8 => "i8",
        IrType::I16 => "i16",
        IrType::I32 => "i32",
        IrType::I64 => "i64",
        IrType::I128 => "i128",
        IrType::F64 => "f64",
        IrType::Bool => "bool",
        IrType::TBool => "tbool",
        IrType::Ptr => "ptr",
        IrType::Void => "void",
    }
}

fn print_value_id(v: ValueId) -> String {
    format!("v{}", v.0)
}

fn print_block_id(b: BlockId) -> String {
    format!("bb{}", b.0)
}

fn print_binary_op(op: &BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
    }
}

fn print_compare_op(op: &CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "eq",
        CompareOp::Ne => "ne",
        CompareOp::Lt => "lt",
        CompareOp::Le => "le",
        CompareOp::Gt => "gt",
        CompareOp::Ge => "ge",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::types::{BlockParam, IrParam};

    #[test]
    fn prints_simple_function() {
        let module = IrModule {
            debug_name: "test".into(),
            globals: vec![],
            data: vec![],
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                return_ty: Some(IrType::I64),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::ConstInt {
                        dst: ValueId(0),
                        ty: IrType::I64,
                        value: 42,
                    }],
                    term: IrTerminator::Return {
                        value: Some(ValueId(0)),
                    },
                }],
            }],
        };

        let output = print_module(&module);
        assert!(output.contains("module test"));
        assert!(output.contains("fn main() -> i64 {"));
        assert!(output.contains("v0 = const i64 42"));
        assert!(output.contains("ret v0"));
    }

    #[test]
    fn prints_call_instruction() {
        let module = IrModule {
            debug_name: "test".into(),
            globals: vec![],
            data: vec![],
            functions: vec![IrFunction {
                name: "caller".into(),
                params: vec![],
                return_ty: Some(IrType::I64),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::Call {
                        dst: Some(ValueId(0)),
                        callee: "get_value".into(),
                        args: vec![ValueId(1), ValueId(2)],
                        return_ty: Some(IrType::I64),
                    }],
                    term: IrTerminator::Return {
                        value: Some(ValueId(0)),
                    },
                }],
            }],
        };

        let output = print_module(&module);
        assert!(output.contains("v0 = call get_value(v1, v2) -> i64"));
    }

    #[test]
    fn prints_trap_terminator() {
        let block = IrBlock {
            id: BlockId(5),
            params: vec![],
            insts: vec![],
            term: IrTerminator::Trap,
        };
        let output = print_block(&block);
        assert!(output.contains("trap"), "expected 'trap' in output, got: {}", output);
    }

    #[test]
    fn prints_branch_with_args() {
        let block = IrBlock {
            id: BlockId(0),
            params: vec![],
            insts: vec![],
            term: IrTerminator::Branch {
                cond: ValueId(0),
                then_block: BlockId(1),
                then_args: vec![ValueId(1)],
                else_block: BlockId(2),
                else_args: vec![ValueId(2)],
            },
        };

        let output = print_block(&block);
        assert!(output.contains("branch v0, bb1(v1), bb2(v2)"));
    }

    #[test]
    fn prints_function_with_params() {
        let function = IrFunction {
            name: "add".into(),
            params: vec![
                IrParam { name: "a".into(), ty: IrType::I64 },
                IrParam { name: "b".into(), ty: IrType::I64 },
            ],
            return_ty: Some(IrType::I64),
            blocks: vec![IrBlock {
                id: BlockId(0),
                params: vec![],
                insts: vec![],
                term: IrTerminator::Return { value: None },
            }],
        };

        let output = print_function(&function);
        assert!(output.contains("fn add(a: i64, b: i64) -> i64 {"));
    }

    #[test]
    fn prints_block_params() {
        let block = IrBlock {
            id: BlockId(3),
            params: vec![
                BlockParam { value: ValueId(5), ty: IrType::I64, read_only: false },
                BlockParam { value: ValueId(6), ty: IrType::Bool, read_only: false },
            ],
            insts: vec![],
            term: IrTerminator::Return { value: None },
        };

        let output = print_block(&block);
        assert!(output.contains("bb3:(v5: i64, v6: bool)"));
    }

    fn string_module() -> IrModule {
        use crate::ir::types::{DataId, GlobalId, IrGlobal};
        IrModule {
            debug_name: "strings".into(),
            globals: vec![IrGlobal { id: GlobalId(0), name: "counter".into(), size: 8, align: 8 }],
            data: vec![
                IrData::string_bytes(DataId(0), b"he said \"hi\"\n".to_vec()),
                IrData::string_descriptor(DataId(1), DataId(0), 14),
                IrData::string_bytes(DataId(2), vec![0xff, 0x00, 0x7f]),
                IrData::string_bytes(DataId(3), Vec::new()),
                IrData::string_descriptor(DataId(4), DataId(3), 0),
            ],
            functions: vec![IrFunction {
                name: "main".into(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::DataAddr { dst: ValueId(0), data: DataId(1) }],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        }
    }

    #[test]
    fn prints_string_bytes_escaped_and_other_bytes_as_hex() {
        let printed = print_module(&string_module());
        assert!(printed.contains(r#"data @d0 align 1 bytes "he said \"hi\"\n""#), "{printed}");
        assert!(printed.contains("data @d2 align 1 hex ff 00 7f"), "{printed}");
    }

    #[test]
    fn prints_a_descriptor_as_an_address_and_a_length() {
        let printed = print_module(&string_module());
        assert!(printed.contains("data @d1 align 8 { addr @d0, i64 14 }"), "{printed}");
    }

    #[test]
    fn prints_an_empty_string() {
        let printed = print_module(&string_module());
        assert!(printed.contains("data @d3 align 1 bytes \"\"\n"), "{printed}");
        assert!(printed.contains("data @d4 align 8 { addr @d3, i64 0 }"), "{printed}");
    }

    #[test]
    fn prints_data_between_globals_and_functions_and_names_it_in_code() {
        let printed = print_module(&string_module());
        let global = printed.find("global @g0").expect("global");
        let data = printed.find("data @d0").expect("data");
        let function = printed.find("fn main").expect("function");
        assert!(global < data && data < function, "{printed}");
        assert!(printed.contains("v0 = data_addr @d1"), "{printed}");
    }

    #[test]
    fn printing_a_module_with_data_twice_gives_the_same_text() {
        let module = string_module();
        assert_eq!(print_module(&module), print_module(&module));
        // Nothing about the text depends on where the module was built.
        assert_eq!(print_module(&module), print_module(&module.clone()));
    }
}
