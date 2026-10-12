//! CRLF adaptation for edit_file (`crlf_adapted_edit` + its wiring in
//! `SmartEditTool::execute`). Every assertion is on bytes.
//!
//! The before case: a file `alpha\r\nbeta\r\ngamma\r\n`, an old_text the model
//! wrote as `alpha\nbeta` (it never sees `\r`). yoagent's edit_file matches
//! literally, so without adaptation this is `old_text not found`.

use super::*;
use yoagent::tools::edit::EditFileTool;

fn ctx() -> yoagent::types::ToolContext {
    yoagent::types::ToolContext::new("test", "test")
}

fn real_edit_tool() -> Box<dyn AgentTool> {
    with_smart_edit(Box::new(EditFileTool::new()))
}

fn write_tmp(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> String {
    let p = dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    p.to_string_lossy().into_owned()
}

fn result_text(r: &yoagent::types::ToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| match c {
            yoagent::Content::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

// ── End to end through the real yoagent edit_file ──────────────────────────

#[tokio::test]
async fn crlf_file_multiline_lf_old_text_applies_and_keeps_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(&dir, "g.txt", b"alpha\r\nbeta\r\ngamma\r\n");
    let params = serde_json::json!({
        "path": path,
        "old_text": "alpha\nbeta",
        "new_text": "ALPHA\nBETA",
    });
    let res = real_edit_tool().execute(params, ctx()).await;
    assert!(res.is_ok(), "CRLF edit should apply, got: {res:?}");
    assert_eq!(std::fs::read(&path).unwrap(), b"ALPHA\r\nBETA\r\ngamma\r\n");
}

#[tokio::test]
async fn crlf_file_new_text_with_more_lines_gets_crlf_on_every_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(&dir, "g.txt", b"alpha\r\nbeta\r\ngamma\r\n");
    let params = serde_json::json!({
        "path": path,
        "old_text": "alpha\nbeta",
        "new_text": "one\ntwo\nthree\nfour",
    });
    let res = real_edit_tool().execute(params, ctx()).await;
    assert!(res.is_ok(), "CRLF edit should apply, got: {res:?}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"one\r\ntwo\r\nthree\r\nfour\r\ngamma\r\n"
    );
}

#[tokio::test]
async fn lf_file_edit_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(&dir, "l.txt", b"alpha\nbeta\ngamma\n");
    let params = serde_json::json!({
        "path": path,
        "old_text": "alpha\nbeta",
        "new_text": "ALPHA\nBETA",
    });
    real_edit_tool().execute(params, ctx()).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"ALPHA\nBETA\ngamma\n");
}

#[tokio::test]
async fn failing_edit_on_crlf_file_names_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(&dir, "g.txt", b"alpha\r\nbeta\r\ngamma\r\n");
    let params = serde_json::json!({
        "path": path,
        "old_text": "zeta\nomega",
        "new_text": "x",
    });
    let err = match real_edit_tool().execute(params, ctx()).await {
        Err(yoagent::types::ToolError::Failed(m)) => m,
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(err.contains("not found"), "anti-vacuous: {err}");
    assert!(
        err.contains(CRLF_NOT_FOUND_NOTE),
        "CRLF note missing: {err}"
    );
    // The file is untouched.
    assert_eq!(std::fs::read(&path).unwrap(), b"alpha\r\nbeta\r\ngamma\r\n");
}

#[tokio::test]
async fn failing_edit_on_lf_file_does_not_name_crlf() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(&dir, "l.txt", b"alpha\nbeta\ngamma\n");
    let params = serde_json::json!({
        "path": path,
        "old_text": "zeta\nomega",
        "new_text": "x",
    });
    let err = match real_edit_tool().execute(params, ctx()).await {
        Err(yoagent::types::ToolError::Failed(m)) => m,
        other => panic!("expected Failed, got {other:?}"),
    };
    assert!(err.contains("not found"), "anti-vacuous: {err}");
    assert!(
        !err.contains("CRLF"),
        "LF file must not mention CRLF: {err}"
    );
}

#[tokio::test]
async fn crlf_success_result_is_unannotated_edit_output() {
    // The adapted edit reports like any successful edit (no error text).
    let dir = tempfile::tempdir().unwrap();
    let path = write_tmp(&dir, "g.txt", b"alpha\r\nbeta\r\n");
    let params = serde_json::json!({
        "path": path,
        "old_text": "alpha\nbeta",
        "new_text": "x\ny",
    });
    let r = real_edit_tool().execute(params, ctx()).await.unwrap();
    assert!(!result_text(&r).contains("not found"));
    assert_eq!(std::fs::read(&path).unwrap(), b"x\r\ny\r\n");
}

// ── The pure function: the whole regression surface ────────────────────────

#[test]
fn adapts_bare_lf_old_and_new_on_crlf_file() {
    assert_eq!(
        crlf_adapted_edit("alpha\r\nbeta\r\ngamma\r\n", "alpha\nbeta", "A\nB\nC"),
        Some(("alpha\r\nbeta".to_string(), "A\r\nB\r\nC".to_string()))
    );
}

#[test]
fn lf_file_is_never_adapted() {
    assert_eq!(
        crlf_adapted_edit("alpha\nbeta\ngamma\n", "alpha\nbeta", "A\nB"),
        None
    );
}

#[test]
fn single_line_edit_on_crlf_file_is_not_adapted() {
    assert_eq!(
        crlf_adapted_edit("line one\r\nline two\r\n", "line two", "line 2"),
        None
    );
}

#[test]
fn old_text_already_crlf_is_literal_and_never_doubled() {
    assert_eq!(
        crlf_adapted_edit("alpha\r\nbeta\r\n", "alpha\r\nbeta", "A\r\nB"),
        None
    );
    // Even when the literal fails, an existing \r\n is not turned into \r\r\n:
    // a mixed old_text converts only its bare \n.
    assert_eq!(
        crlf_adapted_edit("a\r\nb\r\nc\r\n", "a\r\nb\nc", "x\r\ny\nz"),
        Some(("a\r\nb\r\nc".to_string(), "x\r\ny\r\nz".to_string()))
    );
}

#[test]
fn mixed_file_literal_lf_match_wins() {
    let file = "w\r\nx\r\nalpha\nbeta\n";
    assert!(file.contains("alpha\nbeta"), "anti-vacuous fixture");
    assert_eq!(crlf_adapted_edit(file, "alpha\nbeta", "A\nB"), None);
}

#[test]
fn no_match_even_with_crlf_returns_none() {
    assert_eq!(
        crlf_adapted_edit("alpha\r\nbeta\r\n", "zeta\nomega", "x"),
        None
    );
}

#[test]
fn params_pass_through_byte_identical_on_lf_file() {
    let params = serde_json::json!({
        "path": "unused",
        "old_text": "alpha\nbeta",
        "new_text": "A\nB",
    });
    assert_eq!(
        crlf_adapted_params(&params, "alpha\nbeta\ngamma\n"),
        None,
        "LF file: no rewrite, so the inner tool sees the caller's params"
    );
}
