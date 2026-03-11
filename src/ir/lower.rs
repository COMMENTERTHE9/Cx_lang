#![allow(dead_code)]

use crate::frontend::semantic_types::SemanticProgram;
use crate::ir::types::IrModule;

pub fn lower_program(_program: &SemanticProgram) -> IrModule {
    IrModule {
        debug_name: "cxir_v0".into(),
        functions: vec![],
    }
}
