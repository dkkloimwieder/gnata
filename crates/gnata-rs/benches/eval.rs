//! Benchmark: JSONata evaluation at different payload sizes.
//!
//! Separates parse cost from eval cost. Data is pre-parsed from disk
//! fixtures; only expression evaluation is measured.

use std::rc::Rc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use gnata::evaluator::Environment;
use gnata::expression::Expression;
use gnata::value::Value;

/// Short-key fixtures (original + mixed values). Same expressions apply.
const FIXTURES_SHORT: &[(&str, &str)] = &[
    ("tiny_4", "../../bench/data.json"),
    ("1k", "../../bench/data_1k.json"),
    ("10k", "../../bench/data_10k.json"),
    ("10k_mixed", "../../bench/data_10k_mixed.json"),
    ("100k", "../../bench/data_100k.json"),
    ("100k_mixed", "../../bench/data_100k_mixed.json"),
];

/// Long-key fixtures. Need different expressions matching long field names.
const FIXTURES_LONG: &[(&str, &str)] = &[
    ("10k_long", "../../bench/data_10k_long.json"),
    ("100k_long", "../../bench/data_100k_long.json"),
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
    let short_expressions: &[(&str, &str)] = &[
        ("path_simple", "Account.Name"),
        ("path_nested", "Account.Order.Product.SKU"),
        ("filter", "Account.Order.Product[UnitPrice > 50].SKU"),
        ("aggregation", "$sum(Account.Order.Product.(UnitPrice * Quantity * (1 - Discount)))"),
        ("comparison", "Account.Name = \"Firefly\""),
        ("func_count", "$count(Account.Order.Product.SKU)"),
    ];

    let long_expressions: &[(&str, &str)] = &[
        ("path_nested", "Account.Order.Product.StockKeepingUnitIdentifier"),
        ("filter", "Account.Order.Product[UnitPriceWithTaxIncluded > 50].StockKeepingUnitIdentifier"),
        ("aggregation", "$sum(Account.Order.Product.(UnitPriceWithTaxIncluded * QuantityInWarehouseStock * (1 - ApplicableDiscountPercentage)))"),
        ("func_count", "$count(Account.Order.Product.StockKeepingUnitIdentifier)"),
    ];

    // Short-key fixtures with short expressions
    for &(expr_name, expr_src) in short_expressions {
        let mut group = c.benchmark_group(format!("eval/{expr_name}"));
        let compiled = Expression::compile(expr_src).unwrap();

        for &(size_name, path) in FIXTURES_SHORT {
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

    // Long-key fixtures with long expressions
    for &(expr_name, expr_src) in long_expressions {
        let mut group = c.benchmark_group(format!("eval_long/{expr_name}"));
        let compiled = Expression::compile(expr_src).unwrap();

        for &(size_name, path) in FIXTURES_LONG {
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

    // Short-key fixtures
    let compiled_short = Expression::compile("Account.Order.Product.SKU").unwrap();
    for &(size_name, path) in FIXTURES_SHORT {
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
                    let result = compiled_short.evaluate_with_env(&value, &env).unwrap();
                    criterion::black_box(result);
                });
            },
        );
    }

    // Long-key fixtures
    let compiled_long = Expression::compile("Account.Order.Product.StockKeepingUnitIdentifier").unwrap();
    for &(size_name, path) in FIXTURES_LONG {
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
                    let result = compiled_long.evaluate_with_env(&value, &env).unwrap();
                    criterion::black_box(result);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_eval, bench_end_to_end);
criterion_main!(benches);
