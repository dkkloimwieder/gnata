//! Function types and call machinery for JSONata evaluation.
//!
//! Port of Go `internal/evaluator/eval_function.go` and `env.go` function types.

use std::fmt;
use std::rc::Rc;

use crate::error::{JsonataError, JsonataResult};
use crate::parser::{AstArena, Expr, NodeId};
use crate::value::Value;

use super::environment::Environment;
use super::eval;

/// A native Rust function implementing a JSONata built-in.
/// `args` are the evaluated arguments; `focus` is the current context value.
pub type BuiltinFn = dyn Fn(&[Value], &Value) -> JsonataResult;

/// An environment-aware built-in. Required for HOFs ($map, $filter, etc.)
/// and functions that create child scopes ($eval).
pub type EnvAwareBuiltinFn = dyn Fn(&[Value], &Value, &Rc<Environment>, &AstArena) -> JsonataResult;

/// Callable function value in JSONata.
#[derive(Clone)]
pub enum FunctionValue {
    /// Native built-in function.
    Builtin(Rc<BuiltinFn>),

    /// Environment-aware built-in (HOFs, $eval).
    EnvAwareBuiltin(Rc<EnvAwareBuiltinFn>),

    /// Built-in with type signature for arity/type validation.
    SignedBuiltin {
        func: Rc<BuiltinFn>,
        signature: String,
    },

    /// User-defined lambda (function expression).
    Lambda(Rc<Lambda>),

    /// Partial application (pre-bound args with placeholder slots).
    Partial(Rc<BuiltinFn>),
}

impl fmt::Debug for FunctionValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FunctionValue::Builtin(_) => write!(f, "Builtin(<fn>)"),
            FunctionValue::EnvAwareBuiltin(_) => write!(f, "EnvAwareBuiltin(<fn>)"),
            FunctionValue::SignedBuiltin { signature, .. } => {
                write!(f, "SignedBuiltin({signature})")
            }
            FunctionValue::Lambda(l) => write!(f, "Lambda({:?})", l.params),
            FunctionValue::Partial(_) => write!(f, "Partial(<fn>)"),
        }
    }
}

/// User-defined function (lambda expression).
#[derive(Debug)]
pub struct Lambda {
    pub params: Vec<String>,
    pub body: NodeId,
    pub closure: Rc<Environment>,
    pub thunk: bool,
    pub signature: String,
    pub captured_focus: Value,
}

/// Tail-call sentinel returned by thunked function calls.
#[derive(Clone)]
pub struct TailCall {
    pub func: FunctionValue,
    pub args: Vec<Value>,
}

impl fmt::Debug for TailCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TailCall({:?}, {} args)", self.func, self.args.len())
    }
}

/// Evaluate a function call node.
pub fn eval_function(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (procedure, arguments, thunk, keep_array) = match arena.get(node) {
        Expr::Function {
            procedure,
            arguments,
            thunk,
            keep_array,
            ..
        } => (*procedure, arguments.clone(), *thunk, *keep_array),
        _ => unreachable!("eval_function called on non-Function node"),
    };

    // Resolve the function value.
    // When % is used as a function callee (e.g., %(1)), the parent context
    // error S0217 should become T1006 (not a function).
    let fn_val = match eval(arena, procedure, input, env) {
        Ok(v) => v,
        Err(e) if e.code == "S0217" => Value::Undefined,
        Err(e) => return Err(e),
    };

    let func = match fn_val {
        Value::Function(f) => f,
        Value::Undefined => {
            // Check if this is a Name procedure that exists in env as a function
            // but was accessed without $ → T1005.
            if let Expr::Name { value, .. } = arena.get(procedure)
                && let Some(Value::Function(_)) = env.lookup(value)
            {
                return Err(JsonataError::new(
                    "T1005",
                    format!("attempted to invoke a function that has no definition: {value}"),
                ));
            }
            return Err(JsonataError::new(
                "T1006",
                "attempted to invoke undefined function",
            ));
        }
        _ => {
            return Err(JsonataError::new("T1006", "not a function".to_string()));
        }
    };

    // Evaluate arguments.
    let mut args = Vec::with_capacity(arguments.len());
    for &arg_node in &arguments {
        if matches!(arena.get(arg_node), Expr::Placeholder { .. }) {
            args.push(Value::Undefined);
            continue;
        }
        let val = eval(arena, arg_node, input, env)?;
        args.push(val);
    }

    // Signature validation for SignedBuiltins at direct call site.
    // HOF callbacks bypass this (they go through apply_function instead).
    if let FunctionValue::SignedBuiltin { signature, .. } = &func {
        let specs = super::parse_signature(signature)?;
        let (coerced, return_undefined) = super::process_call_args(&specs, &args)?;
        if return_undefined {
            return Ok(Value::Undefined);
        }
        args = coerced;
    }

    // Tail-call optimization: if this call is in tail position within a
    // lambda body, return a TailCall sentinel instead of recursing.
    if thunk && let FunctionValue::Lambda(_) = &func {
        return Ok(Value::TailCall(Box::new(TailCall { func, args })));
    }

    let result = call_function(&func, &args, input, env, arena)?;
    if keep_array {
        // Apply keep_array wrapping: collapse sequences and ensure array result.
        match result {
            Value::Sequence(seq) => Ok(seq.collapse_and_keep(true)),
            Value::Array(_) => Ok(result),
            Value::Undefined => Ok(Value::Undefined),
            scalar => Ok(Value::Array(vec![scalar])),
        }
    } else {
        Ok(result)
    }
}

/// Evaluate a lambda expression node, creating a closure.
pub fn eval_lambda(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (params, body, signature, thunk) = match arena.get(node) {
        Expr::Lambda {
            params,
            body,
            signature,
            thunk,
            ..
        } => (params.clone(), *body, signature.clone(), *thunk),
        _ => unreachable!("eval_lambda called on non-Lambda node"),
    };

    let param_names: Vec<String> = params
        .iter()
        .map(|&p| match arena.get(p) {
            Expr::Variable { name, .. } => name.clone(),
            _ => String::new(),
        })
        .collect();

    let sig = signature
        .map(|s| {
            let r = s.raw.as_str();
            // Strip outer <> brackets if present.
            r.strip_prefix('<')
                .and_then(|r| r.strip_suffix('>'))
                .unwrap_or(r)
                .to_string()
        })
        .unwrap_or_default();

    Ok(Value::Function(FunctionValue::Lambda(Rc::new(Lambda {
        params: param_names,
        body,
        closure: Rc::clone(env),
        thunk,
        signature: sig,
        captured_focus: input.clone(),
    }))))
}

/// Evaluate a partial application node.
pub fn eval_partial(
    arena: &AstArena,
    node: NodeId,
    input: &Value,
    env: &Rc<Environment>,
) -> JsonataResult {
    let (procedure, arguments) = match arena.get(node) {
        Expr::Partial {
            procedure,
            arguments,
            ..
        } => (*procedure, arguments.clone()),
        _ => unreachable!("eval_partial called on non-Partial node"),
    };

    let fn_val = eval(arena, procedure, input, env)?;
    let func = match fn_val {
        Value::Function(f) => f,
        Value::Undefined => {
            // Distinguish T1007 vs T1008 based on whether the name exists in env.
            if let Expr::Name { value, .. } = arena.get(procedure) {
                if env.lookup(value).is_some() {
                    return Err(JsonataError::new(
                        "T1007",
                        "attempted to partially apply a function referenced without $",
                    ));
                }
                return Err(JsonataError::new(
                    "T1008",
                    "cannot partially apply a non-function: the function is not defined",
                ));
            }
            return Err(JsonataError::new(
                "T1007",
                "attempted to partially apply an undefined function",
            ));
        }
        _ => {
            return Err(JsonataError::new(
                "T1008",
                "cannot partially apply a non-function",
            ));
        }
    };

    // Evaluate bound args, tracking placeholders.
    let mut bound_args = Vec::with_capacity(arguments.len());
    let mut is_placeholder = Vec::with_capacity(arguments.len());
    for &arg_node in &arguments {
        if matches!(arena.get(arg_node), Expr::Placeholder { .. }) {
            is_placeholder.push(true);
            bound_args.push(Value::Undefined);
        } else {
            is_placeholder.push(false);
            let val = eval(arena, arg_node, input, env)?;
            bound_args.push(val);
        }
    }

    let env_clone = Rc::clone(env);
    let partial_fn: Rc<super::EnvAwareBuiltinFn> = Rc::new(
        move |args: &[Value],
              focus: &Value,
              _env: &Rc<super::Environment>,
              arena: &crate::parser::AstArena| {
            let mut full_args = bound_args.clone();
            let mut arg_idx = 0;
            for (i, &placeholder) in is_placeholder.iter().enumerate() {
                if placeholder && arg_idx < args.len() {
                    full_args[i] = args[arg_idx].clone();
                    arg_idx += 1;
                }
            }
            super::call_function(&func, &full_args, focus, &env_clone, arena)
        },
    );

    Ok(Value::Function(FunctionValue::EnvAwareBuiltin(partial_fn)))
}

/// Call a function value with arguments. Contains the trampoline loop for TCO.
pub fn call_function(
    func: &FunctionValue,
    args: &[Value],
    focus: &Value,
    env: &Rc<Environment>,
    arena: &AstArena,
) -> JsonataResult {
    let counter = env.call_counter();
    let max_iter = counter.max as usize * 10000;
    let mut current_func = func.clone();
    let mut current_args: Vec<Value> = args.to_vec();
    let mut iter = 0;

    loop {
        if env.is_cancelled() {
            return Err(JsonataError::new("D3001", "evaluation cancelled"));
        }

        match &current_func {
            FunctionValue::SignedBuiltin { func: f, .. } => {
                // No signature validation here — HOF callbacks bypass signatures.
                // Signature is validated at the direct call site (eval_function).
                return f(&current_args, focus);
            }
            FunctionValue::Builtin(f) | FunctionValue::Partial(f) => {
                return f(&current_args, focus);
            }
            FunctionValue::EnvAwareBuiltin(f) => {
                return f(&current_args, focus, env, arena);
            }
            FunctionValue::Lambda(lambda) => {
                // Lambda signature validation.
                if !lambda.signature.is_empty() {
                    let specs = super::parse_signature(&lambda.signature)?;
                    let (coerced, return_undefined) =
                        super::process_call_args(&specs, &current_args)?;
                    if return_undefined {
                        return Ok(Value::Undefined);
                    }
                    current_args = coerced;
                }

                let depth = counter.depth.get() + 1;
                if depth > counter.max {
                    return Err(JsonataError::new(
                        "U1001",
                        format!(
                            "stack overflow error: evaluation exceeded stack depth {}",
                            counter.max
                        ),
                    ));
                }
                counter.depth.set(depth);

                let child_env = {
                    let ce = Environment::new_child(Rc::clone(&lambda.closure));
                    for (i, param) in lambda.params.iter().enumerate() {
                        let val = current_args.get(i).cloned().unwrap_or(Value::Undefined);
                        ce.bind(param.clone(), val);
                    }
                    Rc::new(ce)
                };

                // Zero-param closures use captured focus.
                let body_focus = if lambda.params.is_empty() && current_args.is_empty() {
                    &lambda.captured_focus
                } else {
                    focus
                };

                let result = eval(arena, lambda.body, body_focus, &child_env);
                counter.depth.set(depth - 1);

                match result {
                    Ok(Value::TailCall(tc)) => {
                        iter += 1;
                        if iter > max_iter {
                            return Err(JsonataError::new(
                                "U1001",
                                format!(
                                    "stack overflow error: evaluation exceeded stack depth {}",
                                    counter.max
                                ),
                            ));
                        }
                        current_func = tc.func;
                        current_args = tc.args;
                        continue;
                    }
                    other => return other,
                }
            }
        }
    }
}
