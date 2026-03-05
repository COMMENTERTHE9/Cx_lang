use logos::Logos;
use crate::frontend::diagnostics;

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
// Token kinds recognized by the lexer; whitespace is skipped via a Logos attribute.
#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n\f]+")] // skip whitespace
pub enum Token {
    // ── Comments ────────────────────────────────
    #[regex(r"//[^\r\n]*", logos::skip, allow_greedy = true)]
    LineComment,

    // ── Keywords ────────────────────────────────
    #[token("let")]
    KeywordLet,

    #[token("print")]
    KeywordPrint,

    #[token("print!")]
    KeywordPrintInline,

    #[token("fnc")]
    KeywordFnc,

    #[token("return")]
    KeywordReturn,

    #[token("true")]
    KeywordTrue,

    #[token("false")]
    KeywordFalse,

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

    #[token("bool")]
    TypeBool,

    #[token("str")]
    TypeStr,

    #[token("char")]
    TypeChar,

    // ── Literals ─────────────────────────────────
    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        Some(unescape_string(&s[1..s.len()-1]))
    })]
    LiteralString(String),

    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    LiteralFloat(f64),

    #[regex(r"[0-9]+", |lex| lex.slice().parse::<u128>().ok())]
    LiteralInt(u128),

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

    // ── Comparison Operators ──────────────────────
    #[token("==")]
    OpEqualEqual,

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

    #[end]
    EndOfFile,
}

// Helper that mirrors the lexer rules and is handy for tests or manual inspection.
pub fn tok_match(string: &str) -> Token {
    match string {
        "let" => Token::KeywordLet,
        "print" => Token::KeywordPrint,
        "print!" => Token::KeywordPrintInline,
        "fnc" => Token::KeywordFnc,
        "return" => Token::KeywordReturn,
        "true" => Token::KeywordTrue,
        "false" => Token::KeywordFalse,
        "t8" => Token::TypeT8,
        "t16" => Token::TypeT16,
        "t32" => Token::TypeT32,
        "t64" => Token::TypeT64,
        "t128" => Token::TypeT128,
        "bool" => Token::TypeBool,
        "str" => Token::TypeStr,
        "char" => Token::TypeChar,
        "=" => Token::OpAssign,
        "==" => Token::OpEqualEqual,
        ">" => Token::OpGreaterThan,
        "<" => Token::OpLessThan,
        "->" => Token::PunctArrow,
        ":" => Token::PunctColon,
        ";" => Token::PunctSemicolon,
        "," => Token::PunctComma,
        "." => Token::PunctDot,
        "(" => Token::PunctParenOpen,
        ")" => Token::PunctParenClose,
        "+" => Token::OpAdd,
        "-" => Token::OpSub,
        "*" => Token::OpMul,
        "/" => Token::OpDiv,
        "%" => Token::OpMod,
        _ => {
            if let Ok(x) = string.parse::<f64>() {
                Token::LiteralFloat(x)
            } else if let Ok(n) = string.parse::<u128>() {
                Token::LiteralInt(n)
            } else {
                Token::Identifier(string.to_string())
            }
        }
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

