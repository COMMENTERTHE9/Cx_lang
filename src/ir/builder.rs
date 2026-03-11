use crate::ir::instr::{IrInst, IrTerminator};
use crate::ir::types::{
    BlockId, BlockParam, IrBlock, IrFunction, IrModule, IrParam, IrType, ValueId,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrBuildError {
    TerminatorAlreadySet,
    MissingTerminator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IrBlockBuilder {
    id: BlockId,
    params: Vec<BlockParam>,
    insts: Vec<IrInst>,
    term: Option<IrTerminator>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IrBuilder {
    next_value: u32,
    next_block: u32,
}

impl IrBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    pub fn fresh_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        id
    }

    pub fn module(&self, debug_name: impl Into<String>) -> IrModule {
        IrModule {
            debug_name: debug_name.into(),
            functions: Vec::new(),
        }
    }

    pub fn function(
        &self,
        name: impl Into<String>,
        params: Vec<IrParam>,
        return_ty: Option<IrType>,
    ) -> IrFunction {
        IrFunction {
            name: name.into(),
            params,
            return_ty,
            blocks: Vec::new(),
        }
    }

    pub fn block(&mut self, params: Vec<BlockParam>) -> IrBlockBuilder {
        IrBlockBuilder {
            id: self.fresh_block(),
            params,
            insts: Vec::new(),
            term: None,
        }
    }

    pub fn append_block(&self, function: &mut IrFunction, block: IrBlock) {
        function.blocks.push(block);
    }

    pub fn append_function(&self, module: &mut IrModule, function: IrFunction) {
        module.functions.push(function);
    }
}

impl IrBlockBuilder {
    pub fn id(&self) -> BlockId {
        self.id
    }

    pub fn append_inst(&mut self, inst: IrInst) {
        self.insts.push(inst);
    }

    pub fn set_terminator(&mut self, term: IrTerminator) -> Result<(), IrBuildError> {
        if self.term.is_some() {
            return Err(IrBuildError::TerminatorAlreadySet);
        }
        self.term = Some(term);
        Ok(())
    }

    pub fn finish(self) -> Result<IrBlock, IrBuildError> {
        let term = self.term.ok_or(IrBuildError::MissingTerminator)?;
        Ok(IrBlock {
            id: self.id,
            params: self.params,
            insts: self.insts,
            term,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::instr::{BinaryOp, IrTerminator};

    #[test]
    fn builder_allocates_unique_value_ids_and_block_ids() {
        let mut builder = IrBuilder::new();

        assert_ne!(builder.fresh_value(), builder.fresh_value());
        assert_ne!(builder.fresh_block(), builder.fresh_block());
    }

    #[test]
    fn builder_can_construct_function_with_one_block() {
        let mut builder = IrBuilder::new();
        let mut module = builder.module("m");
        let mut function = builder.function(
            "f",
            vec![IrParam {
                name: "x".to_string(),
                ty: IrType::I64,
            }],
            Some(IrType::I64),
        );
        let block_param = BlockParam {
            value: builder.fresh_value(),
            ty: IrType::I64,
        };
        let mut block = builder.block(vec![block_param.clone()]);
        let dst = builder.fresh_value();

        block.append_inst(IrInst::Binary {
            dst,
            op: BinaryOp::Add,
            ty: IrType::I64,
            lhs: block_param.value,
            rhs: block_param.value,
        });
        block
            .set_terminator(IrTerminator::Return { value: Some(dst) })
            .expect("terminator should be set once");

        builder.append_block(
            &mut function,
            block.finish().expect("block should finish with terminator"),
        );
        builder.append_function(&mut module, function);

        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].blocks.len(), 1);
        assert_eq!(module.functions[0].blocks[0].params, vec![block_param]);
    }
}
