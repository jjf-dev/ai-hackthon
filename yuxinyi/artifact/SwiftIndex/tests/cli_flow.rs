use std::fs;
use std::path::Path;

use anyhow::Result;
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn minimal_workspace_build_and_query_flow() -> Result<()> {
    let fixture = create_workspace_fixture()?;
    let db_path = fixture.path().join("index.db");

    run_ok(&[
        "build",
        "--path",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;

    let symbol_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "symbol",
        "add",
    ])?;
    let symbol_json: Value = serde_json::from_str(&symbol_output)?;
    assert_eq!(symbol_json["items"][0]["qualname"], "mini_ws::add");
    assert_eq!(symbol_json["items"].as_array().map(Vec::len), Some(1));
    assert!(symbol_json["confidence"]
        .as_f64()
        .is_some_and(|value| value > 0.9));
    assert!(symbol_json["items"][0]["why"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let symbol_verbose_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "--compact",
        "false",
        "symbol",
        "add",
    ])?;
    let symbol_verbose_json: Value = serde_json::from_str(&symbol_verbose_output)?;
    assert_eq!(symbol_verbose_json[0]["qualname"], "mini_ws::add");

    let file_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "file",
        "lib.rs",
    ])?;
    let file_json: Value = serde_json::from_str(&file_output)?;
    assert_eq!(file_json["items"][0]["path"], "src/lib.rs");
    assert!(file_json["items"]
        .as_array()
        .is_some_and(|items| items.len() <= 3));
    assert!(file_json["items"][0]["why"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let outline_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "outline",
        "src/lib.rs",
    ])?;
    let outline_json: Value = serde_json::from_str(&outline_output)?;
    assert!(outline_json["items"][0]["top_level_symbols"]
        .as_array()
        .is_some_and(|items| items
            .iter()
            .any(|item| item.as_str() == Some("fn add [1-3]"))));
    assert!(outline_json["items"][0]["why"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let read_symbol_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "read-symbol",
        "mini_ws::add",
    ])?;
    let read_symbol_json: Value = serde_json::from_str(&read_symbol_output)?;
    assert!(read_symbol_json["content"]
        .as_str()
        .is_some_and(|content| content.contains("pub fn add")));
    assert!(read_symbol_json["why"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let snippet_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "snippet",
        "--path",
        "src/lib.rs",
        "--start",
        "1",
        "--end",
        "4",
    ])?;
    let snippet_json: Value = serde_json::from_str(&snippet_output)?;
    assert!(snippet_json["content"]
        .as_str()
        .is_some_and(|content| content.contains("pub fn add")));
    assert!(snippet_json["why"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let neighbors_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "neighbors",
        "mini_ws::add",
    ])?;
    let neighbors_json: Value = serde_json::from_str(&neighbors_output)?;
    assert!(neighbors_json["items"][0]["related_tests"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(neighbors_json["items"][0]["why"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let entrypoints_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "entrypoints",
    ])?;
    let entrypoints_json: Value = serde_json::from_str(&entrypoints_output)?;
    assert!(entrypoints_json
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let explain_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "explain",
        "add flow",
    ])?;
    let explain_json: Value = serde_json::from_str(&explain_output)?;
    assert!(explain_json["items"][0]["top_symbols"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(explain_json["items"][0]["next_steps"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let suggest_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "suggest",
        "fix add logic",
    ])?;
    let suggest_json: Value = serde_json::from_str(&suggest_output)?;
    assert!(suggest_json["items"][0]["symbols"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
    assert!(suggest_json["items"][0]["tests"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));

    let expanded_symbol_output = run_ok_capture(&[
        "query",
        "--workspace",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
        "--json",
        "--expand",
        "2",
        "symbol",
        "ad",
    ])?;
    let expanded_symbol_json: Value = serde_json::from_str(&expanded_symbol_output)?;
    assert!(expanded_symbol_json["items"]
        .as_array()
        .is_some_and(|items| items.len() <= 8 && !items.is_empty()));

    let stats_output = run_ok_capture(&[
        "stats",
        "--path",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;
    assert!(stats_output.contains("Files:"));
    assert!(stats_output.contains("Symbols:"));

    let update_output = run_ok_capture(&[
        "update",
        "--path",
        fixture.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ])?;
    assert!(update_output.contains("Unchanged:"));

    Ok(())
}

fn create_workspace_fixture() -> Result<TempDir> {
    let temp = TempDir::new()?;
    write_file(
        temp.path().join("Cargo.toml"),
        r#"[package]
name = "mini_ws"
version = "0.1.0"
edition = "2021"
"#,
    )?;
    write_file(
        temp.path().join("src/lib.rs"),
        r#"pub fn add(a: i32, b: i32) -> i32 {
    add_impl(a, b)
}

fn add_impl(left: i32, right: i32) -> i32 {
    left + right
}

fn adapter_add(value: i32) -> i32 {
    add(value, 1)
}

#[cfg(test)]
mod tests {
    use super::add;

    #[test]
    fn test_add() {
        assert_eq!(add(1, 2), 3);
    }
}
"#,
    )?;
    write_file(
        temp.path().join("src/main.rs"),
        r#"use clap::Parser;

#[derive(Parser)]
struct Cli {
    value: i32,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    tokio::spawn(async move {
        let _ = cli.value;
    });
}
"#,
    )?;
    Ok(temp)
}

fn write_file(path: impl AsRef<Path>, content: &str) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn run_ok(args: &[&str]) -> Result<()> {
    let mut command = assert_cmd::Command::cargo_bin("repo-index")?;
    command.args(args);
    command.assert().success();
    Ok(())
}

fn run_ok_capture(args: &[&str]) -> Result<String> {
    let mut command = assert_cmd::Command::cargo_bin("repo-index")?;
    command.args(args);
    let output = command.assert().success().get_output().stdout.clone();
    Ok(String::from_utf8(output)?)
}
