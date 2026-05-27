//! Integration tests for deep-research application.
//!
//! Tests cover tool functionality, security validations, error handling,
//! and state initialization using `MockChatModel` where applicable.

#![allow(
    clippy::uninlined_format_args,
    reason = "Test assertions use format strings for clarity in failure messages"
)]

use juncture::llm::{ChatModel, Message, MockChatModel, ToolCall};
use juncture::tools::Tool;
use juncture_core::store::MemoryStore;

// Import from the library being tested
use deep_research::ResearchState;
use deep_research::tools::{Calculator, MemorySearch, ReadFile, WebSearch};

// ---------------------------------------------------------------------------
// Calculator Tool Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_calculator_tool() {
    let calc = Calculator::new();
    let result = calc
        .invoke(serde_json::json!({"expression": "2 + 3"}))
        .await;

    assert!(
        result.is_ok(),
        "Calculator should successfully evaluate '2 + 3': {result:?}"
    );
    assert_eq!(result.unwrap(), "5");
    // Calculator result is straightforward arithmetic
}

#[tokio::test]
async fn test_calculator_division_by_zero() {
    let calc = Calculator::new();
    let result = calc
        .invoke(serde_json::json!({"expression": "1 / 0"}))
        .await;

    assert!(
        result.is_err(),
        "Calculator should return error for division by zero: {:?}",
        result
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("Division by zero") || err_msg.contains("division by zero"),
        "Error message should mention division by zero: {err_msg}"
    );
}

#[tokio::test]
async fn test_calculator_complex_expression() {
    let calc = Calculator::new();
    let result = calc
        .invoke(serde_json::json!({"expression": "10 + 2 * 5 - 3"}))
        .await;

    assert!(
        result.is_ok(),
        "Calculator should evaluate complex expression: {:?}",
        result
    );
    // Calculator evaluates left-to-right without operator precedence:
    // ((10 + 2) * 5) - 3 = (12 * 5) - 3 = 60 - 3 = 57
    assert_eq!(result.unwrap(), "57");
    // Calculator result is straightforward arithmetic
}

#[tokio::test]
async fn test_calculator_invalid_expression() {
    let calc = Calculator::new();
    let result = calc
        .invoke(serde_json::json!({"expression": "invalid + expression"}))
        .await;

    assert!(
        result.is_err(),
        "Calculator should return error for invalid expression: {:?}",
        result
    );
}

#[tokio::test]
async fn test_calculator_missing_parameter() {
    let calc = Calculator::new();
    let result = calc.invoke(serde_json::json!({})).await;

    assert!(
        result.is_err(),
        "Calculator should return error when 'expression' parameter is missing: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// File I/O Tool Security Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_file_rejects_traversal() {
    let reader = ReadFile::new();
    let result = reader
        .invoke(serde_json::json!({"path": "../../../etc/passwd"}))
        .await;

    assert!(
        result.is_err(),
        "ReadFile should reject path traversal attacks: {:?}",
        result
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("traversal") || err_msg.contains("outside"),
        "Error message should mention path traversal restriction: {err_msg}"
    );
}

#[tokio::test]
async fn test_read_file_rejects_absolute_path() {
    let reader = ReadFile::new();
    let result = reader
        .invoke(serde_json::json!({"path": "/etc/passwd"}))
        .await;

    assert!(
        result.is_err(),
        "ReadFile should reject absolute paths: {:?}",
        result
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("Absolute") || err_msg.contains("relative"),
        "Error message should mention absolute path restriction: {err_msg}"
    );
}

#[tokio::test]
async fn test_read_file_missing_parameter() {
    let reader = ReadFile::new();
    let result = reader.invoke(serde_json::json!({})).await;

    assert!(
        result.is_err(),
        "ReadFile should return error when 'path' parameter is missing: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// Web Search Tool Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_web_search_without_api_key() {
    let search = WebSearch::new(None);
    let result = search.invoke(serde_json::json!({"query": "test"})).await;

    assert!(
        result.is_err(),
        "WebSearch should return error when API key is not configured: {:?}",
        result
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("API_KEY")
            || err_msg.contains("api_key")
            || err_msg.contains("configured"),
        "Error message should mention missing API key: {err_msg}"
    );
}

#[tokio::test]
async fn test_web_search_missing_query() {
    let search = WebSearch::new(Some("fake-key".to_string()));
    let result = search.invoke(serde_json::json!({})).await;

    assert!(
        result.is_err(),
        "WebSearch should return error when 'query' parameter is missing: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// Memory Search Tool Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_memory_search_without_store() {
    let search = MemorySearch::new(None);
    let result = search.invoke(serde_json::json!({"query": "test"})).await;

    assert!(
        result.is_err(),
        "MemorySearch should return error when store is not configured: {:?}",
        result
    );

    let err = result.unwrap_err();
    let err_msg = err.to_string();
    assert!(
        err_msg.contains("store") || err_msg.contains("configured") || err_msg.contains("disabled"),
        "Error message should mention missing store configuration: {err_msg}"
    );
}

#[tokio::test]
async fn test_memory_search_with_empty_store() {
    let store = MemoryStore::new();
    let search = MemorySearch::new(Some(std::sync::Arc::new(store)));
    let result = search
        .invoke(serde_json::json!({"query": "test", "limit": 5}))
        .await;

    assert!(
        result.is_ok(),
        "MemorySearch should succeed even with empty store: {:?}",
        result
    );

    let output = result.unwrap();
    assert!(
        output.contains("No relevant facts") || output.contains("not found"),
        "Output should indicate no facts found: {output}"
    );
}

// ---------------------------------------------------------------------------
// State Initialization Tests
// ---------------------------------------------------------------------------

#[test]
fn test_research_state_default() {
    let state = ResearchState::default();

    assert!(
        state.messages.is_empty(),
        "Default state should have empty messages"
    );
    assert!(
        state.plan.is_empty(),
        "Default state should have empty plan"
    );
    assert!(
        state.findings.is_empty(),
        "Default state should have empty findings"
    );
    assert!(
        state.report.is_none(),
        "Default state should have None report"
    );
    assert!(
        state.query.is_empty(),
        "Default state should have empty query"
    );
}

// ---------------------------------------------------------------------------
// MockChatModel Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mock_chat_model_basic() {
    let mock = MockChatModel::new("test-model").with_response("Hello, world!");
    let messages = vec![Message::human("Hi")];

    let result = mock.invoke(&messages, None).await;

    assert!(
        result.is_ok(),
        "MockChatModel should return response successfully: {:?}",
        result
    );

    let response = result.unwrap();
    // Verify response role is AI
    assert_eq!(response.role, juncture::llm::Role::Ai);
    assert_eq!(mock.model_name(), "test-model");
}

#[tokio::test]
async fn test_mock_chat_model_error() {
    let mock = MockChatModel::new("test-model").with_error();
    let messages = vec![Message::human("Hi")];

    let result = mock.invoke(&messages, None).await;

    assert!(
        result.is_err(),
        "MockChatModel with with_error() should return error: {:?}",
        result
    );
}

#[tokio::test]
async fn test_mock_chat_model_tool_calls() {
    let tool_calls = vec![ToolCall {
        id: "call_123".to_string(),
        name: "calculator".to_string(),
        arguments: serde_json::json!({"expression": "2 + 2"}),
    }];

    let mock = MockChatModel::new("test-model").with_tool_calls(tool_calls);
    let messages = vec![Message::human("Calculate 2 + 2")];

    let result = mock.invoke(&messages, None).await;

    assert!(
        result.is_ok(),
        "MockChatModel should return tool calls successfully: {:?}",
        result
    );

    let response = result.unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "calculator");
}

// Rust guideline compliant 2026-05-27
