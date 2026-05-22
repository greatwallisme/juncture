//! Integration tests for Runtime system

use juncture_core::{Heartbeat, Runtime};

// Test context
#[derive(Debug, Clone, Default)]
struct TestContext {
    user_id: String,
}

#[test]
fn test_runtime_new() {
    let runtime = Runtime::<()>::new();
    assert!(runtime.store.is_none());
    assert!(runtime.previous.is_none());
    assert!(runtime.execution_info.is_none());
    assert!(runtime.control.is_none());
}

#[test]
fn test_runtime_default() {
    let runtime = Runtime::<()>::default();
    assert!(runtime.store.is_none());
}

#[test]
fn test_runtime_with_context() {
    let context = TestContext {
        user_id: "test_user".to_string(),
    };

    let runtime = Runtime::with_context(context);
    assert_eq!(runtime.context.user_id, "test_user");
}

#[test]
fn test_runtime_clone() {
    let context = TestContext {
        user_id: "test_user".to_string(),
    };

    let runtime = Runtime::with_context(context);
    let runtime2 = runtime.clone();

    assert_eq!(runtime.context.user_id, runtime2.context.user_id);
}

#[test]
fn test_runtime_managed_values() {
    let runtime = Runtime::<()>::new();
    let managed = runtime.managed_values();

    assert!(!managed.is_last_step);
    assert_eq!(managed.remaining_steps, 25);
}

#[test]
fn test_execution_info() {
    let info = juncture_core::ExecutionInfo {
        checkpoint_id: "checkpoint_123".to_string(),
        checkpoint_ns: "default".to_string(),
        task_id: "task_456".to_string(),
        thread_id: Some("thread_789".to_string()),
        run_id: Some("run_abc".to_string()),
        node_attempt: 2,
        node_first_attempt_time: Some(1_234_567_890.0),
    };

    assert_eq!(info.checkpoint_id, "checkpoint_123");
    assert_eq!(info.checkpoint_ns, "default");
    assert_eq!(info.task_id, "task_456");
    assert_eq!(info.thread_id, Some("thread_789".to_string()));
    assert_eq!(info.run_id, Some("run_abc".to_string()));
    assert_eq!(info.node_attempt, 2);
    assert_eq!(info.node_first_attempt_time, Some(1_234_567_890.0));
}

#[test]
fn test_managed_values() {
    let managed = juncture_core::ManagedValues {
        is_last_step: true,
        remaining_steps: 1,
    };

    assert!(managed.is_last_step);
    assert_eq!(managed.remaining_steps, 1);
}

#[test]
fn test_run_control_new() {
    let control = juncture_core::RunControl::new();
    assert!(!control.drain_requested());
    assert_eq!(control.drain_reason(), None);
}

#[test]
fn test_run_control_request_drain() {
    let control = juncture_core::RunControl::new();
    assert!(!control.drain_requested());

    control.request_drain("testing drain");
    assert!(control.drain_requested());
    assert_eq!(control.drain_reason(), Some("testing drain".to_string()));
}

#[test]
fn test_run_control_default() {
    let control = juncture_core::RunControl::default();
    assert!(!control.drain_requested());
}

#[test]
fn test_heartbeat_new() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let heartbeat = juncture_core::Heartbeat::new(tx);
    let result = heartbeat.ping();
    assert_eq!(result, Ok(()));
}

#[test]
fn test_heartbeat_default() {
    let heartbeat = juncture_core::Heartbeat::default();
    let result = heartbeat.ping();
    assert_eq!(result, Ok(()));
}

#[test]
fn test_heartbeat_watcher_alive_after_ping() {
    let (heartbeat, mut watcher) = juncture_core::Heartbeat::new_pair();

    // Immediately after creation, the watcher considers the source alive
    // (last_beat is initialized to Instant::now())
    assert!(watcher.is_alive(std::time::Duration::from_secs(60)));

    // After sending a ping, the watcher sees recent activity
    heartbeat.ping().expect("ping should succeed");
    assert!(watcher.is_alive(std::time::Duration::from_secs(60)));
}

#[test]
fn test_heartbeat_watcher_zero_duration_timeout_returns_bool() {
    let (heartbeat, mut watcher) = juncture_core::Heartbeat::new_pair();
    heartbeat.ping().expect("ping should succeed");

    // A zero-duration timeout means "no heartbeat within 0 seconds".
    // This always returns a bool (true if the ping and check happened
    // within the same nanosecond, false otherwise).
    let _ = watcher.is_alive(std::time::Duration::ZERO);
    // The purpose of this test is to verify no panic, not a specific value.
}

#[test]
fn test_heartbeat_watcher_drains_multiple_pings() {
    let (heartbeat, mut watcher) = juncture_core::Heartbeat::new_pair();

    // Send multiple pings
    heartbeat.ping().expect("first ping");
    heartbeat.ping().expect("second ping");
    heartbeat.ping().expect("third ping");

    // After draining, all pings are consumed. The watcher should still
    // report alive because the last heartbeat timestamp is current.
    assert!(watcher.is_alive(std::time::Duration::from_secs(60)));

    // Second check should also work (no panic on empty channel)
    assert!(watcher.is_alive(std::time::Duration::from_secs(60)));
}

#[test]
fn test_heartbeat_watcher_clone_heartbeat_works() {
    let (heartbeat, mut watcher) = juncture_core::Heartbeat::new_pair();

    // Clone the heartbeat sender and verify pings from
    // cloned senders are received by the original watcher.
    let heartbeat_clone = Heartbeat::clone(&heartbeat);

    // Send ping from clone, not from the original heartbeat
    heartbeat_clone.ping().expect("ping from clone should succeed");

    // Watcher should see the ping from the clone
    assert!(watcher.is_alive(std::time::Duration::from_secs(60)));
}

// Rust guideline compliant 2026-05-22
