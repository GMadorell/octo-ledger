use rust_decimal::Decimal;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

mod scale;

type Row = (String, String, String, String);

#[test]
fn happy_path_produces_expected_balances() {
    let stdout = run_and_capture(&example_path("happy_path.csv"));
    let rows = parse_rows(&stdout);

    assert_eq!(
        rows.len(),
        2,
        "expected exactly 2 client rows, got {:?}",
        rows
    );
    assert_balance(&rows, "1", "1.5", "0", "1.5", "false");
    assert_balance(&rows, "2", "2", "0", "2", "false");
}

#[test]
fn malformed_input_yields_graceful_error_not_panic() {
    let output = Command::new(bin_path())
        .arg(example_path("malformed.csv"))
        .output()
        .expect("failed to execute octo-ledger binary");

    assert!(
        !output.status.success(),
        "expected a failure exit status for malformed input"
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid UTF-8");
    assert!(
        stdout.is_empty(),
        "expected no partial output on stdout when an error occurs, got: {:?}",
        stdout
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be valid UTF-8");
    assert!(
        stderr.contains("error:"),
        "expected stderr to contain a typed error message, got: {:?}",
        stderr
    );
    assert!(
        !stderr.contains("thread 'main' panicked"),
        "expected a graceful typed error, not a panic; stderr: {:?}",
        stderr
    );
}

#[test]
fn dispute_places_funds_on_hold_for_client_1() {
    let stdout = run_and_capture(&example_path("dispute.csv"));
    let rows = parse_rows(&stdout);
    assert_balance(&rows, "1", "0", "5", "5", "false");
}

#[test]
fn resolve_returns_disputed_funds_for_client_1() {
    let stdout = run_and_capture(&example_path("resolve.csv"));
    let rows = parse_rows(&stdout);
    assert_balance(&rows, "1", "5", "0", "5", "false");
}

#[test]
fn chargeback_zeroes_and_locks_client_1() {
    let stdout = run_and_capture(&example_path("chargeback.csv"));
    let rows = parse_rows(&stdout);
    assert_balance(&rows, "1", "0", "0", "0", "true");
}

#[test]
fn edge_cases_prove_each_ignore_path_had_no_effect() {
    let stdout = run_and_capture(&example_path("edge_cases.csv"));
    let rows = parse_rows(&stdout);

    assert_balance(&rows, "10", "5", "0", "5", "false");
    assert!(
        !rows.contains_key("11"),
        "client 11 should have no row since its only mention is a mismatched dispute: {rows:?}"
    );

    assert_balance(&rows, "20", "0", "5", "5", "false");
    assert_balance(&rows, "30", "5", "0", "5", "false");
    assert_balance(&rows, "40", "7", "0", "7", "false");
    assert_balance(&rows, "50", "0", "0", "0", "true");
    assert_balance(&rows, "60", "0", "0", "0", "false");

    assert_eq!(
        rows.len(),
        6,
        "expected exactly 6 client rows (10, 20, 30, 40, 50, 60), got {rows:?}"
    );
}

#[test]
fn scale_test_50k_rows_preserve_available_plus_held_invariant() {
    let csv = scale::build_scale_csv();

    let mut file = tempfile::NamedTempFile::new().expect("failed to create temp scale csv");
    file.write_all(csv.as_bytes())
        .expect("failed to write temp scale csv");

    let stdout = run_and_capture(file.path());
    let rows = parse_rows(&stdout);

    assert!(
        rows.len() >= scale::SCALE_BULK_CLIENTS as usize,
        "expected at least {} bulk client accounts, got {}",
        scale::SCALE_BULK_CLIENTS,
        rows.len()
    );

    for (client, (available, held, total, _locked)) in &rows {
        let available: Decimal = available
            .parse()
            .unwrap_or_else(|e| panic!("bad available for client {client}: {e}"));
        let held: Decimal = held
            .parse()
            .unwrap_or_else(|e| panic!("bad held for client {client}: {e}"));
        let total: Decimal = total
            .parse()
            .unwrap_or_else(|e| panic!("bad total for client {client}: {e}"));
        assert_eq!(
            available + held,
            total,
            "invariant available + held == total violated for client {client}"
        );
    }

    assert_balance_decimal(&rows, "9001", "120.00", "0", "120.00", "false");
    assert_balance_decimal(&rows, "9002", "0", "0", "0", "true");
}

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_octo-ledger")
}

fn example_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

fn run_and_capture(path: &Path) -> String {
    let output = Command::new(bin_path())
        .arg(path)
        .output()
        .expect("failed to execute octo-ledger binary");

    assert!(
        output.status.success(),
        "expected success exit status, got {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout).expect("stdout should be valid UTF-8")
}

fn parse_rows(stdout: &str) -> HashMap<String, Row> {
    let mut lines = stdout.lines();

    let header = lines.next().expect("stdout should have a header line");
    assert_eq!(header, "client,available,held,total,locked");

    let mut rows: HashMap<String, Row> = HashMap::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        assert_eq!(
            fields.len(),
            5,
            "expected 5 columns in row {:?}, got {:?}",
            line,
            fields
        );
        rows.insert(
            fields[0].to_string(),
            (
                fields[1].to_string(),
                fields[2].to_string(),
                fields[3].to_string(),
                fields[4].to_string(),
            ),
        );
    }
    rows
}

fn get_row<'a>(rows: &'a HashMap<String, Row>, client: &str) -> &'a Row {
    rows.get(client)
        .unwrap_or_else(|| panic!("expected a row for client {client}, got rows: {rows:?}"))
}

fn assert_balance(
    rows: &HashMap<String, Row>,
    client: &str,
    available: &str,
    held: &str,
    total: &str,
    locked: &str,
) {
    let row = get_row(rows, client);
    assert_eq!(row.0, available, "client {client} available");
    assert_eq!(row.1, held, "client {client} held");
    assert_eq!(row.2, total, "client {client} total");
    assert_eq!(row.3, locked, "client {client} locked");
}

fn assert_balance_decimal(
    rows: &HashMap<String, Row>,
    client: &str,
    available: &str,
    held: &str,
    total: &str,
    locked: &str,
) {
    let row = get_row(rows, client);
    let parse = |s: &str| -> Decimal {
        s.parse()
            .unwrap_or_else(|e| panic!("failed to parse {s:?} as a decimal: {e}"))
    };
    assert_eq!(parse(&row.0), parse(available), "client {client} available");
    assert_eq!(parse(&row.1), parse(held), "client {client} held");
    assert_eq!(parse(&row.2), parse(total), "client {client} total");
    assert_eq!(row.3, locked, "client {client} locked");
}
