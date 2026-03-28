use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::frontend::ast::{Program, Stmt, ImportDecl};
use crate::frontend::lexer::Token;
use crate::frontend::parser;
use chumsky::prelude::*;
use chumsky::input::Stream;

// Stable internal ID for each resolved file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(pub usize);

// Resolved import relationship — alias lives here not on the file
#[derive(Debug, Clone)]
pub struct ImportEdge {
    pub importer: ModuleId,
    pub importee: ModuleId,
    pub alias: String,
    pub pos: usize,
}

// Intrinsic file data only
#[derive(Debug)]
pub struct ResolvedFile {
    pub id: ModuleId,
    pub path: PathBuf,
    pub program: Program,
    pub imports: Vec<ImportDecl>,
}

// Full resolved dependency graph topo-sorted
#[derive(Debug)]
pub struct ResolvedProgram {
    pub entry: ModuleId,
    pub files: HashMap<ModuleId, ResolvedFile>,
    pub edges: Vec<ImportEdge>,
    pub topo_order: Vec<ModuleId>, // leaves first entry last
}

#[derive(Debug)]
pub enum ResolveError {
    FileNotFound { path: String, pos: usize },
    CircularImport { chain: Vec<PathBuf> },
    RegistryNotSupported { path: String, pos: usize },
    IoError { path: PathBuf, msg: String },
    ParseError { path: PathBuf, msg: String },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ResolveError::FileNotFound { path, .. } =>
                write!(f, "cannot resolve import {:?} — file not found", path),
            ResolveError::CircularImport { chain } => {
                let names: Vec<_> = chain.iter()
                    .map(|p| p.file_name().unwrap_or_default().to_string_lossy())
                    .collect();
                write!(f, "circular import detected: {}", names.join(" -> "))
            }
            ResolveError::RegistryNotSupported { path, .. } =>
                write!(f, "registry imports not supported in v0.1: {:?}", path),
            ResolveError::IoError { path, msg } =>
                write!(f, "IO error reading {:?}: {}", path, msg),
            ResolveError::ParseError { path, msg } =>
                write!(f, "parse error in {:?}: {}", path, msg),
        }
    }
}

struct Resolver {
    path_to_id: HashMap<PathBuf, ModuleId>,
    files: HashMap<ModuleId, ResolvedFile>,
    edges: Vec<ImportEdge>,
    next_id: usize,
    topo_order: Vec<ModuleId>,
}

impl Resolver {
    fn new() -> Self {
        Resolver {
            path_to_id: HashMap::new(),
            files: HashMap::new(),
            edges: Vec::new(),
            next_id: 0,
            topo_order: Vec::new(),
        }
    }

    fn next_module_id(&mut self) -> ModuleId {
        let id = ModuleId(self.next_id);
        self.next_id += 1;
        id
    }

    // Postorder DFS — leaves added to topo_order before parents
    fn resolve_file(
        &mut self,
        path: &Path,
        chain: &mut Vec<PathBuf>,
        importer: Option<(ModuleId, String, usize)>,
        depth: usize,
    ) -> Result<ModuleId, ResolveError> {
        if depth > 100 {
            return Err(ResolveError::IoError {
                path: path.to_path_buf(),
                msg: "import chain exceeds maximum depth of 100 — possible circular dependency".to_string(),
            });
        }
        let canonical = canonicalize_path(path)?;

        // Cycle check
        if chain.contains(&canonical) {
            let cycle: Vec<PathBuf> = chain.iter()
                .skip_while(|p| **p != canonical)
                .cloned()
                .chain(std::iter::once(canonical.clone()))
                .collect();
            return Err(ResolveError::CircularImport { chain: cycle });
        }

        // Cache check — already fully resolved
        if let Some(&id) = self.path_to_id.get(&canonical) {
            if let Some((importer_id, alias, pos)) = importer {
                self.edges.push(ImportEdge {
                    importer: importer_id,
                    importee: id,
                    alias,
                    pos,
                });
            }
            return Ok(id);
        }

        // Read and parse
        let source = std::fs::read_to_string(&canonical)
            .map_err(|e| ResolveError::IoError {
                path: canonical.clone(),
                msg: e.to_string(),
            })?;

        let program = parse_source(&source)
            .map_err(|msg| ResolveError::ParseError {
                path: canonical.clone(),
                msg,
            })?;

        let imports = extract_imports(&program);
        let id = self.next_module_id();
        self.path_to_id.insert(canonical.clone(), id);

        // Store file before recursing so cycle detection works
        self.files.insert(id, ResolvedFile {
            id,
            path: canonical.clone(),
            program,
            imports: imports.clone(),
        });

        // Record edge from importer
        if let Some((importer_id, alias, pos)) = importer {
            self.edges.push(ImportEdge {
                importer: importer_id,
                importee: id,
                alias,
                pos,
            });
        }

        // Recurse into imports before adding self to topo_order (postorder)
        chain.push(canonical.clone());
        let parent_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
        for import in &imports {
            let child_path = resolve_import_path(&import.path, &parent_dir, import.pos)?;
            self.resolve_file(
                &child_path,
                chain,
                Some((id, import.alias.clone(), import.pos)),
                depth + 1,
            )?;
        }
        chain.pop();

        // Postorder: add self AFTER all children
        self.topo_order.push(id);

        Ok(id)
    }
}

// NOTE: No sandbox boundary check — ../ paths can escape project root.
// Malicious imports will fail at parse time. Acceptable for v0.1.
fn resolve_import_path(raw: &str, parent_dir: &Path, pos: usize) -> Result<PathBuf, ResolveError> {
    if raw.starts_with("./") || raw.starts_with("../") {
        let path = parent_dir.join(raw);
        // Add .cx extension only if not already present
        let path = if path.extension().map_or(true, |e| e != "cx") {
            path.with_extension("cx")
        } else {
            path
        };
        // Check existence before canonicalize for better error message
        if !path.exists() {
            return Err(ResolveError::FileNotFound { path: raw.to_string(), pos });
        }
        Ok(path)
    } else if raw.starts_with("std/") {
        // Stdlib not bundled in v0.1
        Err(ResolveError::FileNotFound {
            path: format!("{} (stdlib not yet bundled)", raw),
            pos,
        })
    } else {
        // Registry path not supported in v0.1
        Err(ResolveError::RegistryNotSupported { path: raw.to_string(), pos })
    }
}

fn canonicalize_path(path: &Path) -> Result<PathBuf, ResolveError> {
    path.canonicalize().map_err(|e| ResolveError::IoError {
        path: path.to_path_buf(),
        msg: e.to_string(),
    })
}

fn extract_imports(program: &Program) -> Vec<ImportDecl> {
    for stmt in &program.stmts {
        if let Stmt::ImportBlock { imports, .. } = stmt {
            return imports.clone();
        }
    }
    Vec::new()
}

fn parse_source(source: &str) -> Result<Program, String> {
    use logos::Logos;
    let mut tokens = Vec::new();
    let mut lex = Token::lexer(source);
    while let Some(tok_result) = lex.next() {
        let span = lex.span();
        match tok_result {
            Ok(token) => tokens.push((token, (span.start..span.end).into())),
            Err(_) => {} // skip unknown tokens like the main lexer does
        }
    }
    let eoi: SimpleSpan = (source.len()..source.len()).into();
    let input = Stream::from_iter(tokens).map(eoi, |(token, span): (_, _)| (token, span));
    parser::program_parser()
        .parse(input)
        .into_result()
        .map_err(|errs| {
            errs.into_iter()
                .map(|_e| "parse failed — check syntax near the reported location".to_string())
                .collect::<Vec<_>>()
                .join("; ")
        })
}

// Public entry point
pub fn resolve(entry_path: &Path, entry_program: Program) -> Result<ResolvedProgram, ResolveError> {
    if !entry_path.exists() {
        return Err(ResolveError::FileNotFound {
            path: entry_path.to_string_lossy().to_string(),
            pos: 0,
        });
    }
    let mut resolver = Resolver::new();
    let canonical = canonicalize_path(entry_path)?;
    let imports = extract_imports(&entry_program);
    let entry_id = resolver.next_module_id();

    resolver.path_to_id.insert(canonical.clone(), entry_id);
    resolver.files.insert(entry_id, ResolvedFile {
        id: entry_id,
        path: canonical.clone(),
        program: entry_program,
        imports: imports.clone(),
    });

    // Resolve all imports from entry
    let mut chain = vec![canonical.clone()];
    let parent_dir = canonical.parent().unwrap_or(Path::new(".")).to_path_buf();
    for import in &imports {
        let child_path = resolve_import_path(&import.path, &parent_dir, import.pos)?;
        resolver.resolve_file(
            &child_path,
            &mut chain,
            Some((entry_id, import.alias.clone(), import.pos)),
            0,
        )?;
    }

    // Entry file added last — after all dependencies (postorder)
    resolver.topo_order.push(entry_id);

    Ok(ResolvedProgram {
        entry: entry_id,
        files: resolver.files,
        edges: resolver.edges,
        topo_order: resolver.topo_order,
    })
}
