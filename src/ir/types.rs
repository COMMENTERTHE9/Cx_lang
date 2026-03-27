use crate::ir::instr::{IrInst, IrTerminator};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrType {
    I8,
    I16,
    I32,
    I64,
    I128,
    F64,
    Bool,
}

impl IrType {
    pub fn size_bytes(&self) -> usize {
        match self {
            IrType::I8 => 1,
            IrType::I16 => 2,
            IrType::I32 => 4,
            IrType::I64 => 8,
            IrType::I128 => 16,
            IrType::F64 => 8,
            IrType::Bool => 1,
        }
    }

    pub fn align_bytes(&self) -> usize {
        match self {
            IrType::I8 => 1,
            IrType::I16 => 2,
            IrType::I32 => 4,
            IrType::I64 => 8,
            IrType::I128 => 16,
            IrType::F64 => 8,
            IrType::Bool => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ValueId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub struct IrModule {
    pub debug_name: String,
    pub functions: Vec<IrFunction>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<IrParam>,
    pub return_ty: Option<IrType>,
    pub blocks: Vec<IrBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrParam {
    pub name: String,
    pub ty: IrType,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrBlock {
    pub id: BlockId,
    pub params: Vec<BlockParam>,
    pub insts: Vec<IrInst>,
    pub term: IrTerminator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockParam {
    pub value: ValueId,
    pub ty: IrType,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::instr::IrTerminator;

    #[test]
    fn ir_module_can_hold_one_function() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        };

        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].name, "f");
    }

    #[test]
    fn block_params_are_representable() {
        let block = IrBlock {
            id: BlockId(7),
            params: vec![BlockParam {
                value: ValueId(3),
                ty: IrType::I64,
            }],
            insts: vec![],
            term: IrTerminator::Return { value: None },
        };

        assert_eq!(block.params.len(), 1);
        assert_eq!(block.params[0].value, ValueId(3));
        assert_eq!(block.params[0].ty, IrType::I64);
    }

    #[test]
    fn scalar_layout_size_i8() {
        assert_eq!(IrType::I8.size_bytes(), 1);
        assert_eq!(IrType::I8.align_bytes(), 1);
    }

    #[test]
    fn scalar_layout_size_i16() {
        assert_eq!(IrType::I16.size_bytes(), 2);
        assert_eq!(IrType::I16.align_bytes(), 2);
    }

    #[test]
    fn scalar_layout_size_i32() {
        assert_eq!(IrType::I32.size_bytes(), 4);
        assert_eq!(IrType::I32.align_bytes(), 4);
    }

    #[test]
    fn scalar_layout_size_i64() {
        assert_eq!(IrType::I64.size_bytes(), 8);
        assert_eq!(IrType::I64.align_bytes(), 8);
    }

    #[test]
    fn scalar_layout_size_i128() {
        assert_eq!(IrType::I128.size_bytes(), 16);
        assert_eq!(IrType::I128.align_bytes(), 16);
    }

    #[test]
    fn scalar_layout_size_f64() {
        assert_eq!(IrType::F64.size_bytes(), 8);
        assert_eq!(IrType::F64.align_bytes(), 8);
    }

    #[test]
    fn scalar_layout_size_bool() {
        assert_eq!(IrType::Bool.size_bytes(), 1);
        assert_eq!(IrType::Bool.align_bytes(), 1);
    }
}
