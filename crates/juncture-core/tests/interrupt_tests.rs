//! Integration tests for Interrupt/HITL system

use juncture_core::{interrupt::InterruptContext, interrupt_with_ctx};
use juncture_derive::State;
use serde_json::json;

// Test state (needed for interrupt context but not directly used)
#[allow(
    dead_code,
    reason = "state type needed for interrupt context integration"
)]
#[derive(Debug, Clone, State)]
struct TestState {
    value: u32,
}

#[test]
fn test_interrupt_context_new() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![Some(json!("input1")), None, Some(json!("input3"))];

    let ctx = InterruptContext::new(resume_values, tx);
    assert_eq!(ctx.current_index(), 0);
}

#[test]
fn test_interrupt_context_next_index() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = InterruptContext::new(vec![None, None], tx);

    assert_eq!(ctx.next_index(), 0);
    assert_eq!(ctx.next_index(), 1);
    assert_eq!(ctx.next_index(), 2);
    assert_eq!(ctx.current_index(), 3);
}

#[test]
fn test_interrupt_context_get_resume_value() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![Some(json!("first")), None, Some(json!("third"))];

    let ctx = InterruptContext::new(resume_values, tx);

    assert_eq!(ctx.get_resume_value(0), Some(json!("first")));
    assert_eq!(ctx.get_resume_value(1), None);
    assert_eq!(ctx.get_resume_value(2), Some(json!("third")));
    assert_eq!(ctx.get_resume_value(3), None);
}

#[test]
fn test_interrupt_context_send_signal() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = InterruptContext::new(vec![None], tx);

    let signal = juncture_core::interrupt::InterruptSignal {
        index: 0,
        id: Some("test_id".to_string()),
        payload: json!("test_payload"),
    };

    ctx.send_interrupt(signal).unwrap();

    let received = rx.blocking_recv().unwrap();
    assert_eq!(received.index, 0);
    assert_eq!(received.id, Some("test_id".to_string()));
    assert_eq!(received.payload, json!("test_payload"));
}

#[tokio::test]
async fn test_interrupt_impl_with_resume_value() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![Some(json!("resumed_value"))];
    let ctx = InterruptContext::new(resume_values, tx);

    let result =
        juncture_core::interrupt::__interrupt_impl(&ctx, json!("original_payload"), None).await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!("resumed_value"));
}

#[tokio::test]
async fn test_interrupt_impl_without_resume_value() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![None];
    let ctx = InterruptContext::new(resume_values, tx);

    let result =
        juncture_core::interrupt::__interrupt_impl(&ctx, json!("original_payload"), None).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().is_interrupt());

    let signal = rx.try_recv().expect("signal should be available");
    assert_eq!(signal.index, 0);
    assert_eq!(signal.payload, json!("original_payload"));
    assert!(signal.id.is_some());
}

#[tokio::test]
async fn test_interrupt_macro_basic() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![Some(json!("macro_value"))];
    let ctx = InterruptContext::new(resume_values, tx);

    // Use interrupt_with_ctx! for explicit context version
    let result = interrupt_with_ctx!(&ctx, json!("test_payload"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!("macro_value"));
}

#[tokio::test]
async fn test_interrupt_macro_with_interrupt() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![None];
    let ctx = InterruptContext::new(resume_values, tx);

    // Use interrupt_with_ctx! for explicit context version
    let result = interrupt_with_ctx!(&ctx, json!("interrupt_test"));

    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(err.is_interrupt());

    let signal = rx.try_recv().expect("signal should be available");
    assert_eq!(signal.payload, json!("interrupt_test"));
}

#[tokio::test]
async fn test_interrupt_impl_with_named_id() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![Some(json!("resumed_value"))];
    let ctx = InterruptContext::new(resume_values, tx);

    let result = juncture_core::interrupt::__interrupt_impl(
        &ctx,
        json!("original_payload"),
        Some("my_named_id"),
    )
    .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!("resumed_value"));
}

#[tokio::test]
async fn test_interrupt_impl_named_id_without_resume() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![None];
    let ctx = InterruptContext::new(resume_values, tx);

    let result = juncture_core::interrupt::__interrupt_impl(
        &ctx,
        json!("payload"),
        Some("custom_interrupt_id"),
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().is_interrupt());

    let signal = rx.try_recv().expect("signal should be available");
    assert_eq!(signal.index, 0);
    assert_eq!(signal.payload, json!("payload"));
    assert_eq!(signal.id, Some("custom_interrupt_id".to_string()));
}

#[tokio::test]
async fn test_interrupt_macro_with_named_id() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![Some(json!("resumed_value"))];
    let ctx = InterruptContext::new(resume_values, tx);

    let result = interrupt_with_ctx!(&ctx, "named_step", json!("test_payload"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), json!("resumed_value"));
}

#[tokio::test]
async fn test_interrupt_macro_named_id_interrupts() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![None];
    let ctx = InterruptContext::new(resume_values, tx);

    let result = interrupt_with_ctx!(&ctx, "approve_action", json!({"question": "ok?"}));

    assert!(result.is_err());
    assert!(result.unwrap_err().is_interrupt());

    let signal = rx.try_recv().expect("signal should be available");
    assert_eq!(signal.payload, json!({"question": "ok?"}));
    assert_eq!(signal.id, Some("approve_action".to_string()));
}

#[tokio::test]
async fn test_interrupt_macro_anonymous_still_works() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let resume_values = vec![None];
    let ctx = InterruptContext::new(resume_values, tx);

    let result = interrupt_with_ctx!(&ctx, json!("anonymous_payload"));

    assert!(result.is_err());
    assert!(result.unwrap_err().is_interrupt());

    let signal = rx.try_recv().expect("signal should be available");
    assert_eq!(signal.payload, json!("anonymous_payload"));
    // Anonymous: ID is auto-generated, not None in the signal itself
    assert!(signal.id.is_some());
}

// Rust guideline compliant 2026-05-21
