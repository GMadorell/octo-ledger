use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn bin_path() -> &'static str {
    env!("CARGO_BIN_EXE_octo-ledger")
}

fn example_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(name)
}

type Row = (String, String, String, String);

#[test]
fn happy_path_produces_expected_balances() {
    let output = Command::new(bin_path())
        .arg(example_path("happy_path.csv"))
        .output()
        .expect("failed to execute octo-ledger binary");

    assert!(
        output.status.success(),
        "expected success exit status, got {:?}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be valid UTF-8");
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
        let client = fields[0].to_string();
        let row = (
            fields[1].to_string(),
            fields[2].to_string(),
            fields[3].to_string(),
            fields[4].to_string(),
        );
        rows.insert(client, row);
    }

    assert_eq!(
        rows.len(),
        2,
        "expected exactly 2 client rows, got {:?}",
        rows
    );

    let client_1 = rows.get("1").expect("expected a row for client 1");
    assert_eq!(client_1.0, "1.5", "client 1 available");
    assert_eq!(client_1.1, "0", "client 1 held");
    assert_eq!(client_1.2, "1.5", "client 1 total");
    assert_eq!(client_1.3, "false", "client 1 locked");

    let client_2 = rows.get("2").expect("expected a row for client 2");
    assert_eq!(client_2.0, "2", "client 2 available");
    assert_eq!(client_2.1, "0", "client 2 held");
    assert_eq!(client_2.2, "2", "client 2 total");
    assert_eq!(client_2.3, "false", "client 2 locked");
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
