//! Benchmark: JSON parsing into gnata::Value at different payload sizes.
//!
//! Compares three parsers:
//!   1. simd-json → Value (current default via from_json_str)
//!   2. serde_json → Value (direct Visitor, no intermediate tree)
//!   3. serde_json → serde_json::Value (baseline)
//!
//! Fixtures live in bench/data*.json.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use gnata::value::Value;

const FIXTURES: &[(&str, &str)] = &[
    ("tiny_4", "../../bench/data.json"),
    ("1k", "../../bench/data_1k.json"),
    ("10k", "../../bench/data_10k.json"),
    ("10k_long", "../../bench/data_10k_long.json"),
    ("10k_mixed", "../../bench/data_10k_mixed.json"),
    ("100k", "../../bench/data_100k.json"),
    ("100k_long", "../../bench/data_100k_long.json"),
    ("100k_mixed", "../../bench/data_100k_mixed.json"),
];

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    for &(name, path) in FIXTURES {
        let json_str = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let bytes = json_str.len() as u64;

        group.throughput(Throughput::Bytes(bytes));

        // 1. simd-json → gnata::Value (current default)
        group.bench_with_input(
            BenchmarkId::new("simd_to_value", format!("{name}_{bytes}B")),
            &json_str,
            |b, data| {
                b.iter(|| {
                    let v = Value::from_json_str(data).unwrap();
                    criterion::black_box(v);
                });
            },
        );

        // 2. serde_json → gnata::Value (direct Visitor, no intermediate tree)
        group.bench_with_input(
            BenchmarkId::new("serde_to_value", format!("{name}_{bytes}B")),
            &json_str,
            |b, data| {
                b.iter(|| {
                    let v: Value = serde_json::from_str(data).unwrap();
                    criterion::black_box(v);
                });
            },
        );

        // 3. serde_json → serde_json::Value (baseline)
        group.bench_with_input(
            BenchmarkId::new("serde_to_serde", format!("{name}_{bytes}B")),
            &json_str,
            |b, data| {
                b.iter(|| {
                    let v: serde_json::Value = serde_json::from_str(data).unwrap();
                    criterion::black_box(v);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
