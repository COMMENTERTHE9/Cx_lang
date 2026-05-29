use crate::frontend::diagnostics;
use logos::Logos;

fn unescape_string(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('0') => out.push('\0'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
fn skip_block_comment(lex: &mut logos::Lexer<Token>) -> logos::Skip {
    let remainder = lex.remainder();
    match remainder.find("#/") {
        Some(end) => lex.bump(end + 2),
        None => lex.bump(remainder.len()),
    }
    logos::Skip
}

// Token kinds recognized by the lexer; whitespace is skipped via a Logos attribute.
#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n\f]+")] // skip whitespace
pub enum Token {
    // ── Comments ────────────────────────────────
    #[regex(r"//[^\r\n]*", logos::skip, allow_greedy = true)]
    LineComment,

    #[token("/#", skip_block_comment)]
    BlockComment,

    // ── Keywords ────────────────────────────────
    #[token("let")]
    KeywordLet,

    #[token("fnc")]
    KeywordFnc,

    #[token("const")]
    KeywordConst,

    #[token("pub")]
    KeywordPub,

    #[token("return")]
    KeywordReturn,

    #[token("true")]
    KeywordTrue,

    #[token("false")]
    KeywordFalse,

    #[token("unknown")]
    KeywordUnknown,

    #[token("when")]
    KeywordWhen,

    #[token("while")]
    KeywordWhile,

    #[token("if")]
    KeywordIf,

    #[token("else")]
    KeywordElse,

    #[token("for")]
    KeywordFor,

    #[token("in")]
    KeywordIn,

    #[token("loop")]
    KeywordLoop,

    #[token("break")]
    KeywordBreak,

    #[token("continue")]
    KeywordContinue,

    #[token("then")]
    KeywordThen,

    #[token("enum")]
    KeywordEnum,

    #[token("group")]
    KeywordGroup,

    #[token("Handle")]
    KeywordHandle,

    #[token("struct")]
    KeywordStruct,

    #[token("impl")]
    KeywordImpl,

    #[token("Result")]
    KeywordResult,

    #[token("Ok")]
    KeywordOk,

    #[token("Err")]
    KeywordErr,

    // ── Type Keywords ────────────────────────────
    #[token("t8")]
    TypeT8,

    #[token("t16")]
    TypeT16,

    #[token("t32")]
    TypeT32,

    #[token("t64")]
    TypeT64,

    #[token("t128")]
    TypeT128,

    #[token("f64")]
    TypeF64,

    #[token("bool")]
    TypeBool,

    #[token("str")]
    TypeStr,

    #[token("strref")]
    TypeStrRef,

    #[token("char")]
    TypeChar,

    #[token("void")]
    TypeVoid,

    // ── Literals ─────────────────────────────────
    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        Some(unescape_string(&s[1..s.len()-1]))
    })]
    LiteralString(String),

    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    LiteralFloat(f64),

    #[regex(r"[0-9]+", |lex| lex.slice().parse::<i128>().ok())]
    LiteralInt(i128),

    #[regex(r"'([^'\\]|\\.)'", |lex| {
        let s = lex.slice();
        let inner = &s[1..s.len()-1];
        let ch = if inner.starts_with('\\') {
            match inner.chars().nth(1).unwrap_or('\\') {
                'n'  => '\n',
                'r'  => '\r',
                't'  => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '0'  => '\0',
                other => other,
            }
        } else {
            inner.chars().next().unwrap_or('\0')
        };
        ch
    })]
    LiteralChar(char),

    // ── Identifiers ───────────────────────────────
    #[regex(r"[_\p{L}][_\p{L}\p{N}]*", |lex| lex.slice().to_string())]
    Identifier(String),

    // ── Arithmetic Operators ──────────────────────
    #[token("+")]
    OpAdd,

    #[token("-")]
    OpSub,

    #[token("*")]
    OpMul,

    #[token("/")]
    OpDiv,

    #[token("%")]
    OpMod,

    #[token("&&")]
    OpAnd,

    #[token("||")]
    OpOr,

    // ── Comparison Operators ──────────────────────
    #[token("!")]
    OpBang,
    #[token("==")]
    OpEqualEqual,
    #[token("!=")]
    OpNotEqual,

    #[token("<=")]
    OpLessEq,

    #[token(">=")]
    OpGreaterEq,

    #[token(">")]
    OpGreaterThan,

    #[token("<")]
    OpLessThan,

    // ── Assignment ───────────────────────────────
    #[token("=")]
    OpAssign,

    // ── Arrow ────────────────────────────────────
    #[token("->")]
    PunctArrow,

    // ── Punctuation ──────────────────────────────
    #[token(":")]
    PunctColon,

    #[token(";")]
    PunctSemicolon,

    #[token(",")]
    PunctComma,

    #[token("?")]
    QuestionMark,

    #[token("..=")]
    RangeInclusive,

    #[token("..")]
    RangeExclusive,

    #[token("=>")]
    PunctFatArrow,

    #[token("::")]
    PunctDoubleColon,

    #[token(".")]
    PunctDot,

    #[token("(")]
    PunctParenOpen,

    #[token(")")]
    PunctParenClose,

    #[token("{")]
    PunctBraceOpen,

    #[token("}")]
    PunctBraceClose,

    #[token("[")]
    PunctBracketOpen,

    #[token("]")]
    PunctBracketClose,

    // ── Macros ───────────────────────────────────
    #[token("#![")]
    MacroInnerOpen,

    #[token("#[")]
    MacroOuterOpen,
}

impl std::fmt::Display for Token {
    /// User-facing name for a token, used in parse-error messages instead of
    /// the internal enum debug name (tracker #014). Keywords/operators render as
    /// the source text they match; literals and identifiers render as a category.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s: &str = match self {
            Token::LineComment => "comment",
            Token::BlockComment => "comment",
            // Keywords
            Token::KeywordLet => "let",
            Token::KeywordFnc => "fnc",
            Token::KeywordConst => "const",
            Token::KeywordPub => "pub",
            Token::KeywordReturn => "return",
            Token::KeywordTrue => "true",
            Token::KeywordFalse => "false",
            Token::KeywordUnknown => "unknown",
            Token::KeywordWhen => "when",
            Token::KeywordWhile => "while",
            Token::KeywordIf => "if",
            Token::KeywordElse => "else",
            Token::KeywordFor => "for",
            Token::KeywordIn => "in",
            Token::KeywordLoop => "loop",
            Token::KeywordBreak => "break",
            Token::KeywordContinue => "continue",
            Token::KeywordThen => "then",
            Token::KeywordEnum => "enum",
            Token::KeywordGroup => "group",
            Token::KeywordHandle => "Handle",
            Token::KeywordStruct => "struct",
            Token::KeywordImpl => "impl",
            Token::KeywordResult => "Result",
            Token::KeywordOk => "Ok",
            Token::KeywordErr => "Err",
            // Type keywords
            Token::TypeT8 => "t8",
            Token::TypeT16 => "t16",
            Token::TypeT32 => "t32",
            Token::TypeT64 => "t64",
            Token::TypeT128 => "t128",
            Token::TypeF64 => "f64",
            Token::TypeBool => "bool",
            Token::TypeStr => "str",
            Token::TypeStrRef => "strref",
            Token::TypeChar => "char",
            Token::TypeVoid => "void",
            // Literals & identifiers
            Token::LiteralString(_) => "string literal",
            Token::LiteralFloat(_) => "float literal",
            Token::LiteralInt(_) => "integer literal",
            Token::LiteralChar(_) => "char literal",
            Token::Identifier(_) => "identifier",
            // Operators
            Token::OpAdd => "+",
            Token::OpSub => "-",
            Token::OpMul => "*",
            Token::OpDiv => "/",
            Token::OpMod => "%",
            Token::OpAnd => "&&",
            Token::OpOr => "||",
            Token::OpBang => "!",
            Token::OpEqualEqual => "==",
            Token::OpNotEqual => "!=",
            Token::OpLessEq => "<=",
            Token::OpGreaterEq => ">=",
            Token::OpGreaterThan => ">",
            Token::OpLessThan => "<",
            Token::OpAssign => "=",
            // Punctuation
            Token::PunctArrow => "->",
            Token::PunctColon => ":",
            Token::PunctSemicolon => ";",
            Token::PunctComma => ",",
            Token::QuestionMark => "?",
            Token::RangeInclusive => "..=",
            Token::RangeExclusive => "..",
            Token::PunctFatArrow => "=>",
            Token::PunctDoubleColon => "::",
            Token::PunctDot => ".",
            Token::PunctParenOpen => "(",
            Token::PunctParenClose => ")",
            Token::PunctBraceOpen => "{",
            Token::PunctBraceClose => "}",
            Token::PunctBracketOpen => "[",
            Token::PunctBracketClose => "]",
            // Macros
            Token::MacroInnerOpen => "#![",
            Token::MacroOuterOpen => "#[",
        };
        f.write_str(s)
    }
}

pub struct ParseError {
    pub msg: String,
    pub pos: usize,
}

#[derive(Debug, Clone)]
pub struct Tok {
    pub kind: Token,
    pub span: std::ops::Range<usize>,
}

// Drive the Logos lexer to collect tokens and emit diagnostics on bad input.
pub fn tok_collector(input: &str) -> Result<Vec<Tok>, ParseError> {
    let mut lex_in = Token::lexer(input);
    let mut lex_out = Vec::new();

    while let Some(res) = lex_in.next() {
        match res {
            Ok(kind) => {
                lex_out.push(Tok {
                    kind,
                    span: lex_in.span(),
                });
            }
            Err(_) => {
                let span = lex_in.span();
                return Err(ParseError {
                    msg: diagnostics::lexer_error_message(lex_in.slice()),
                    pos: span.start,
                });
            }
        }
    }
    Ok(lex_out)
}
