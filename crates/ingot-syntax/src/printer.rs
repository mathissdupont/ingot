//! Canonical printer for the AST — the engine behind `ingot fmt`.
//!
//! There is exactly one canonical rendering of a valid program: two spaces per
//! level, sections in a fixed order, one statement per line. Formatting is
//! idempotent, which the test suite asserts, so `ingot fmt --check` is a stable
//! CI gate.

use std::fmt::Write as _;

use crate::{
    AgentDecl, Arg, BudgetBlock, Expr, FieldDecl, FlowBlock, FunctionDecl, MemoryBlock, ModelBlock,
    ModelRequirement, PolicyAction, PolicyBlock, Program, Stmt, StringLit, StringPart, ToolDecl,
    ToolsBlock, TypeDecl, VerifierDecl,
};

const INDENT: &str = "  ";

/// Target line width. Statements longer than this get their arguments wrapped
/// one per line, which is what keeps a long prompt readable in a diff.
const MAX_WIDTH: usize = 100;

/// Render a whole program in canonical form.
pub fn print_program(program: &Program) -> String {
    let mut out = String::new();

    if let Some(language) = &program.language {
        let _ = writeln!(out, "language {}", language.text());
    }
    if let Some(package) = &program.package {
        let _ = writeln!(out, "package {}", package.text());
    }

    for decl in &program.imports {
        blank_line(&mut out);
        print_import_decl(&mut out, decl);
    }
    for decl in &program.types {
        blank_line(&mut out);
        print_type_decl(&mut out, decl);
    }
    for decl in &program.tools {
        blank_line(&mut out);
        print_tool_decl(&mut out, decl);
    }
    for decl in &program.verifiers {
        blank_line(&mut out);
        print_verifier_decl(&mut out, decl);
    }
    for decl in &program.functions {
        blank_line(&mut out);
        print_function_decl(&mut out, decl);
    }
    for decl in &program.agents {
        blank_line(&mut out);
        print_agent_decl(&mut out, decl);
    }

    out
}

fn blank_line(out: &mut String) {
    if !out.is_empty() {
        out.push('\n');
    }
}

fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str(INDENT);
    }
}

fn print_doc(out: &mut String, doc: Option<&String>, level: usize) {
    let Some(doc) = doc else { return };
    for line in doc.lines() {
        indent(out, level);
        if line.is_empty() {
            out.push_str("///\n");
        } else {
            let _ = writeln!(out, "/// {line}");
        }
    }
}

fn print_import_decl(out: &mut String, decl: &crate::ImportDecl) {
    let _ = writeln!(out, "import \"{}\" {{", decl.path.template());
    for item in &decl.items {
        indent(out, 1);
        let _ = writeln!(out, "{} {}", item.kind.as_str(), item.name.text());
    }
    out.push_str("}\n");
}

fn print_type_decl(out: &mut String, decl: &TypeDecl) {
    print_doc(out, decl.doc.as_ref(), 0);
    let _ = writeln!(out, "type {} {{", decl.name.text);
    for field in &decl.fields {
        indent(out, 1);
        let _ = writeln!(out, "{}: {}", field.name.text, field.ty.text());
    }
    out.push_str("}\n");
}

fn print_params(params: &[FieldDecl]) -> String {
    params
        .iter()
        .map(|param| format!("{}: {}", param.name.text, param.ty.text()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_tool_decl(out: &mut String, decl: &ToolDecl) {
    print_doc(out, decl.doc.as_ref(), 0);
    let _ = write!(
        out,
        "tool {}({}) -> {}",
        decl.name.text(),
        print_params(&decl.params),
        decl.ret.text()
    );
    for effect in &decl.effects {
        let _ = write!(out, " !{}", effect.name.text);
        // Round-tripped even when empty, because `!network()` is a mistake the
        // formatter must leave visible rather than tidy into `!network`, which
        // means something else and compiles.
        if effect.parenthesised {
            let values: Vec<String> = effect
                .values
                .iter()
                .map(|value| format!("\"{}\"", value.template()))
                .collect();
            let _ = write!(out, "({})", values.join(", "));
        }
    }
    out.push('\n');
}

fn print_verifier_decl(out: &mut String, decl: &VerifierDecl) {
    print_doc(out, decl.doc.as_ref(), 0);
    let _ = write!(
        out,
        "verifier {}({})",
        decl.name.text,
        print_params(&decl.params)
    );
    if let Some(body) = &decl.body {
        out.push_str(" = ");
        print_expr(out, body, 0);
    }
    out.push('\n');
}

fn print_function_decl(out: &mut String, decl: &FunctionDecl) {
    print_doc(out, decl.doc.as_ref(), 0);
    let _ = write!(
        out,
        "fn {}({}) -> {} = ",
        decl.name.text,
        print_params(&decl.params),
        decl.ret.text()
    );
    print_expr(out, &decl.body, 0);
    out.push('\n');
}

fn print_agent_decl(out: &mut String, decl: &AgentDecl) {
    print_doc(out, decl.doc.as_ref(), 0);
    let _ = write!(
        out,
        "agent {}({})",
        decl.name.text,
        print_params(&decl.params)
    );
    if let Some(output) = &decl.output {
        let _ = write!(out, " -> {}<{}>", output.name.text, output.content.text);
    }
    out.push_str(" {\n");

    let mut first = true;
    if let Some(model) = &decl.model {
        first = false;
        print_model_block(out, model);
    }
    if let Some(tools) = &decl.tools {
        section_gap(out, &mut first);
        print_tools_block(out, tools);
    }
    if let Some(memory) = &decl.memory {
        section_gap(out, &mut first);
        print_memory_block(out, memory);
    }
    if let Some(budget) = &decl.budget {
        section_gap(out, &mut first);
        print_budget_block(out, budget);
    }
    if let Some(policy) = &decl.policy {
        section_gap(out, &mut first);
        print_policy_block(out, policy);
    }
    if let Some(flow) = &decl.flow {
        section_gap(out, &mut first);
        print_flow_block(out, flow);
    }

    out.push_str("}\n");
}

fn section_gap(out: &mut String, first: &mut bool) {
    if *first {
        *first = false;
    } else {
        out.push('\n');
    }
}

fn print_model_block(out: &mut String, block: &ModelBlock) {
    match block {
        ModelBlock::Exact { reference, .. } => {
            indent(out, 1);
            let _ = writeln!(out, "model exact \"{reference}\"");
        }
        ModelBlock::Requires { requirements, .. } => {
            indent(out, 1);
            out.push_str("model requires {\n");
            for requirement in requirements {
                indent(out, 2);
                match requirement {
                    ModelRequirement::Capability(ident) => {
                        let _ = writeln!(out, "{}", ident.text);
                    }
                    ModelRequirement::ContextAtLeast { tokens, .. } => {
                        let _ = writeln!(out, "context >= {}", format_token_count(*tokens));
                    }
                }
            }
            indent(out, 1);
            out.push_str("}\n");
        }
    }
}

/// Print exact multiples of 1024 back in `k`/`m` form so formatting round-trips.
fn format_token_count(tokens: i64) -> String {
    const MEGA: i64 = 1024 * 1024;
    const KILO: i64 = 1024;
    if tokens >= MEGA && tokens % MEGA == 0 {
        format!("{}m", tokens / MEGA)
    } else if tokens >= KILO && tokens % KILO == 0 {
        format!("{}k", tokens / KILO)
    } else {
        tokens.to_string()
    }
}

fn print_tools_block(out: &mut String, block: &ToolsBlock) {
    indent(out, 1);
    out.push_str("tools {\n");
    for grant in &block.grants {
        indent(out, 2);
        let _ = writeln!(out, "{} {}", grant.transport.text, grant.name.text());
    }
    indent(out, 1);
    out.push_str("}\n");
}

fn print_memory_block(out: &mut String, block: &MemoryBlock) {
    indent(out, 1);
    out.push_str("memory {\n");
    if let Some(working) = &block.working {
        indent(out, 2);
        let _ = write!(out, "working {}", working.lifetime.text);
        if working.fields.is_empty() {
            out.push('\n');
        } else {
            out.push_str(" {\n");
            for field in &working.fields {
                indent(out, 3);
                let _ = writeln!(out, "{}: {}", field.name.text, field.ty.text());
            }
            indent(out, 2);
            out.push_str("}\n");
        }
    }
    if let Some(persistent) = &block.persistent {
        indent(out, 2);
        out.push_str("persistent {\n");
        for field in &persistent.fields {
            indent(out, 3);
            let _ = write!(out, "{}: {}", field.name.text, field.ty.text());
            match &field.initial {
                Some(initial) => {
                    let mut rendered = String::new();
                    print_expr(&mut rendered, initial, 3);
                    let _ = writeln!(out, " = {rendered}");
                }
                // Malformed source. The formatter shows what is there rather
                // than inventing the value the checker is about to ask for.
                None => out.push('\n'),
            }
        }
        indent(out, 2);
        out.push_str("}\n");
    }
    indent(out, 1);
    out.push_str("}\n");
}

fn print_budget_block(out: &mut String, block: &BudgetBlock) {
    indent(out, 1);
    out.push_str("budget {\n");
    for limit in &block.limits {
        indent(out, 2);
        let _ = write!(out, "{} <= {}", limit.key.text, print_number(limit.value));
        if let Some(unit) = &limit.unit {
            let _ = write!(out, " {}", unit.text);
        }
        out.push('\n');
    }
    indent(out, 1);
    out.push_str("}\n");
}

fn print_number(value: crate::NumberLit) -> String {
    match value {
        crate::NumberLit::Int(value) => value.to_string(),
        crate::NumberLit::Float(value) => {
            let rendered = format!("{value}");
            if rendered.contains('.') {
                rendered
            } else {
                format!("{rendered}.0")
            }
        }
    }
}

fn print_policy_block(out: &mut String, block: &PolicyBlock) {
    indent(out, 1);
    out.push_str("policy {\n");
    for rule in &block.rules {
        indent(out, 2);
        let _ = write!(out, "{}", rule.subject.text);
        match &rule.action {
            PolicyAction::Allow {
                qualifier, values, ..
            } => {
                out.push_str(" allow");
                if let Some(qualifier) = qualifier {
                    let _ = write!(out, " {}", qualifier.text);
                }
                if !values.is_empty() {
                    let rendered: Vec<String> = values
                        .iter()
                        .map(|value| format!("\"{}\"", value.template()))
                        .collect();
                    let _ = write!(out, " [{}]", rendered.join(", "));
                }
            }
            PolicyAction::Deny { qualifier, .. } => {
                out.push_str(" deny");
                if let Some(qualifier) = qualifier {
                    let _ = write!(out, " {}", qualifier.text);
                }
            }
            PolicyAction::RequireApproval { .. } => out.push_str(" require approval"),
        }
        out.push('\n');
    }
    indent(out, 1);
    out.push_str("}\n");
}

fn print_flow_block(out: &mut String, block: &FlowBlock) {
    indent(out, 1);
    out.push_str("flow {\n");
    print_statements(out, &block.statements, 2);
    indent(out, 1);
    out.push_str("}\n");
}

fn print_statements(out: &mut String, statements: &[Stmt], level: usize) {
    for statement in statements {
        print_statement(out, statement, level);
    }
}

fn print_statement(out: &mut String, statement: &Stmt, level: usize) {
    match statement {
        Stmt::Bind { name, value, .. } => {
            print_expr_statement(out, &format!("{} = ", name.text), value, level);
        }
        Stmt::StateWrite {
            scope,
            field,
            value,
            ..
        } => {
            let prefix = format!("{}.{} = ", scope.root(), field.text);
            print_expr_statement(out, &prefix, value, level);
        }
        Stmt::Expr { value, .. } => {
            print_expr_statement(out, "", value, level);
        }
        Stmt::Verify {
            validator, args, ..
        } => {
            let prefix = format!("verify {}(", validator.text);
            let inline = format!("{prefix}{})", print_args(args, level));
            if fits(level, "", &inline) {
                indent(out, level);
                out.push_str(&inline);
                out.push('\n');
            } else {
                indent(out, level);
                out.push_str(&prefix);
                print_wrapped_args(out, args, level);
                out.push_str(")\n");
            }
        }
        Stmt::Emit { output, value, .. } => {
            print_expr_statement(out, &format!("emit {} = ", output.text), value, level);
        }
        Stmt::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            indent(out, level);
            out.push_str("if ");
            print_expr(out, condition, level);
            out.push_str(" {\n");
            print_statements(out, then_branch, level + 1);
            indent(out, level);
            if let Some(else_branch) = else_branch {
                out.push_str("} else {\n");
                print_statements(out, else_branch, level + 1);
                indent(out, level);
            }
            out.push_str("}\n");
        }
        Stmt::Loop {
            max, guard, body, ..
        } => {
            indent(out, level);
            out.push_str("loop");
            if let Some(max) = max {
                let _ = write!(out, " max {}", max.value);
            }
            if let Some(guard) = guard {
                out.push_str(" while ");
                print_expr(out, guard, level);
            }
            out.push_str(" {\n");
            print_statements(out, body, level + 1);
            indent(out, level);
            out.push_str("}\n");
        }
        Stmt::Checkpoint { label, .. } => {
            indent(out, level);
            let _ = writeln!(out, "checkpoint \"{}\"", label.template());
        }
        Stmt::Error { .. } => {
            indent(out, level);
            out.push_str("/* <error> */\n");
        }
    }
}

/// Whether `prefix + body` fits on one line at `level`.
fn fits(level: usize, prefix: &str, body: &str) -> bool {
    if body.contains('\n') {
        // Already multi-line (a block expression); leave it alone.
        return true;
    }
    level * INDENT.len() + prefix.chars().count() + body.chars().count() <= MAX_WIDTH
}

/// Print `prefix<expr>` on one line, or wrap the call's arguments if it is too long.
fn print_expr_statement(out: &mut String, prefix: &str, expr: &Expr, level: usize) {
    let mut inline = String::new();
    print_expr(&mut inline, expr, level);

    if fits(level, prefix, &inline) {
        indent(out, level);
        out.push_str(prefix);
        out.push_str(&inline);
        out.push('\n');
        return;
    }

    match expr {
        Expr::Ask { result, args, .. } => {
            indent(out, level);
            let _ = write!(out, "{prefix}ask<{}>(", result.text());
            print_wrapped_args(out, args, level);
            out.push_str(")\n");
        }
        Expr::Call { callee, args, .. } => {
            indent(out, level);
            let _ = write!(out, "{prefix}call {}(", callee.text());
            print_wrapped_args(out, args, level);
            out.push_str(")\n");
        }
        // Nothing sensible to break: a long literal or path stays as it is.
        _ => {
            indent(out, level);
            out.push_str(prefix);
            out.push_str(&inline);
            out.push('\n');
        }
    }
}

/// One argument per line, indented one level deeper than the call.
fn print_wrapped_args(out: &mut String, args: &[Arg], level: usize) {
    if args.is_empty() {
        return;
    }
    out.push('\n');
    for (index, arg) in args.iter().enumerate() {
        indent(out, level + 1);
        if let Some(name) = &arg.name {
            let _ = write!(out, "{}: ", name.text);
        }
        print_expr(out, &arg.value, level + 1);
        if index + 1 < args.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent(out, level);
}

fn print_args(args: &[Arg], level: usize) -> String {
    args.iter()
        .map(|arg| {
            let mut rendered = String::new();
            if let Some(name) = &arg.name {
                let _ = write!(rendered, "{}: ", name.text);
            }
            print_expr(&mut rendered, &arg.value, level);
            rendered
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_expr(out: &mut String, expr: &Expr, level: usize) {
    match expr {
        Expr::Str(literal) => print_string_literal(out, literal),
        Expr::Int { value, .. } => {
            let _ = write!(out, "{value}");
        }
        Expr::Float { value, .. } => {
            let _ = write!(out, "{}", print_number(crate::NumberLit::Float(*value)));
        }
        Expr::Bool { value, .. } => {
            let _ = write!(out, "{value}");
        }
        Expr::List { items, .. } => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                print_expr(out, item, level);
            }
            out.push(']');
        }
        Expr::Path(path) => {
            let _ = write!(out, "{}", path_text(path));
        }
        Expr::Ask { result, args, .. } => {
            let _ = write!(out, "ask<{}>({})", result.text(), print_args(args, level));
        }
        Expr::Consult { args, .. } => {
            let _ = write!(out, "consult({})", print_args(args, level));
        }
        Expr::Call { callee, args, .. } => {
            let _ = write!(out, "call {}({})", callee.text(), print_args(args, level));
        }
        Expr::ParallelMap {
            source,
            binder,
            body,
            ..
        } => {
            out.push_str("parallel map ");
            print_expr(out, source, level);
            let _ = writeln!(out, " as {} {{", binder.text);
            print_statements(out, body, level + 1);
            indent(out, level);
            out.push('}');
        }
        Expr::Builtin { name, args, .. } => {
            let _ = write!(out, "{}(", name.text);
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                print_expr(out, arg, level);
            }
            out.push(')');
        }
        Expr::FunctionCall { callee, args, .. } => {
            let _ = write!(out, "{}({})", callee.text, print_args(args, level));
        }
        Expr::Unary { op, operand, .. } => {
            out.push_str(op.symbol());
            print_expr(out, operand, level);
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            print_expr(out, lhs, level);
            let _ = write!(out, " {} ", op.symbol());
            print_expr(out, rhs, level);
        }
        Expr::Error { .. } => out.push_str("/* <error> */"),
    }
}

fn print_string_literal(out: &mut String, literal: &StringLit) {
    out.push('"');
    for part in &literal.parts {
        match part {
            StringPart::Literal(text) => {
                for ch in text.chars() {
                    match ch {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        other => out.push(other),
                    }
                }
            }
            StringPart::Interpolation(path) => {
                let _ = write!(out, "${{{}}}", path.text());
            }
        }
    }
    out.push('"');
}

fn path_text(path: &crate::PathExpr) -> String {
    let mut out = match &path.root {
        crate::PathRoot::Binding(ident) => ident.text.clone(),
        crate::PathRoot::State { .. } => "state".to_string(),
        crate::PathRoot::Memory { .. } => "memory".to_string(),
    };
    for segment in &path.segments {
        out.push('.');
        out.push_str(&segment.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::format_token_count;

    #[test]
    fn renders_binary_multiples_with_suffixes() {
        assert_eq!(format_token_count(131_072), "128k");
        assert_eq!(format_token_count(2 * 1024 * 1024), "2m");
        assert_eq!(format_token_count(120_000), "120000");
    }
}
