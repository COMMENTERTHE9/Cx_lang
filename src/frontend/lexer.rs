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
