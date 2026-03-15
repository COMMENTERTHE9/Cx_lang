// incremental rebuild test
use chumsky::{
    input::{MapExtra, ValueInput},
    prelude::*,
};

use crate::frontend::{ast::*, lexer::Token};

type Span = SimpleSpan;
type ParserError<'a> = extra::Err<Rich<'a, Token, Span>>;
type ParseExtra<'a, 'b, I> = MapExtra<'a, 'b, I, ParserError<'a>>;

fn expr_pos(expr: &Expr) -> usize {
    match expr {
        Expr::Val(_) => 0,
        Expr::Ident(_, pos) => *pos,
        Expr::DotAccess(_, _) => 0,
        Expr::HandleNew(_, pos) => *pos,
        Expr::HandleVal(_, pos) => *pos,
        Expr::HandleDrop(_, pos) => *pos,
        Expr::Call(_, _, pos) => *pos,
        Expr::Unary(_, _, pos) => *pos,
        Expr::Range(a, _, _) => expr_pos(a),
        Expr::Bin(_, _, pos, _) => *pos,
        Expr::ArrayLit(_) => 0,
        Expr::Index(_, _, pos) => *pos,
    }
}

fn type_parser<'a, I>() -> impl Parser<'a, I, Type, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    let scalar = select! {
        Token::TypeT8     => Type::T8,
        Token::TypeT16    => Type::T16,
        Token::TypeT32    => Type::T32,
        Token::TypeT64    => Type::T64,
        Token::TypeT128   => Type::T128,
        Token::TypeBool   => Type::Bool,
        Token::TypeStr    => Type::Str,
        Token::TypeStrRef => Type::StrRef,
        Token::TypeChar   => Type::Char,
    };

    let array = just(Token::PunctBracketOpen)
        .ignore_then(select! { Token::LiteralInt(n) => n as usize })
        .then_ignore(just(Token::PunctBracketClose))
        .then(scalar.clone())
        .map(|(size, elem_ty)| Type::Array(size, Box::new(elem_ty)));

    array.or(scalar)
}

fn expr_parser<'a, I>() -> impl Parser<'a, I, Expr, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    recursive(|expr| {
        let literal = select! {
            Token::LiteralInt(n)    => Expr::Val(AstValue::Num(n)),
            Token::LiteralFloat(x)  => Expr::Val(AstValue::Float(x)),
            Token::LiteralString(s) => Expr::Val(AstValue::Str(s)),
            Token::LiteralChar(c)   => Expr::Val(AstValue::Char(c)),
            Token::KeywordTrue      => Expr::Val(AstValue::Bool(true)),
            Token::KeywordFalse     => Expr::Val(AstValue::Bool(false)),
        }
        .or(just(Token::QuestionMark)
            .map_with(|_, _e: &mut ParseExtra<'a, '_, I>| Expr::Val(AstValue::Unknown)));

        let ident = select! { Token::Identifier(s) => s };
        let ident_with_pos = ident
            .clone()
            .map_with(|s, e: &mut ParseExtra<'a, '_, I>| (s, e.span().start));
        let pos = empty().map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start);

        let args = {
            let call_arg = select! { Token::Identifier(s) if s == "copy_into" => () }
                .ignore_then(
                    ident
                        .clone()
                        .separated_by(just(Token::PunctComma))
                        .collect::<Vec<_>>()
                        .delimited_by(just(Token::PunctParenOpen), just(Token::PunctParenClose)),
                )
                .map(|vars| CallArg::CopyInto(vars))
                .or(ident
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
                        Some((field, None)) => CallArg::Expr(Expr::DotAccess(name, field)),
                        _ => CallArg::Expr(Expr::Ident(name, 0)),
                    })
                    .or(expr.clone().map(CallArg::Expr)));

            call_arg
                .separated_by(just(Token::PunctComma))
                .collect::<Vec<_>>()
                .delimited_by(just(Token::PunctParenOpen), just(Token::PunctParenClose))
        };

        let handle_new = just(Token::KeywordHandle)
            .then_ignore(just(Token::PunctDot))
            .then_ignore(select! { Token::Identifier(s) if s == "new" => s })
            .then_ignore(just(Token::PunctParenOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctParenClose))
            .then(pos.clone())
            .map(|((_, e), p)| Expr::HandleNew(Box::new(e), p));

        let handle_drop = ident
            .clone()
            .then_ignore(just(Token::PunctDot))
            .then_ignore(select! { Token::Identifier(s) if s == "drop" => s })
            .then_ignore(just(Token::PunctParenOpen))
            .then_ignore(just(Token::PunctParenClose))
            .then(pos.clone())
            .map(|(name, p)| Expr::HandleDrop(name, p));

        let handle_val = ident
            .clone()
            .then_ignore(just(Token::PunctDot))
            .then_ignore(select! { Token::Identifier(s) if s == "val" => s })
            .then(pos.clone())
            .map(|(name, p)| Expr::HandleVal(name, p));

        let enum_variant = ident
            .clone()
            .then_ignore(just(Token::PunctDoubleColon))
            .then(ident.clone())
            .map_with(|(enum_name, variant), _| {
                Expr::Val(AstValue::EnumVariant(enum_name, variant))
            });

        let ident_or_call = ident_with_pos
            .then(args.or_not())
            .then(
                just(Token::PunctDot)
                    .ignore_then(select! { Token::Identifier(s) => s })
                    .or_not(),
            )
            .map(|(((name, pos), args), field)| match (args, field) {
                (Some(args), _) => Expr::Call(name, args, pos),
                (None, Some(field)) => Expr::DotAccess(name, field),
                (None, None) => Expr::Ident(name, pos),
            });

        let paren = expr
            .clone()
            .delimited_by(just(Token::PunctParenOpen), just(Token::PunctParenClose));

        let array_lit = expr
            .clone()
            .separated_by(just(Token::PunctComma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .delimited_by(
                just(Token::PunctBracketOpen),
                just(Token::PunctBracketClose),
            )
            .map(|elems| Expr::ArrayLit(elems));

        let primary = literal
            .or(enum_variant)
            .or(handle_new)
            .or(handle_drop)
            .or(handle_val)
            .or(ident_or_call)
            .or(paren)
            .or(array_lit);

        let indexed = primary
            .clone()
            .then(
                just(Token::PunctColon)
                    .ignore_then(just(Token::PunctBracketOpen))
                    .ignore_then(expr.clone())
                    .then_ignore(just(Token::PunctBracketClose))
                    .map_with(|idx_expr, e: &mut ParseExtra<'a, '_, I>| (idx_expr, e.span().start))
                    .or_not(),
            )
            .map(|(base, idx)| match idx {
                Some((idx_expr, pos)) => Expr::Index(Box::new(base), Box::new(idx_expr), pos),
                None => base,
            });

        let unary = choice((
            just(Token::OpMul).to(Op::Mul),
            just(Token::OpSub).to(Op::Minus),
        ))
        .map_with(|op, e: &mut ParseExtra<'a, '_, I>| (op, e.span().start))
        .then(indexed.clone())
        .map(|((op, pos), expr)| Expr::Unary(op, Box::new(expr), pos))
        .or(indexed.clone());

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

        let and_op = just(Token::OpAnd).map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start);

        let or_op = just(Token::OpOr).map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start);

        let term = unary.clone().foldl(
            mul_div_op.then(unary).repeated(),
            |lhs, ((op, op_pos), rhs)| Expr::Bin(Box::new(lhs), op, op_pos, Box::new(rhs)),
        );

        let additive = term.clone().foldl(
            add_sub_op.then(term).repeated(),
            |lhs, ((op, op_pos), rhs)| Expr::Bin(Box::new(lhs), op, op_pos, Box::new(rhs)),
        );

        let equality_op = choice((
            just(Token::OpEqualEqual).to(Op::EqEq),
            just(Token::OpLessThan).to(Op::Lt),
            just(Token::OpGreaterThan).to(Op::Gt),
            just(Token::OpLessEq).to(Op::LtEq),
            just(Token::OpGreaterEq).to(Op::GtEq),
        ))
        .map_with(|op, e: &mut ParseExtra<'a, '_, I>| (op, e.span().start));

        let equality = additive.clone().foldl(
            equality_op.then(additive).repeated(),
            |lhs, ((op, op_pos), rhs)| Expr::Bin(Box::new(lhs), op, op_pos, Box::new(rhs)),
        );

        let logical_and = equality
            .clone()
            .foldl(and_op.then(equality).repeated(), |lhs, (op_pos, rhs)| {
                Expr::Bin(Box::new(lhs), Op::And, op_pos, Box::new(rhs))
            });

        logical_and
            .clone()
            .foldl(or_op.then(logical_and).repeated(), |lhs, (op_pos, rhs)| {
                Expr::Bin(Box::new(lhs), Op::Or, op_pos, Box::new(rhs))
            })
    })
}

pub fn stmt_parser<'a, I>() -> impl Parser<'a, I, Stmt, ParserError<'a>> + Clone
where
    I: ValueInput<'a, Token = Token, Span = Span>,
{
    recursive(|stmt| {
        let expr = expr_parser::<I>().boxed();
        let ty = type_parser::<I>();
        let ident = select! { Token::Identifier(s) => s };
        let semi = just(Token::PunctSemicolon);

        let decl = just(Token::KeywordLet)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(ident.clone())
            .then(just(Token::PunctColon).ignore_then(ty.clone()).or_not())
            .then_ignore(semi.clone())
            .map(|((pos, name), ty)| Stmt::Decl { name, ty, pos })
            .boxed();

        let index_assign = ident
            .clone()
            .map_with(|name, e: &mut ParseExtra<'a, '_, I>| (name, e.span().start))
            .then_ignore(just(Token::PunctColon))
            .then_ignore(just(Token::PunctBracketOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctBracketClose))
            .then(just(Token::OpAssign).map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start))
            .then(expr.clone())
            .then_ignore(semi.clone().or_not())
            .map(
                |((((name, name_pos), idx_expr), pos_eq), val_expr)| Stmt::Assign {
                    target: Expr::Index(
                        Box::new(Expr::Ident(name, name_pos)),
                        Box::new(idx_expr),
                        name_pos,
                    ),
                    expr: val_expr,
                    pos_eq,
                },
            )
            .boxed();

        let assign = ident
            .clone()
            .map_with(|name, e: &mut ParseExtra<'a, '_, I>| (name, e.span().start))
            .then(
                just(Token::PunctDot)
                    .ignore_then(select! { Token::Identifier(s) => s })
                    .or_not(),
            )
            .then(just(Token::OpAssign).map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start))
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
            })
            .boxed();

        let typed_assign = ident
            .clone()
            .then(
                just(Token::PunctColon).map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start),
            )
            .then(ty.clone())
            .then_ignore(just(Token::OpAssign))
            .then(expr.clone())
            .then_ignore(semi.clone().or_not())
            .map(|(((name, pos_type), ty), expr)| Stmt::TypedAssign {
                name,
                ty,
                expr,
                pos_type,
            })
            .boxed();

        let print = just(Token::KeywordPrint)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::PunctParenOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctParenClose))
            .then_ignore(semi.clone().or_not())
            .map(|(pos, expr)| Stmt::Print { expr, pos })
            .boxed();

        let print_inline = just(Token::KeywordPrintInline)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::PunctParenOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctParenClose))
            .then_ignore(semi.clone().or_not())
            .map(|(pos, expr)| Stmt::PrintInline { expr, _pos: pos })
            .boxed();

        let ret = just(Token::KeywordReturn)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(expr.clone().or_not())
            .then_ignore(semi.clone())
            .map(|(pos, expr)| Stmt::Return { expr, pos })
            .boxed();

        let block = just(Token::PunctBraceOpen)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(|(pos, stmts)| Stmt::Block { stmts, _pos: pos })
            .boxed();

        let expr_stmt = expr
            .clone()
            .then_ignore(semi.clone().or_not())
            .map(|expr| Stmt::ExprStmt {
                _pos: expr_pos(&expr),
                expr,
            })
            .boxed();

        let expr_stmt_with_semi = expr
            .clone()
            .then_ignore(semi.clone())
            .map(|expr| Stmt::ExprStmt {
                _pos: expr_pos(&expr),
                expr,
            })
            .boxed();

        let copy_into_param = ident
            .clone()
            .then_ignore(just(Token::PunctColon))
            .then_ignore(select! { Token::Identifier(s) if s == "copy_into" => () })
            .then_ignore(just(Token::PunctParenOpen))
            .then(
                ident
                    .clone()
                    .separated_by(just(Token::PunctComma))
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(Token::PunctParenClose))
            .map(|(name, vars)| ParamKind::CopyInto(name, vars));

        let param = copy_into_param
            .or(ident
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
                    Some((m1, Some(m2))) if m1 == "copy" && m2 == "free" => {
                        ParamKind::CopyFree(name)
                    }
                    Some((m1, None)) if m1 == "copy" => ParamKind::Copy(name),
                    _ => ParamKind::Typed(name, ty_opt.unwrap()),
                }))
            .boxed();

        let group = just(Token::KeywordGroup)
            .ignore_then(just(Token::PunctDoubleColon))
            .ignore_then(ident.clone())
            .then_ignore(just(Token::PunctBracketOpen))
            .then(
                ident
                    .clone()
                    .separated_by(just(Token::PunctComma))
                    .allow_trailing()
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(Token::PunctBracketClose))
            .then_ignore(just(Token::PunctComma).or_not())
            .map(|(name, variants)| (name, variants));

        let sub_group = ident
            .clone()
            .then_ignore(just(Token::PunctBracketOpen))
            .then(
                ident
                    .clone()
                    .separated_by(just(Token::PunctComma))
                    .allow_trailing()
                    .collect::<Vec<_>>(),
            )
            .then_ignore(just(Token::PunctBracketClose))
            .then_ignore(just(Token::PunctComma).or_not())
            .map(|(name, variants)| (name, variants));

        let super_group = just(Token::KeywordGroup)
            .ignore_then(just(Token::PunctDoubleColon))
            .ignore_then(ident.clone())
            .then_ignore(just(Token::PunctBracketOpen))
            .then(sub_group.repeated().at_least(1).collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBracketClose))
            .then_ignore(just(Token::PunctComma).or_not())
            .map(|(name, sub_groups)| (name, sub_groups));

        let enum_def = just(Token::KeywordEnum)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(ident.clone())
            .then_ignore(just(Token::PunctBraceOpen))
            .then(choice((
                super_group
                    .clone()
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<_>>()
                    .map(|sgs| (vec![], vec![], sgs)),
                group
                    .clone()
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<_>>()
                    .map(|groups| (vec![], groups, vec![])),
                ident
                    .clone()
                    .separated_by(just(Token::PunctComma))
                    .allow_trailing()
                    .collect::<Vec<_>>()
                    .map(|variants| (variants, vec![], vec![])),
            )))
            .then_ignore(just(Token::PunctBraceClose))
            .map(
                |((pos, name), (variants, groups, super_groups))| Stmt::EnumDef {
                    name,
                    variants,
                    groups,
                    super_groups,
                    pos,
                },
            )
            .boxed();

        let when_arm = {
            let pattern = choice((
                select! { Token::Identifier(s) if s == "_" => WhenPattern::Catchall },
                just(Token::KeywordUnknown).map(|_| WhenPattern::Literal(AstValue::Unknown)),
                select! { Token::LiteralInt(n) => n }
                    .then(choice((
                        just(Token::RangeInclusive).to(true),
                        just(Token::RangeExclusive).to(false),
                    )))
                    .then(select! { Token::LiteralInt(n) => n })
                    .map(|((start, inclusive), end)| {
                        WhenPattern::Range(
                            AstValue::Num(start),
                            AstValue::Num(end),
                            inclusive,
                        )
                    }),
                ident
                    .clone()
                    .then_ignore(just(Token::PunctDoubleColon))
                    .then(ident.clone())
                    .map(|(enum_name, variant)| WhenPattern::EnumVariant(enum_name, variant)),
                ident
                    .clone()
                    .map(|name| WhenPattern::Group(String::new(), name)),
                expr.clone().map(|e| match e {
                    Expr::Val(v) => WhenPattern::Literal(v),
                    _ => WhenPattern::Catchall,
                }),
            ))
            .boxed();

            let placeholder_handler = just(Token::PunctBraceOpen)
                .then(select! { Token::Identifier(s) if s == "_" => () })
                .then(just(Token::PunctBraceClose))
                .map(|_| SuperGroupHandler::Placeholder)
                .boxed();

            let stmt_handler = stmt
                .clone()
                .try_map(|s, span| match &s {
                    Stmt::ExprStmt {
                        expr: Expr::Call(_, _, _),
                        ..
                    } => Ok(SuperGroupHandler::Stmts(vec![s])),
                    Stmt::ExprStmt {
                        expr: Expr::Ident(_, _),
                        ..
                    } => Err(Rich::custom(
                        span,
                        "bare identifier is not a valid super-group handler",
                    )),
                    Stmt::Print { .. } => Ok(SuperGroupHandler::Stmts(vec![s])),
                    Stmt::Break { .. } | Stmt::Continue { .. } => {
                        Ok(SuperGroupHandler::Stmts(vec![s]))
                    }
                    _ => Err(Rich::custom(
                        span,
                        "only call expressions are valid super-group handlers",
                    )),
                })
                .boxed();

            let handler_item = placeholder_handler.or(stmt_handler).boxed();

            let brace_body = just(Token::PunctBraceOpen)
                .ignore_then(stmt.clone().repeated().collect::<Vec<_>>())
                .then_ignore(just(Token::PunctBraceClose))
                .map(|stmts| WhenBody::Stmts(stmts))
                .boxed();

            let handler_list_body = handler_item
                .separated_by(just(Token::PunctComma))
                .allow_trailing()
                .at_least(1)
                .collect::<Vec<SuperGroupHandler>>()
                .map(|items| {
                    let is_super = items.len() > 1
                        || items
                            .iter()
                            .any(|i| matches!(i, SuperGroupHandler::Placeholder));
                    if is_super {
                        WhenBody::SuperGroup(items)
                    } else {
                        match items.into_iter().next().unwrap() {
                            SuperGroupHandler::Stmts(stmts) => WhenBody::Stmts(stmts),
                            SuperGroupHandler::Placeholder => {
                                WhenBody::SuperGroup(vec![SuperGroupHandler::Placeholder])
                            }
                        }
                    }
                })
                .boxed();

            let when_body = brace_body.or(handler_list_body).boxed();

            pattern
                .map_with(|pattern, e: &mut ParseExtra<'a, '_, I>| (pattern, e.span().start))
                .then_ignore(just(Token::PunctFatArrow))
                .then(when_body)
                .then_ignore(just(Token::PunctComma).or_not())
                .map(|((pattern, pos), body)| WhenArm { pattern, body, pos })
        };

        let when_stmt = just(Token::KeywordWhen)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(
                just(Token::PunctParenOpen)
                    .or_not()
                    .ignore_then(expr.clone())
                    .then_ignore(just(Token::PunctParenClose).or_not()),
            )
            .then_ignore(just(Token::PunctBraceOpen))
            .then(when_arm.repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(|((pos, expr), arms)| Stmt::When { expr, arms, pos })
            .boxed();

        // while in arr:[slot], start..end { body } then in arr2:[slot], start..end { body }
        let arr_slot = ident
            .clone()
            .then_ignore(just(Token::PunctColon))
            .then_ignore(just(Token::PunctBracketOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctBracketClose));

        let while_in_range = expr
            .clone()
            .then(choice((
                just(Token::RangeInclusive).to(true),
                just(Token::RangeExclusive).to(false),
            )))
            .then(expr.clone());

        let then_chain = just(Token::KeywordThen)
            .ignore_then(just(Token::KeywordIn))
            .ignore_then(arr_slot.clone())
            .then_ignore(just(Token::PunctComma))
            .then(while_in_range.clone())
            .then_ignore(just(Token::PunctBraceOpen))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(
                |(((arr, slot_expr), ((start, inclusive), end)), body)| {
                    let start_slot = match &slot_expr {
                        Expr::Val(AstValue::Num(n)) => *n as usize,
                        _ => 0,
                    };
                    WhileInChain {
                        arr,
                        start_slot,
                        range_start: start,
                        range_end: end,
                        inclusive,
                        body,
                    }
                },
            );

        let if_else = recursive(|if_else| {
            let if_body = just(Token::PunctBraceOpen)
                .ignore_then(stmt.clone().repeated().collect::<Vec<_>>())
                .then_ignore(just(Token::PunctBraceClose));

            just(Token::KeywordIf)
                .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
                .then(expr.clone())
                .then(if_body.clone())
                .then(
                    just(Token::KeywordElse)
                        .ignore_then(just(Token::KeywordIf))
                        .ignore_then(expr.clone())
                        .then(if_body.clone())
                        .repeated()
                        .collect::<Vec<_>>(),
                )
                .then(
                    just(Token::KeywordElse)
                        .ignore_then(if_body.clone())
                        .or_not(),
                )
                .map(|((((pos, condition), then_body), else_ifs), else_body)| {
                    Stmt::IfElse {
                        condition,
                        then_body,
                        else_ifs,
                        else_body,
                        pos,
                    }
                })
                .boxed()
        });

        let while_in = just(Token::KeywordWhile)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::KeywordIn))
            .then(arr_slot.clone())
            .then_ignore(just(Token::PunctComma))
            .then(while_in_range.clone())
            .then_ignore(just(Token::PunctBraceOpen))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .then(then_chain.repeated().collect::<Vec<_>>())
            .map(
                |((((pos, (arr, slot_expr)), ((start, inclusive), end)), body), chains)| {
                    let start_slot = match &slot_expr {
                        Expr::Val(AstValue::Num(n)) => *n as usize,
                        _ => 0,
                    };
                    Stmt::WhileIn {
                        arr,
                        start_slot,
                        range_start: start,
                        range_end: end,
                        inclusive,
                        body,
                        then_chains: chains,
                        result: None,
                        pos,
                    }
                },
            )
            .boxed();

        let while_stmt = just(Token::KeywordWhile)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::PunctParenOpen))
            .then(expr.clone())
            .then_ignore(just(Token::PunctParenClose))
            .then_ignore(just(Token::PunctBraceOpen))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(|((pos, cond), body)| Stmt::While { cond, body, pos })
            .boxed();

        let for_stmt = just(Token::KeywordFor)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then(ident.clone())
            .then_ignore(just(Token::KeywordIn))
            .then(expr.clone())
            .then(choice((
                just(Token::RangeInclusive).to(true),
                just(Token::RangeExclusive).to(false),
            )))
            .then(expr.clone())
            .then_ignore(just(Token::PunctBraceOpen))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(
                |(((((pos, var), start), inclusive), end), body)| Stmt::For {
                    var,
                    start,
                    end,
                    inclusive,
                    body,
                    pos,
                },
            )
            .boxed();

        let loop_stmt = just(Token::KeywordLoop)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(just(Token::PunctBraceOpen))
            .then(stmt.clone().repeated().collect::<Vec<_>>())
            .then_ignore(just(Token::PunctBraceClose))
            .map(|(pos, body)| Stmt::Loop { body, pos })
            .boxed();

        let break_stmt = just(Token::KeywordBreak)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(semi.clone().or_not())
            .map(|pos| Stmt::Break { pos })
            .boxed();

        let continue_stmt = just(Token::KeywordContinue)
            .map_with(|_, e: &mut ParseExtra<'a, '_, I>| e.span().start)
            .then_ignore(semi.clone().or_not())
            .map(|pos| Stmt::Continue { pos })
            .boxed();

        let func_def = recursive(|func_def| {
            let func_body_stmt = choice((
                if_else.clone(),
                decl.clone(),
                func_def.clone(),
                print.clone(),
                print_inline.clone(),
                ret.clone(),
                typed_assign.clone(),
                assign.clone(),
                when_stmt.clone(),
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
        })
        .boxed();

        let compound_assign = ident
            .clone()
            .map_with(|name, e: &mut ParseExtra<'a, '_, I>| (name, e.span().start))
            .then(choice((
                just(Token::OpAdd).to(Op::Plus),
                just(Token::OpSub).to(Op::Minus),
                just(Token::OpMul).to(Op::Mul),
                just(Token::OpDiv).to(Op::Div),
                just(Token::OpMod).to(Op::Mod),
            )))
            .then(select! { Token::LiteralInt(n) => n })
            .then_ignore(just(Token::OpAssign))
            .then_ignore(semi.clone().or_not())
            .map(|(((name, pos), op), n)| Stmt::CompoundAssign {
                name,
                op,
                operand: Expr::Val(AstValue::Num(n)),
                pos,
            })
            .boxed();

        choice((
            enum_def,
            if_else.clone(),
            decl,
            func_def,
            print,
            print_inline,
            ret,
            typed_assign,
            compound_assign,
            index_assign,
            assign,
            block,
            when_stmt,
            while_in,
            while_stmt,
            for_stmt,
            loop_stmt,
            break_stmt,
            continue_stmt,
            expr_stmt,
        ))
        .boxed()
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
