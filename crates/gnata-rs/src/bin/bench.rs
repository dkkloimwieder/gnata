// Benchmark CLI for Rust JSONata.
// Usage: gnata-bench -expr 'Account.Name' -data '{"Account":{"Name":"Firefly"}}' [-n 1000]
//        gnata-bench -stream -datafile data.json -n 1000   (evaluates 4 expressions per iter)

#[cfg(all(feature = "mimalloc-alloc", not(target_arch = "wasm32")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use gnata::expression::Expression;
use gnata::value::Value;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut expr_str = "";
    let mut data_str = String::from("{}");
    let mut n: u64 = 1;
    let mut stream_mode = false;

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
            "-stream" => { stream_mode = true; i += 1; }
            _ => { i += 1; }
        }
    }

    if stream_mode {
        run_stream_bench(&data_str, n);
    } else {
        run_single_bench(expr_str, &data_str, n);
    }
}

fn run_single_bench(expr_str: &str, data_str: &str, n: u64) {
    if expr_str.is_empty() {
        eprintln!("usage: gnata-bench -expr EXPR [-data JSON | -datafile FILE] [-n ITERS]");
        std::process::exit(1);
    }

    let compiled = Expression::compile(expr_str).expect("compile failed");
    let input = Value::from_json_str(data_str).unwrap_or(Value::Undefined);

    // Build environment ONCE and reuse across all iterations.
    // evaluate_value() rebuilds the env every call — 60+ stdlib registrations
    // per iteration was the source of a 30x regression on small payloads.
    let env = gnata::expression::new_custom_env(&[]);
    if !input.is_undefined() {
        env.bind("$", input.clone());
    }

    let mut result = Value::Undefined;
    for _ in 0..n {
        result = compiled.evaluate_with_env(&input, &env).expect("eval failed");
    }

    let json = result.to_json();
    println!("{}", serde_json::to_string(&json).unwrap_or_default());
}

fn run_stream_bench(data_str: &str, n: u64) {
    use gnata::StreamEvaluator;

    let exprs = [
        "Account.Name",
        "Account.Order.Product.SKU",
        "Account.Order.Product[UnitPrice > 50].SKU",
        "$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))",
    ];

    let mut se = StreamEvaluator::new(Vec::new());
    let indices: Vec<usize> = exprs
        .iter()
        .map(|e| se.compile(e).expect("compile failed"))
        .collect();
    let input = Value::from_json_str(data_str).unwrap_or(Value::Undefined);

    let mut results = Vec::new();
    for _ in 0..n {
        results = se.eval_many(&input, &indices).expect("eval failed");
    }
    println!("{} expressions, {} results", exprs.len(), results.len());
}
