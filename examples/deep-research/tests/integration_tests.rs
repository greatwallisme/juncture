//! Integration tests for deep-research application.
//!
//! Tests cover tool functionality, security validations, error handling,
//! and state initialization using `MockChatModel` where applicable.

use juncture::llm::{ChatModel, Message, MockChatModel, ToolCall};
use juncture::tools::Tool;

// Import from the library being tested
use deep_research::tools::{Calculator, ReadFile, WebSearch};

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
}

#[tokio::test]
async fn test_calculator_division_by_zero() {
    let calc = Calculator::new();
    let result = calc
        .invoke(serde_json::json!({"expression": "1 / 0"}))
        .await;

    assert!(
        result.is_err(),
        "Calculator should return error for division by zero: {result:?}"
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
        "Calculator should evaluate complex expression: {result:?}"
    );
    assert_eq!(result.unwrap(), "57");
}

#[tokio::test]
async fn test_calculator_invalid_expression() {
    let calc = Calculator::new();
    let result = calc
        .invoke(serde_json::json!({"expression": "invalid + expression"}))
        .await;

    assert!(
        result.is_err(),
        "Calculator should return error for invalid expression: {result:?}"
    );
}

#[tokio::test]
async fn test_calculator_missing_parameter() {
    let calc = Calculator::new();
    let result = calc.invoke(serde_json::json!({})).await;

    assert!(
        result.is_err(),
        "Calculator should return error when 'expression' parameter is missing: {result:?}"
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
        "ReadFile should reject path traversal attacks: {result:?}"
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
        "ReadFile should reject absolute paths: {result:?}"
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
        "ReadFile should return error when 'path' parameter is missing: {result:?}"
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
        "WebSearch should return error when API key is not configured: {result:?}"
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
        "WebSearch should return error when 'query' parameter is missing: {result:?}"
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
        "MockChatModel should return response successfully: {result:?}"
    );

    let response = result.unwrap();
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
        "MockChatModel with with_error() should return error: {result:?}"
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
        "MockChatModel should return tool calls successfully: {result:?}"
    );

    let response = result.unwrap();
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].name, "calculator");
}

// Rust guideline compliant 2026-05-27
