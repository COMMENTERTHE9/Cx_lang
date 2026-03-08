#![allow(dead_code)]

use crate::frontend::semantic_types::SemanticProgram;

pub struct IrModule {
    pub debug_name: String,
}

pub fn lower_program(_program: &SemanticProgram) -> IrModule {
    IrModule {
        debug_name: "cxir_v0".into(),
    }
}
