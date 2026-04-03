//! $eval function: dynamically evaluate JSONata expression strings.

use std::rc::Rc;

use crate::error::{JsonataError, JsonataResult};
use crate::evaluator::Environment;
use crate::parser::{AstArena, Parser, process_ast};
use crate::value::Value;

const MAX_EVAL_DEPTH: u32 = 5;

pub fn fn_eval(
    args: &[Value],
    focus: &Value,
    env: &Rc<Environment>,
    _arena: &AstArena,
) -> JsonataResult {
    if args.is_empty() {
        return Err(JsonataError::new(
            "D3006",
            "$eval: requires at least 1 argument",
        ));
    }
    if args[0].is_undefined() {
        return Ok(Value::Undefined);
    }
    let expr = match &args[0] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(JsonataError::new(
                "T0410",
                "$eval: argument must be a string",
            ));
        }
    };

    env.incr_eval_depth(MAX_EVAL_DEPTH)?;

    let result = (|| -> JsonataResult {
        let (mut arena, root) = Parser::parse(expr)
            .map_err(|e| JsonataError::new("D3120", format!("$eval: invalid expression: {e}")))?;
        let root = process_ast(&mut arena, root)
            .map_err(|e| JsonataError::new("D3120", format!("$eval: invalid expression: {e}")))?;

        let ctx = if args.len() >= 2 && !args[1].is_undefined() {
            &args[1]
        } else {
            focus
        };

        let child_env = Rc::new(Environment::new_child(Rc::clone(env)));
        crate::stdlib::register_all_on_rc(&child_env);
        crate::evaluator::eval(&arena, root, ctx, &child_env)
    })();

    env.decr_eval_depth();
    result
}
