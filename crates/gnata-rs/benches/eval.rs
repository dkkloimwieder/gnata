//! Benchmark: JSONata evaluation at different payload sizes.
//!
//! Separates parse cost from eval cost. Data is pre-parsed from disk
//! fixtures; only expression evaluation is measured.

use std::rc::Rc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use gnata::evaluator::Environment;
use gnata::expression::Expression;
use gnata::value::Value;

const FIXTURES: &[(&str, &str)] = &[
    ("tiny_4", "../../bench/data.json"),
    ("1k", "../../bench/data_1k.json"),
    ("10k", "../../bench/data_10k.json"),
    ("100k", "../../bench/data_100k.json"),
];

/// Pre-parsed input ready for evaluation.
struct PreparedInput {
    value: Value,
    env: Rc<Environment>,
}

impl PreparedInput {
    fn from_file(path: &str) -> Self {
        let json_str = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let value = Value::from_json_str(&json_str).unwrap();
        let mut env = Environment::new();
        gnata::stdlib::register_all(&mut env);
        env.bind("$".into(), value.clone());
        let env = Rc::new(env);
        PreparedInput { value, env }
    }
}

fn bench_eval(c: &mut Criterion) {
    let expressions: &[(&str, &str)] = &[
        ("path_simple", "Account.Name"),
        ("path_nested", "Account.Order.Product.SKU"),
        ("filter", "Account.Order.Product[UnitPrice > 50].SKU"),
        ("aggregation", "$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))"),
        ("comparison", "Account.Name = \"Firefly\""),
        ("func_count", "$count(Account.Order.Product.SKU)"),
    ];

    for &(expr_name, expr_src) in expressions {
        let mut group = c.benchmark_group(format!("eval/{expr_name}"));
        let compiled = Expression::compile(expr_src).unwrap();

        for &(size_name, path) in FIXTURES {
            let input = PreparedInput::from_file(path);

            group.bench_with_input(
                BenchmarkId::new("rust", size_name),
                &(),
                |b, _| {
                    b.iter(|| {
                        let result = compiled.evaluate_with_env(&input.value, &input.env).unwrap();
                        criterion::black_box(result);
                    });
                },
            );
        }

        group.finish();
    }
}

/// Benchmark the full pipeline (parse + eval) to show end-to-end cost.
fn bench_end_to_end(c: &mut Criterion) {
    let mut group = c.benchmark_group("end_to_end");
    let compiled = Expression::compile("Account.Order.Product.SKU").unwrap();

    for &(size_name, path) in FIXTURES {
        let json_str = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));

        group.bench_with_input(
            BenchmarkId::new("parse_and_eval", size_name),
            &json_str,
            |b, data| {
                b.iter(|| {
                    let value = Value::from_json_str(data).unwrap();
                    let mut env = Environment::new();
                    gnata::stdlib::register_all(&mut env);
                    env.bind("$".into(), value.clone());
                    let env = Rc::new(env);
                    let result = compiled.evaluate_with_env(&value, &env).unwrap();
                    criterion::black_box(result);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_eval, bench_end_to_end);
criterion_main!(benches);
