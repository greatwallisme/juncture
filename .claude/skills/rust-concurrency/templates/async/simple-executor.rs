// Simple Async Executor Template
// Based on "深入理解Rust并发编程" Chapter 6

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable};
use std::time::{Duration, Instant};

/// Simple single-threaded async executor
pub struct SimpleExecutor {
    tasks: VecDeque<Task>,
    waker_cache: Vec<Waker>,
}

/// Task wrapper for futures
struct Task {
    id: usize,
    future: Pin<Box<dyn Future<Output = ()>>>,
    state: TaskState,
    created_at: Instant,
}

#[derive(Debug, Clone, Copy)]
enum TaskState {
    Ready,
    Waiting,
    Completed,
}

impl SimpleExecutor {
    /// Create a new executor
    pub fn new() -> Self {
        Self {
            tasks: VecDeque::new(),
            waker_cache: Vec::new(),
        }
    }

    /// Spawn a new future
    pub fn spawn<F>(&mut self, future: F) -> usize
    where
        F: Future<Output = ()> + 'static,
    {
        let task_id = self.tasks.len();
        let task = Task {
            id: task_id,
            future: Box::pin(future),
            state: TaskState::Ready,
            created_at: Instant::now(),
        };

        self.tasks.push_back(task);
        task_id
    }

    /// Run all spawned tasks to completion
    pub fn run(&mut self) {
        println!("Starting executor with {} tasks", self.tasks.len());

        let mut iteration = 0;
        while !self.tasks.is_empty() {
            iteration += 1;
            println!("\n=== Iteration {} ===", iteration);

            let mut completed_tasks = Vec::new();

            // Poll all ready tasks
            for (index, task) in self.tasks.iter_mut().enumerate() {
                match task.state {
                    TaskState::Ready | TaskState::Waiting => {
                        let waker = self.create_waker(task.id, index);
                        let mut cx = Context::from_waker(&waker);

                        println!("Polling task {}", task.id);
                        match task.future.as_mut().poll(&mut cx) {
                            Poll::Ready(()) => {
                                println!("Task {} completed", task.id);
                                completed_tasks.push(index);
                                task.state = TaskState::Completed;
                            }
                            Poll::Pending => {
                                task.state = TaskState::Waiting;
                                println!("Task {} pending", task.id);
                            }
                        }
                    }
                    TaskState::Completed => {
                        completed_tasks.push(index);
                    }
                }
            }

            // Remove completed tasks
            completed_tasks.sort_by(|a, b| b.cmp(a)); // Sort descending to maintain indices
            for &index in &completed_tasks {
                if index < self.tasks.len() {
                    self.tasks.remove(index);
                }
            }

            // Prevent busy waiting
            if !self.tasks.is_empty() {
                thread::sleep(Duration::from_millis(1));
            }
        }

        println!("\nAll tasks completed in {} iterations", iteration);
    }

    /// Create a waker for a specific task
    fn create_waker(&mut self, task_id: usize, task_index: usize) -> Waker {
        // Reuse waker if available to reduce allocations
        if task_index < self.waker_cache.len() {
            self.waker_cache[task_index].clone()
        } else {
            let waker = self.make_waker(task_id);

            // Extend cache if needed
            if task_index >= self.waker_cache.len() {
                self.waker_cache.resize(task_index + 1, waker.clone());
            } else {
                self.waker_cache[task_index] = waker.clone();
            }

            waker
        }
    }

    /// Create a new waker
    fn make_waker(&self, task_id: usize) -> Waker {
        let waker_data = WakerData { task_id };
        let raw_waker = RawWaker::new(
            Box::into_raw(Box::new(waker_data)) as *const (),
            &WAKER_VTABLE,
        );
        unsafe { Waker::from_raw(raw_waker) }
    }
}

/// Data stored in the waker
struct WakerData {
    task_id: usize,
}

/// Virtual table for waker operations
static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    clone_fn,
    wake_fn,
    wake_by_ref_fn,
    drop_fn,
);

/// Clone function for waker
fn clone_fn(data: *const ()) -> RawWaker {
    let waker_data = unsafe { &*(data as *const WakerData) };
    let new_data = Box::new(WakerData {
        task_id: waker_data.task_id,
    });
    RawWaker::new(
        Box::into_raw(new_data) as *const (),
        &WAKER_VTABLE,
    )
}

/// Wake function for waker
fn wake_fn(data: *const ()) {
    let waker_data = unsafe { &*(data as *const WakerData) };
    println!("Waking task {}", waker_data.task_id);

    // In a real implementation, this would mark the task as ready
    // and potentially notify the executor
}

/// Wake by reference function
fn wake_by_ref_fn(data: *const ()) {
    wake_fn(data);
}

/// Drop function for waker
fn drop_fn(data: *const ()) {
    let _waker_data = unsafe { Box::from_raw(data as *mut WakerData) };
}

/// Helper function to create a simple delay future
pub async fn delay_ms(milliseconds: u64) {
    DelayFuture::new(Duration::from_millis(milliseconds)).await
}

/// Simple delay future implementation
struct DelayFuture {
    start_time: Instant,
    duration: Duration,
}

impl DelayFuture {
    fn new(duration: Duration) -> Self {
        Self {
            start_time: Instant::now(),
            duration,
        }
    }
}

impl Future for DelayFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.start_time.elapsed() >= self.duration {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_simple_executor() {
        let mut executor = SimpleExecutor::new();

        // Spawn some test tasks
        for i in 0..3 {
            executor.spawn(async move {
                println!("Task {} started", i);
                delay_ms(10).await;
                println!("Task {} finished", i);
            });
        }

        executor.run();
    }

    #[test]
    fn test_multiple_tasks() {
        let mut executor = SimpleExecutor::new();

        // Spawn concurrent tasks
        for i in 0..5 {
            executor.spawn(async move {
                println!("Concurrent task {} starting", i);

                // Simulate work
                for j in 0..3 {
                    println!("Task {} step {}", i, j);
                    delay_ms(5).await;
                }

                println!("Concurrent task {} completed", i);
            });
        }

        executor.run();
    }

    #[test]
    fn test_nested_tasks() {
        let mut executor = SimpleExecutor::new();

        executor.spawn(async {
            println!("Outer task started");

            // Spawn inner async blocks
            let join_handle = async {
                println!("Inner task 1");
                delay_ms(20).await;
                println!("Inner task 1 completed");
            };

            let join_handle2 = async {
                println!("Inner task 2");
                delay_ms(10).await;
                println!("Inner task 2 completed");
            };

            // Wait for both inner tasks
            join_handle.await;
            join_handle2.await;

            println!("Outer task completed");
        });

        executor.run();
    }
}

/// Example usage
pub fn example_usage() {
    let mut executor = SimpleExecutor::new();

    // Spawn various types of tasks
    executor.spawn(async {
        println!("Starting producer task");

        for i in 0..5 {
            println!("Producing item: {}", i);
            delay_ms(100).await;
        }

        println!("Producer task finished");
    });

    executor.spawn(async {
        println!("Starting consumer task");

        for i in 0..3 {
            println!("Consuming item: {}", i);
            delay_ms(150).await;
        }

        println!("Consumer task finished");
    });

    // Run all tasks
    executor.run();
}