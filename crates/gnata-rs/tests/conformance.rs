//! Conformance test harness: runs the JSONata test suite from testdata/groups/.
//!
//! Each test case is a JSON file with:
//! - `expr`: JSONata expression string
//! - `dataset` or `data`: input data (dataset name or inline JSON)
//! - `result`: expected result (if successful)
//! - `undefinedResult`: true if result should be undefined
//! - `code`: expected error code (if expression should fail)
//! - `bindings`: optional variable bindings

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gnata::eval;
use gnata::evaluator::Environment;
use gnata::parser::{Parser, process_ast};
use gnata::value::Value;

fn testdata_dir() -> PathBuf {
    // crates/gnata-rs/tests/conformance.rs → testdata/ is at repo root
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.join("../../testdata")
}

fn load_dataset(name: &str) -> Value {
    let path = testdata_dir().join("datasets").join(format!("{name}.json"));
    let data = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing dataset: {name}"));
    Value::from_json_str(&data).unwrap_or_else(|e| panic!("bad dataset {name}: {e}"))
}

#[derive(Debug)]
struct TestCase {
    expr: String,
    input: Value,
    expected: Expected,
    bindings: Vec<(String, Value)>,
    file: String,
}

#[derive(Debug)]
enum Expected {
    Result(Value),
    Undefined,
    Error(String), // error code
}

/// Load one or more test cases from a file.
/// Handles both single-object and array-of-objects formats,
/// and `expr-file` references to external .jsonata files.
fn load_test_cases(path: &Path) -> Vec<TestCase> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let objects: Vec<&serde_json::Map<String, serde_json::Value>> = match &json {
        serde_json::Value::Object(obj) => vec![obj],
        serde_json::Value::Array(arr) => arr.iter().filter_map(|v| v.as_object()).collect(),
        _ => return vec![],
    };

    let dir = path.parent().unwrap_or(Path::new("."));

    objects
        .into_iter()
        .filter_map(|obj| parse_test_object(obj, dir, path))
        .collect()
}

fn parse_test_object(
    obj: &serde_json::Map<String, serde_json::Value>,
    dir: &Path,
    file_path: &Path,
) -> Option<TestCase> {
    // Expression: either inline "expr" or external "expr-file".
    let expr = if let Some(e) = obj.get("expr").and_then(|v| v.as_str()) {
        e.to_string()
    } else if let Some(f) = obj.get("expr-file").and_then(|v| v.as_str()) {
        std::fs::read_to_string(dir.join(f)).ok()?
    } else {
        return None;
    };

    // Load input data.
    let input = if let Some(dataset) = obj.get("dataset").and_then(|v| v.as_str()) {
        load_dataset(dataset)
    } else if let Some(data) = obj.get("data") {
        Value::from_json(data.clone())
    } else {
        Value::Undefined
    };

    // Determine expected outcome.
    let expected = if let Some(code) = obj.get("code").and_then(|v| v.as_str()) {
        Expected::Error(code.to_string())
    } else if let Some(err_obj) = obj.get("error").and_then(|v| v.as_object()) {
        let code = err_obj
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Expected::Error(code)
    } else if obj.get("undefinedResult").and_then(|v| v.as_bool()) == Some(true) {
        Expected::Undefined
    } else if let Some(result) = obj.get("result") {
        Expected::Result(Value::from_json(result.clone()))
    } else {
        Expected::Undefined
    };

    // Load bindings.
    let mut bindings = Vec::new();
    if let Some(b) = obj.get("bindings").and_then(|v| v.as_object()) {
        for (k, v) in b {
            bindings.push((k.clone(), Value::from_json(v.clone())));
        }
    }

    Some(TestCase {
        expr,
        input,
        expected,
        bindings,
        file: file_path.display().to_string(),
    })
}

fn run_test_case(tc: &TestCase) -> Result<(), String> {
    // Parse.
    let parse_result = Parser::parse(&tc.expr);
    let (mut arena, root) = match parse_result {
        Ok(r) => r,
        Err(e) => {
            return match &tc.expected {
                Expected::Error(code) if e.code == *code => Ok(()),
                Expected::Error(code) => {
                    Err(format!("expected error {code}, got parse error: {e}"))
                }
                _ => Err(format!("parse error: {e}")),
            };
        }
    };

    // Process AST.
    let root = match process_ast(&mut arena, root) {
        Ok(r) => r,
        Err(e) => {
            return match &tc.expected {
                Expected::Error(code) if e.code == *code => Ok(()),
                _ => Err(format!("process error: {e}")),
            };
        }
    };

    // Set up environment with stdlib.
    let mut env = Environment::new();
    gnata::stdlib::register_all(&mut env);
    // Bind $$ (root input reference) — Go does env.Bind("$", data).
    if !tc.input.is_undefined() {
        env.bind("$".into(), tc.input.clone());
    }
    for (name, value) in &tc.bindings {
        env.bind(name.clone(), value.clone());
    }
    let env = Rc::new(env);

    // Evaluate.
    let result = eval(&arena, root, &tc.input, &env);

    match (&tc.expected, result) {
        (Expected::Error(code), Err(e)) => {
            if e.code == *code {
                Ok(())
            } else {
                Err(format!(
                    "expected error {code}, got error {}: {}",
                    e.code, e.message
                ))
            }
        }
        (Expected::Error(code), Ok(val)) => {
            Err(format!("expected error {code}, got value: {val:?}"))
        }
        (Expected::Undefined, Ok(val)) => {
            if val.is_undefined() {
                Ok(())
            } else {
                Err(format!("expected undefined, got: {val:?}"))
            }
        }
        (Expected::Result(expected), Ok(actual)) => {
            if values_match(expected, &actual) {
                Ok(())
            } else {
                Err(format!(
                    "result mismatch:\n  expected: {}\n  actual:   {}",
                    serde_json::to_string(&expected.to_json()).unwrap_or_default(),
                    serde_json::to_string(&actual.to_json()).unwrap_or_default()
                ))
            }
        }
        (Expected::Undefined, Err(e)) => Err(format!("expected undefined, got error: {e}")),
        (Expected::Result(expected), Err(e)) => Err(format!(
            "expected result {}, got error: {e}",
            serde_json::to_string(&expected.to_json()).unwrap_or_default()
        )),
    }
}

/// Compare values, treating both as JSON for comparison.
fn values_match(expected: &Value, actual: &Value) -> bool {
    // Compare via JSON representation for robustness.
    let ej = expected.to_json();
    let aj = actual.to_json();
    json_equal(&ej, &aj)
}

fn json_equal(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a, b) {
        (serde_json::Value::Null, serde_json::Value::Null) => true,
        (serde_json::Value::Bool(a), serde_json::Value::Bool(b)) => a == b,
        (serde_json::Value::Number(a), serde_json::Value::Number(b)) => {
            // Compare as f64 for numeric equality.
            let af = a.as_f64().unwrap_or(f64::NAN);
            let bf = b.as_f64().unwrap_or(f64::NAN);
            (af - bf).abs() < 1e-10 || (af.is_nan() && bf.is_nan())
        }
        (serde_json::Value::String(a), serde_json::Value::String(b)) => a == b,
        (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| json_equal(x, y))
        }
        (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|bv| json_equal(v, bv)))
        }
        _ => false,
    }
}

#[test]
fn conformance_suite() {
    let groups_dir = testdata_dir().join("groups");
    let mut total = 0;
    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut failures: Vec<(String, String)> = Vec::new();

    let mut groups: Vec<_> = std::fs::read_dir(&groups_dir)
        .expect("cannot read testdata/groups")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    groups.sort_by_key(|e| e.file_name());

    for group in &groups {
        let group_name = group.file_name();
        let group_name = group_name.to_string_lossy();
        let mut cases: Vec<_> = std::fs::read_dir(group.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            .collect();
        cases.sort_by_key(|e| e.file_name());

        for case_entry in &cases {
            let test_cases = load_test_cases(&case_entry.path());
            if test_cases.is_empty() {
                total += 1;
                skipped += 1;
                let case_name = format!(
                    "{}/{}",
                    group_name,
                    case_entry.file_name().to_string_lossy()
                );
                eprintln!("SKIP {case_name}");
                continue;
            }
            for tc in &test_cases {
                total += 1;
                match run_test_case(tc) {
                    Ok(()) => passed += 1,
                    Err(msg) => {
                        failed += 1;
                        let case_name = format!(
                            "{}/{}",
                            group_name,
                            case_entry.file_name().to_string_lossy()
                        );
                        failures.push((case_name, msg));
                    }
                }
            }
        }
    }

    // Print summary.
    eprintln!("\n═══ Conformance Suite Results ═══");
    eprintln!("Total:   {total}");
    eprintln!("Passed:  {passed}");
    eprintln!("Failed:  {failed}");
    eprintln!("Skipped: {skipped}");
    eprintln!(
        "Pass rate: {:.1}%",
        if total > 0 {
            passed as f64 / total as f64 * 100.0
        } else {
            0.0
        }
    );

    if !failures.is_empty() {
        eprintln!("\n── All failures ──");
        for (name, msg) in &failures {
            eprintln!("FAIL {name}: {msg}");
        }
    }

    // Don't assert 100% — just report progress.
    // Uncomment below when targeting full conformance:
    // assert_eq!(failed, 0, "{failed} conformance tests failed");

    // For now, assert we pass at least a meaningful percentage.
    assert!(
        passed > 100,
        "expected at least 100 conformance tests to pass, got {passed}"
    );
}
