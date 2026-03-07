#![allow(dead_code)]

use crate::frontend::ast;

pub struct IrModule {
    pub debug_name: String,
}

pub fn lower_program(_program: &ast::Program) -> IrModule {
    IrModule {
        debug_name: "cxir_v0".into(),
    }
}
