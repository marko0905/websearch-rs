//! CLI parity: our binary must print exactly what the upstream `ddgs` CLI
//! prints (stdout, stderr, exit code) for every case in
//! `fixtures/cli/reference.json` (made by `scripts/regen_cli_reference.py`
//! from ddgs 9.16.1). The binary runs through a symlink named `ddgs`, so the
//! program name in usage lines matches. No case touches the network.

#![cfg(unix)]

use std::process::{Command, Stdio};

#[derive(serde::Deserialize)]
struct Case {
    args: Vec<String>,
    stdout: String,
    stderr: String,
    code: i32,
}

#[test]
fn cli_matches_upstream_reference() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("../fixtures/cli/reference.json"))
        .expect("reference json");
    let dir = std::env::temp_dir().join(format!("websearch-cli-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let link = dir.join("ddgs");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_websearch"), &link).expect("symlink");

    let mut failures = Vec::new();
    for case in &cases {
        let output = Command::new(&link)
            .args(&case.args)
            .env("COLUMNS", "80")
            .env_remove("DDGS_PROXY")
            .stdin(Stdio::null())
            .output()
            .expect("run binary");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let code = output.status.code().unwrap_or(-1);
        if stdout != case.stdout || stderr != case.stderr || code != case.code {
            failures.push(format!(
                "args {:?}\n  code {code} (expected {})\n  stdout {stdout:?}\n  expect {:?}\n  stderr {stderr:?}\n  expect {:?}",
                case.args, case.code, case.stdout, case.stderr
            ));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}
