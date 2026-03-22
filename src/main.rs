mod backend;
mod frontend;
mod ir;
mod runtime;

pub use runtime::arena::Arena;

use backend::Backend;
use frontend::ast::*;
use frontend::diagnostics;
use frontend::lexer::*;
use frontend::parser;
use frontend::semantic;
use frontend::semantic_types::SemanticProgram;
use runtime::runtime::*;

use chumsky::input::{Input, Stream};
use chumsky::prelude::SimpleSpan;
use chumsky::Parser;
use colored::Colorize;
use std::collections::HashMap;
use std::sync::Arc;
use std::env;
use std::fs;
use std::time::Instant;

#[derive(Debug, Default)]
struct DebugFlags {
    tokens: bool,
    ast: bool,
    scope: bool,
    step: bool,
    phase: bool,
}

impl DebugFlags {
    fn from_args(args: &[String]) -> Self {
        let all = args.contains(&"--debug".to_string());
        Self {
            tokens: all || args.contains(&"--debug-tokens".to_string()),
            ast: all || args.contains(&"--debug-ast".to_string()),
            scope: all || args.contains(&"--debug-scope".to_string()),
            step: all || args.contains(&"--debug-step".to_string()),
            phase: all || args.contains(&"--debug-phase".to_string()),
        }
    }
}

struct PhaseTimer {
    label: &'static str,
    start: Instant,
}

impl PhaseTimer {
    fn start(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
        }
    }

    fn finish(self, detail: &str) {
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "{}",
            format!("[{:<10}] {:<30} {:.2}ms", self.label, detail, ms)
                .cyan()
                .dimmed()
        );
    }
}

fn wait_for_step() {
    use std::io::Write;
    eprint!("{}", "  [step] press enter to continue...".dimmed());
    std::io::stderr().flush().ok();

    #[cfg(unix)]
    {
        use std::io::BufRead;
        let tty = std::fs::File::open("/dev/tty").unwrap();
        let mut reader = std::io::BufReader::new(tty);
        let mut line = String::new();
        reader.read_line(&mut line).ok();
    }
    #[cfg(windows)]
    {
        use std::io::BufRead;
        let tty = std::fs::File::open("CONIN$").unwrap();
        let mut reader = std::io::BufReader::new(tty);
        let mut line = String::new();
        reader.read_line(&mut line).ok();
    }
}

// Simple driver that tokenizes, parses, and executes a .cx file.
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let flags = DebugFlags::from_args(&args);
    let backend_kind = backend::parse_backend_flag(&args);
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "src/tests/test.cx".to_string());

    let input = fs::read_to_string(&path).expect("failed to read .cx file");

    // LEXER PHASE
    let lex_timer = flags.phase.then(|| PhaseTimer::start("LEXER"));
    let tok_list = match tok_collector(&input) {
        Ok(t) => t,
        Err(e) => {
            diagnostics::print_parse(&input, &e);
            std::process::exit(1);
        }
    };
    if let Some(t) = lex_timer {
        t.finish(&format!("{} tokens", tok_list.len()));
    }
    if flags.tokens {
        let pairs: Vec<_> = tok_list
            .iter()
            .map(|t| (t.kind.clone(), t.span.clone()))
            .collect();
        diagnostics::print_token_table(&pairs, &input);
    }

    // PARSER PHASE
    let parse_timer = flags.phase.then(|| PhaseTimer::start("PARSER"));
    let program = match parse_program_with_fallback(&tok_list, &input, false) {
        Ok(p) => p,
        Err(e) => {
            diagnostics::print_parse(&input, &e);
            std::process::exit(1);
        }
    };
    if let Some(t) = parse_timer {
        t.finish(&format!("{} statements", program.stmts.len()));
    }
    if flags.ast {
        diagnostics::print_ast(&program);
    }

    // SEMANTIC PHASE
    let sem_timer = flags.phase.then(|| PhaseTimer::start("SEMANTIC"));
    let semantic_program = match semantic::analyze_program(&program) {
        Ok(program) => program,
        Err(errors) => {
            if let Some(t) = sem_timer {
                t.finish(&format!("{} errors", errors.len()));
            }
            for e in &errors {
                diagnostics::print_custom(&input, &e.msg, e.pos);
            }
            diagnostics::print_summary(errors.len());
            std::process::exit(1);
        }
    };
    if let Some(t) = sem_timer {
        t.finish("0 errors");
    }

    match backend_kind {
        backend::BackendKind::Interpret => run_with_interpreter(semantic_program, &input, &flags),
        backend::BackendKind::Cranelift => {
            let ir = match prepare_ir(&semantic_program) {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("{}", err);
                    return;
                }
            };
            println!("{}", crate::ir::printer::print_module(&ir));
            let b = backend::cranelift::CraneliftBackend;
            if let Err(msg) = b.execute(&program) {
                eprintln!("{}", msg);
            }
        }
        backend::BackendKind::Llvm => {
            let _ir = match prepare_ir(&semantic_program) {
                Ok(ir) => ir,
                Err(err) => {
                    eprintln!("{}", err);
                    return;
                }
            };
            let b = backend::llvm::LlvmBackend;
            if let Err(msg) = b.execute(&program) {
                eprintln!("{}", msg);
            }
        }
    }
}

fn run_with_interpreter(program: SemanticProgram, input: &str, flags: &DebugFlags) {
    use frontend::semantic_types::{SemanticStmt, SemanticType};

    let rt_timer = flags.phase.then(|| PhaseTimer::start("RUNTIME"));
    let mut rt = RunTime::new();
    rt.debug_scope = flags.scope;

    // Single pre-pass — register structs, funcs, and impls from semantic program
    for stmt in &program.stmts {
        match stmt {
            SemanticStmt::StructDef { name, fields, .. } => {
                rt.structs.insert(name.clone(), fields.iter().map(|(n, t)| (n.clone(), t.clone().into())).collect());
            }
            SemanticStmt::FuncDef(sem_func) => {
                rt.register_semantic_func(sem_func.clone());
            }
            SemanticStmt::EnumDef { name, variants, .. } => {
                rt.enums.insert(name.clone(), EnumRuntimeInfo {
                    variants: variants.clone(),
                    groups: HashMap::new(),
                    super_group_order: HashMap::new(),
                });
            }
            SemanticStmt::ImplBlock { aliases, methods, .. } => {
                for sem_func in methods {
                    for (_, alias_type) in aliases {
                        let type_key = match alias_type {
                            SemanticType::Struct(n) => n.clone(),
                            _ => continue,
                        };
                        rt.semantic_impls.insert(
                            (type_key, sem_func.name.clone()),
                            (aliases.clone(), Arc::new(sem_func.clone())),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // Main execution loop — runs off SemanticProgram
    let mut step_count = 0;
    for stmt in &program.stmts {
        if let Err(err) = rt.run_semantic_stmt(stmt) {
            diagnostics::print_runtime(input, &err);
            diagnostics::print_summary(1);
            std::process::exit(1);
        }
        step_count += 1;
    }
    if let Some(t) = rt_timer {
        t.finish(&format!("{} steps", step_count));
    }
}

fn parse_program_with_fallback(
    tok_list: &[Tok],
    src: &str,
    _debug: bool,
) -> Result<Program, ParseError> {
    match parse_program_chumsky(tok_list, src) {
        Ok(program) => Ok(program),
        Err(chumsky_errs) => Err(chumsky_errs.into_iter().next().unwrap_or(ParseError {
            msg: diagnostics::ERR_FAILED_STATEMENT.to_string(),
            pos: src.len(),
        })),
    }
}

fn parse_program_chumsky(tok_list: &[Tok], src: &str) -> Result<Program, Vec<ParseError>> {
    let token_iter = tok_list
        .iter()
        .map(|t| (t.kind.clone(), (t.span.start..t.span.end).into()));

    let eoi: SimpleSpan = (src.len()..src.len()).into();
    let input = Stream::from_iter(token_iter).map(eoi, |(token, span): (_, _)| (token, span));

    match parser::program_parser().parse(input).into_result() {
        Ok(program) => Ok(program),
        Err(errs) => {
            let mapped = errs
                .into_iter()
                .map(|e| ParseError {
                    msg: format!("{:?}", e.reason()),
                    pos: e.span().start,
                })
                .collect::<Vec<ParseError>>();
            Err(mapped)
        }
    }
}

fn prepare_ir(
    semantic_program: &SemanticProgram,
) -> Result<crate::ir::IrModule, crate::ir::lower::LoweringError> {
    backend::lower_to_ir(semantic_program)
}
