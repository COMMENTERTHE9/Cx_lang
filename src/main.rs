mod frontend;
mod runtime;
mod backend;
mod ir;

pub use runtime::arena::Arena;

use frontend::ast::*;
use frontend::diagnostics;
use frontend::lexer::*;
use frontend::parser;
use frontend::semantic;
use runtime::runtime::*;
use backend::Backend;

use chumsky::input::{Input, Stream};
use chumsky::prelude::SimpleSpan;
use chumsky::Parser;
use colored::Colorize;
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
            return;
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
            return;
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
    let semantic_errors = semantic::analyze_program(&program);
    if let Some(t) = sem_timer {
        t.finish(&format!("{} errors", semantic_errors.len()));
    }
    if !semantic_errors.is_empty() {
        for e in &semantic_errors {
            diagnostics::print_custom(&input, &e.msg, e.pos);
        }
        diagnostics::print_summary(semantic_errors.len());
        return;
    }

    match backend_kind {
        backend::BackendKind::Interpret => run_with_interpreter(program, &input, &flags),
        backend::BackendKind::Cranelift => {
            let b = backend::cranelift::CraneliftBackend;
            if let Err(msg) = b.execute(&program) {
                eprintln!("{}", msg);
            }
        }
        backend::BackendKind::Llvm => {
            let b = backend::llvm::LlvmBackend;
            if let Err(msg) = b.execute(&program) {
                eprintln!("{}", msg);
            }
        }
    }
}

fn run_with_interpreter(program: Program, input: &str, flags: &DebugFlags) {
    // RUNTIME PHASE
    let rt_timer = flags.phase.then(|| PhaseTimer::start("RUNTIME"));
    let mut rt = RunTime::new();
    rt.debug_scope = flags.scope;
    let mut step_count = 0;

    for stmt in program.stmts {
        if flags.step {
            eprintln!("{}", format!("\n[STEP {}]", step_count + 1).cyan().bold());
            diagnostics::print_stmt_summary(&stmt);
            wait_for_step();
        }
        if let Err(err) = run_stmt(&mut rt, stmt) {
            diagnostics::print_runtime(&input, &err);
            diagnostics::print_summary(1);
            break;
        }
        step_count += 1;
    }
    if let Some(t) = rt_timer {
        t.finish(&format!("{} steps", step_count));
    }
}

fn parse_program_with_fallback(tok_list: &[Tok], src: &str, _debug: bool) -> Result<Program, ParseError> {
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

