// Benchmark CLI for Rust JSONata.
// Usage: gnata-bench -expr 'Account.Name' -data '{"Account":{"Name":"Firefly"}}' [-n 1000]

use std::rc::Rc;

use gnata::evaluator::Environment;
use gnata::parser::{Parser, process_ast};
use gnata::value::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut expr_str = "";
    let mut data_str = String::from("{}");
    let mut n: u64 = 1;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-expr" => { expr_str = &args[i + 1]; i += 2; }
            "-data" => { data_str = args[i + 1].clone(); i += 2; }
            "-datafile" => {
                data_str = std::fs::read_to_string(&args[i + 1]).expect("read file failed");
                i += 2;
            }
            "-n" => { n = args[i + 1].parse().expect("invalid -n"); i += 2; }
            _ => { i += 1; }
        }
    }

    if expr_str.is_empty() {
        eprintln!("usage: gnata-bench -expr EXPR [-data JSON | -datafile FILE] [-n ITERS]");
        std::process::exit(1);
    }

    let (mut arena, root) = Parser::parse(expr_str).expect("parse failed");
    let root = process_ast(&mut arena, root).expect("process failed");
    let input = Value::from_json_str(&data_str).unwrap_or(Value::Undefined);

    let mut env = Environment::new();
    gnata::stdlib::register_all(&mut env);
    if !input.is_undefined() {
        env.bind("$".into(), input.clone());
    }
    let env = Rc::new(env);

    let mut result = Value::Undefined;
    for _ in 0..n {
        result = gnata::eval(&arena, root, &input, &env).expect("eval failed");
    }

    let json = result.to_json();
    println!("{}", serde_json::to_string(&json).unwrap_or_default());
}
