use chumsky::{
    input::{MapExtra, ValueInput},
    prelude::*,
};

use crate::{CallArg, Expr, Op, ParamKind, Program, Stmt, Token, Type, Value};

type Span = SimpleSpan;
type ParserError<'a> = extra::Err<Rich<'a, Token, Span>>;
type ParseExtra<'a, 'b, I> = MapExtra<'a, 'b, I, ParserError<'a>>;

fn expr_pos(expr: &Expr) -> usize {
    match expr {
        Expr::Val(_) => 0,
        Expr::Ident(_, pos) => *pos,
        Expr::DotAccess(_, _) => 0,
        Expr::Call(_, _, pos) => *pos,
        Expr::Bin(_, _, pos, _) => *pos,
    }
}

fn type_parser<'a, I>() -> impl Parser<'a, I, Type, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    select! {
        Token::TypeT8   => Type::T8,
        Token::TypeT16  => Type::T16,
        Token::TypeT32  => Type::T32,
        Token::TypeT64  => Type::T64,
        Token::TypeT128 => Type::T128,
        Token::TypeBool => Type::Bool,
        Token::TypeStr  => Type::Str,
        Token::TypeChar => Type::Char,
    }
}

fn expr_parser<'a, I>() -> impl Parser<'a, I, Expr, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    recursive(|expr| {
        let literal = select! {
            Token::LiteralInt(n)    => Expr::Val(Value::Num(n)),
            Token::LiteralFloat(x)  => Expr::Val(Value::Float(x)),
            Token::LiteralString(s) => Expr::Val(Value::Str(s)),
            Token::LiteralChar(c)   => Expr::Val(Value::Char(c)),
            Token::KeywordTrue      => Expr::Val(Value::Bool(true)),
            Token::KeywordFalse     => Expr::Val(Value::Bool(false)),
        };

        let ident = select! { Token::Identifier(s) => s };
        let ident_with_pos = ident
            .clone()
            .map_with(|s, e: &mut ParseExtra<'a, '_, I>| (s, e.span().start));

        let args = {
            let call_arg = ident
                .clone()
                .then(
                    just(Token::PunctDot)
                        .ignore_then(select! { Token::Identifier(s) => s })
                        .then(
                            just(Token::PunctDot)
                                .ignore_then(select! { Token::Identifier(s) => s })
                                .or_not(),
                        )
                        .or_not(),
                )
                .map(|(name, modifier)| match modifier {
                    Some((m1, Some(m2))) if m1 == "copy" && m2 == "free" => {
                        CallArg::CopyFree(name)
                    }
                    Some((m1, None)) if m1 == "copy" => CallArg::Copy(name),
                    _ => CallArg::Expr(Expr::Ident(name, 0)),
                })
                .or(expr.clone().map(CallArg::Expr));

            call_arg
                .separated_by(just(Token::PunctComma))
                .collect::<Vec<_>>()
                .delimited_by(just(Token::PunctParenOpen), just(Token::PunctParenClose))
        };

        let ident_or_call = ident_with_pos
            .then(args.or_not())
            .map(|((name, pos), args)| match args {
                Some(args) => Expr::Call(name, args, pos),
                None => Expr::Ident(name, pos),
            });

        let paren = expr
            .clone()
            .delimited_by(just(Token::PunctParenOpen), just(Token::PunctParenClose));

        let primary = literal.or(ident_or_call).or(paren);

        let mul_div_op = select! {
            Token::OpMul => Op::Mul,
            Token::OpDiv => Op::Div,
            Token::OpMod => Op::Mod,
        }
        .map_with(|op, e: &mut ParseExtra<'a, '_, I>| (op, e.span().start));

        let add_sub_op = select! {
            Token::OpAdd => Op::Plus,
            Token::OpSub => Op::Minus,
        }
        .map_with(|op, e: &mut ParseExtra<'a, '_, I>| (op, e.span().start));

        let term = primary.clone().foldl(
            mul_div_op
                .then(primary)
                .repeated(),
            |lhs, ((op, op_pos), rhs)| Expr::Bin(Box::new(lhs), op, op_pos, Box::new(rhs)),
        );

        let additive = term.clone().foldl(
            add_sub_op
                .then(term)
                .repeated(),
            |lhs, ((op, op_pos), rhs)| Expr::Bin(Box::new(lhs), op, op_pos, Box::new(rhs)),
        );

        additive.clone().foldl(
            just(Token::OpEqualEqual)
                .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
                .then(additive)
                .repeated(),
            |lhs, (op_pos, rhs)| Expr::Bin(Box::new(lhs), Op::EqEq, op_pos, Box::new(rhs)),
        )
    })
}

pub fn stmt_parser<'a, I>() -> impl Parser<'a, I, Stmt, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    recursive(|stmt| {
        let expr = expr_parser::<I>();
        let ty = type_parser::<I>();
        let ident = select! { Token::Identifier(s) => s };
        let semi = just(Token::PunctSemicolon);

        let decl = just(Token::KeywordLet)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(ident.clone())
            .then(just(Token::PunctColon).ignore_then(ty.clone()).or_not())
            .then_ignore(semi.clone())
            .map(|((pos, name), ty)| Stmt::Decl { name, ty, pos });

        let assign = ident
            .clone()
            .map_with(|name, e: &mut ParseExtra<'a, '_, I>| (name, e.span().start))
            .then(
                just(Token::PunctDot)
                    .ignore_then(select! { Token::Identifier(s) => s })
                    .or_not(),
            )
            .then(just(Token::OpAssign).map_with(|_, e: &mut ParseExtra<'a, '_, I>| {
                e.span().start
            }))
            .then(expr.clone())
            .then_ignore(semi.clone().or_not())
            .map(|((((name, name_pos), field), pos_eq), expr)| {
                let target = match field {
                    Some(f) => Expr::DotAccess(name, f),
                    None => Expr::Ident(name, name_pos),
                };
                Stmt::Assign {
                    target,
                    expr,
                    pos_eq,
                }
            });

        let typed_assign = ident
            .clone()
            .then(just(Token::PunctColon).map_with(
                |_, e: &mut ParseExtra<'a, '_, I>| e.span().start,
            ))
            .then(ty.clone())
            .then_ignore(just(Token::OpAssign))
            .then(expr.clone())
            .then_ignore(semi.clone().or_not())
            .map(|(((name, pos_type), ty), expr)| Stmt::TypedAssign {
                name,
                ty,
                expr,
                pos_type,
            });

        let print = just(Token::KeywordPrint)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::PunctParenOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctParenClose))
            .then_ignore(semi.clone().or_not())
            .map(|(pos, expr)| Stmt::Print { expr, pos });

        let print_inline = just(Token::KeywordPrintInline)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::PunctParenOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctParenClose))
            .then_ignore(semi.clone().or_not())
            .map(|(pos, expr)| Stmt::PrintInline { expr, pos });

        let ret = just(Token::KeywordReturn)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(expr.clone().or_not())
            .then_ignore(semi.clone())
            .map(|(pos, expr)| Stmt::Return { expr, pos });

        let block = just(Token::PunctBraceOpen)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(|(pos, stmts)| Stmt::Block { stmts, pos });

        let expr_stmt = expr
            .clone()
            .then_ignore(semi.clone().or_not())
            .map(|expr| Stmt::ExprStmt {
                pos: expr_pos(&expr),
                expr,
            });

        let expr_stmt_with_semi = expr
            .clone()
            .then_ignore(semi.clone())
            .map(|expr| Stmt::ExprStmt {
                pos: expr_pos(&expr),
                expr,
            });

        let param = ident
            .clone()
            .then(
                just(Token::PunctDot)
                    .ignore_then(select! { Token::Identifier(s) => s })
                    .then(
                        just(Token::PunctDot)
                            .ignore_then(select! { Token::Identifier(s) => s })
                            .or_not(),
                    )
                    .or_not(),
            )
            .then(just(Token::PunctColon).ignore_then(ty.clone()).or_not())
            .map(|((name, modifier), ty_opt)| match modifier {
                Some((m1, Some(m2))) if m1 == "copy" && m2 == "free" => ParamKind::CopyFree(name),
                Some((m1, None)) if m1 == "copy" => ParamKind::Copy(name),
                _ => ParamKind::Typed(name, ty_opt.unwrap()),
            });

        let func_def = recursive(|func_def| {
            let func_body_stmt = choice((
                decl.clone(),
                func_def.clone(),
                print.clone(),
                print_inline.clone(),
                ret.clone(),
                typed_assign.clone(),
                assign.clone(),
                block.clone(),
                expr_stmt_with_semi.clone(),
            ));

            // Keep implicit return support: trailing expression with no semicolon.
            let func_body = just(Token::PunctBraceOpen)
                .ignore_then(
                    func_body_stmt
                        .repeated()
                        .collect::<Vec<_>>()
                        .then(expr.clone().or_not()),
                )
                .then_ignore(just(Token::PunctBraceClose));

            just(Token::KeywordFnc)
                .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
                .then(ident.clone())
                .then(
                    param
                        .separated_by(just(Token::PunctComma))
                        .collect::<Vec<_>>()
                        .delimited_by(just(Token::PunctParenOpen), just(Token::PunctParenClose)),
                )
                .then(just(Token::PunctArrow).ignore_then(ty.clone()).or_not())
                .then(func_body)
                .map(
                    |((((pos, name), params), ret_ty), (body, ret_expr))| Stmt::FuncDef {
                        name,
                        params,
                        ret_ty,
                        body,
                        ret_expr,
                        pos,
                    },
                )
        });

        choice((
            decl,
            func_def,
            print,
            print_inline,
            ret,
            typed_assign,
            assign,
            block,
            expr_stmt,
        ))
    })
}

pub fn program_parser<'a, I>() -> impl Parser<'a, I, Program, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    stmt_parser::<I>()
        .repeated()
        .collect::<Vec<_>>()
        .map(|stmts| Program { stmts })
        .then_ignore(end())
}
