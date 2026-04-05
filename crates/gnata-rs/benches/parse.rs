//! Benchmark: JSON parsing into gnata::Value at different payload sizes.
//!
//! Measures the cost of serde_json → Value conversion, which dominates
//! wall-clock time for large payloads. Fixtures live in bench/data*.json.

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use gnata::value::Value;

const FIXTURES: &[(&str, &str)] = &[
    ("tiny_4", "../../bench/data.json"),
    ("1k", "../../bench/data_1k.json"),
    ("10k", "../../bench/data_10k.json"),
    ("100k", "../../bench/data_100k.json"),
];

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse");

    for &(name, path) in FIXTURES {
        let json_str = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {path}: {e}"));
        let bytes = json_str.len() as u64;

        group.throughput(Throughput::Bytes(bytes));

        group.bench_with_input(
            BenchmarkId::new("json_to_value", format!("{name}_{bytes}B")),
            &json_str,
            |b, data| {
                b.iter(|| {
                    let v = Value::from_json_str(data).unwrap();
                    criterion::black_box(v);
                });
            },
        );

        // Raw serde_json::Value as baseline to isolate our Value conversion cost
        group.bench_with_input(
            BenchmarkId::new("json_to_serde", format!("{name}_{bytes}B")),
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
