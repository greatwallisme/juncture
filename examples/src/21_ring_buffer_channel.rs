//! Example 21: Ring Buffer Channel
//!
//! Demonstrates the `RingBufferChannel` for bounded append-heavy fields:
//! - Creating a channel with maximum capacity
//! - Automatic trimming of oldest elements when capacity is exceeded
//! - Checkpoint round-trip preserving capacity
//!
//! Key concepts:
//! - `RingBufferChannel` for bounded append semantics
//! - Automatic capacity enforcement
//! - Checkpoint serialization with capacity preservation

use juncture_core::state::channel::{Channel, RingBufferChannel};
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = std::io::stdout();

    // Demonstrate RingBufferChannel directly
    writeln!(stdout, "=== RingBufferChannel Demo ===")?;
    writeln!(stdout)?;

    // Create a ring buffer with capacity 5
    let mut buffer = RingBufferChannel::new(Vec::new(), 5);
    writeln!(stdout, "Created buffer with capacity: {}", buffer.capacity())?;

    // Add items using Channel trait
    for i in 1..=8 {
        <RingBufferChannel<String> as Channel<Vec<String>>>::update(
            &mut buffer,
            vec![vec![format!("message_{i}")]],
        );
        writeln!(
            stdout,
            "After adding message_{i}: len={}, items={:?}",
            buffer.len(),
            buffer.get()
        )?;
    }

    writeln!(stdout)?;
    writeln!(stdout, "Note: Only the last 5 messages are kept")?;
    writeln!(stdout)?;

    // Demonstrate checkpoint round-trip
    writeln!(stdout, "=== Checkpoint Round-trip ===")?;
    let checkpoint = <RingBufferChannel<String> as Channel<Vec<String>>>::checkpoint(&buffer)
        .unwrap();
    writeln!(stdout, "Checkpoint: {checkpoint}")?;

    let restored = <RingBufferChannel<String> as Channel<Vec<String>>>::from_checkpoint(checkpoint)
        .unwrap();
    writeln!(
        stdout,
        "Restored: len={}, capacity={}",
        restored.len(),
        restored.capacity()
    )?;
    writeln!(stdout, "Items: {:?}", restored.get())?;

    Ok(())
}

// Rust guideline compliant 2026-06-06
