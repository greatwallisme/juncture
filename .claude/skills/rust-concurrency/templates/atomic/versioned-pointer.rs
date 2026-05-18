// Versioned Pointer Implementation for ABA Problem Prevention
// Based on "深入理解Rust并发编程" Chapter 4

use std::sync::atomic::{AtomicU64, Ordering};
use std::ptr;

/// Tagged pointer with version number to prevent ABA problem
/// Uses 48 bits for pointer and 16 bits for version tag
#[derive(Debug, Clone, Copy)]
pub struct TaggedPtr {
    raw: u64,
}

impl TaggedPtr {
    /// Create new tagged pointer from raw pointer and version
    /// Pointer must be 8-byte aligned (lowest 3 bits are zero)
    pub fn new(ptr: *mut u8, version: u64) -> Self {
        debug_assert_eq!(ptr as u64 & 0b111, 0, "Pointer must be 8-byte aligned");
        debug_assert!(version < (1 << 16), "Version must fit in 16 bits");

        let ptr_bits = ptr as u64;
        let version_bits = version << 48;

        Self { raw: ptr_bits | version_bits }
    }

    /// Extract pointer from tagged value
    pub fn ptr(&self) -> *mut u8 {
        ((self.raw & 0x0000FFFFFFFFFFFF) as *mut u8)
    }

    /// Extract version from tagged value
    pub fn version(&self) -> u64 {
        self.raw >> 48
    }

    /// Create new tagged pointer with incremented version
    pub fn with_incremented_version(&self) -> Self {
        Self::new(self.ptr(), self.version() + 1)
    }

    /// Check if pointer is null
    pub fn is_null(&self) -> bool {
        self.ptr().is_null()
    }
}

/// Atomic version of TaggedPtr for concurrent operations
pub struct AtomicTaggedPtr {
    atomic: AtomicU64,
}

impl AtomicTaggedPtr {
    /// Create new atomic tagged pointer
    pub fn new(ptr: *mut u8) -> Self {
        Self {
            atomic: AtomicU64::new(TaggedPtr::new(ptr, 0).raw),
        }
    }

    /// Load tagged pointer with specified memory ordering
    pub fn load(&self, ordering: Ordering) -> TaggedPtr {
        TaggedPtr { raw: self.atomic.load(ordering) }
    }

    /// Compare and swap operation for tagged pointer
    /// Returns old value on success, None on failure
    pub fn compare_exchange_weak(
        &self,
        current: TaggedPtr,
        new: TaggedPtr,
        success_order: Ordering,
        failure_order: Ordering,
    ) -> Result<TaggedPtr, TaggedPtr> {
        match self.atomic.compare_exchange_weak(
            current.raw,
            new.raw,
            success_order,
            failure_order,
        ) {
            Ok(raw) => Ok(TaggedPtr { raw }),
            Err(raw) => Err(TaggedPtr { raw }),
        }
    }

    /// Store new tagged pointer
    pub fn store(&self, ptr: TaggedPtr, ordering: Ordering) {
        self.atomic.store(ptr.raw, ordering);
    }
}

/// Example: Lock-free queue using versioned pointers
pub struct LockFreeQueue<T> {
    head: AtomicTaggedPtr,
    tail: AtomicTaggedPtr,
}

struct Node<T> {
    data: Option<T>,
    next: *mut Node<T>,
}

impl<T> LockFreeQueue<T> {
    pub fn new() -> Self {
        let dummy = Box::into_raw(Box::new(Node {
            data: None,
            next: ptr::null_mut(),
        }));

        Self {
            head: AtomicTaggedPtr::new(dummy as *mut u8),
            tail: AtomicTaggedPtr::new(dummy as *mut u8),
        }
    }

    /// Push item to the queue
    pub fn push(&self, data: T) {
        let new_node = Box::into_raw(Box::new(Node {
            data: Some(data),
            next: ptr::null_mut(),
        }));

        loop {
            let tail_tagged = self.tail.load(Ordering::Acquire);
            let tail_ptr = tail_tagged.ptr() as *mut Node<T>;

            unsafe {
                if (*tail_ptr).next.is_null() {
                    // Try to link new node
                    let new_tail_tagged = TaggedPtr::new(
                        new_node as *mut u8,
                        tail_tagged.version() + 1
                    );

                    match self.tail.compare_exchange_weak(
                        tail_tagged,
                        new_tail_tagged,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => {
                            // Successfully linked, now update tail
                            unsafe {
                                (*new_node as *mut Node<T>).next = ptr::null_mut();
                            }

                            let final_tail = TaggedPtr::new(
                                new_node as *mut u8,
                                tail_tagged.version() + 2
                            );

                            self.tail.store(final_tail, Ordering::Release);
                            break;
                        }
                        Err(_) => continue,
                    }
                } else {
                    // Help advance tail
                    let next = (*tail_ptr).next;
                    let advanced_tail = TaggedPtr::new(
                        next as *mut u8,
                        tail_tagged.version() + 1
                    );

                    if self.tail.compare_exchange_weak(
                        tail_tagged,
                        advanced_tail,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ).is_ok() {
                        // Successfully advanced tail
                        continue;
                    }
                }
            }
        }
    }

    /// Pop item from the queue
    pub fn pop(&self) -> Option<T> {
        loop {
            let head_tagged = self.head.load(Ordering::Acquire);
            let tail_tagged = self.tail.load(Ordering::Acquire);

            if head_tagged.ptr() == tail_tagged.ptr() {
                let head_ptr = head_tagged.ptr() as *mut Node<T>;
                unsafe {
                    if (*head_ptr).next.is_null() {
                        return None; // Queue is empty
                    }

                    // Tail is behind, try to help advance it
                    let next = (*head_ptr).next;
                    let advanced_tail = TaggedPtr::new(
                        next as *mut u8,
                        tail_tagged.version() + 1
                    );

                    if self.tail.compare_exchange_weak(
                        tail_tagged,
                        advanced_tail,
                        Ordering::Release,
                        Ordering::Relaxed,
                    ).is_ok() {
                        continue;
                    }
                }
            }

            let head_ptr = head_tagged.ptr() as *mut Node<T>;
            unsafe {
                let next = (*head_ptr).next;

                if next.is_null() {
                    // This shouldn't happen if queue is not empty
                    continue;
                }

                let new_head = TaggedPtr::new(
                    next as *mut u8,
                    head_tagged.version() + 1
                );

                match self.head.compare_exchange_weak(
                    head_tagged,
                    new_head,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => {
                        // Successfully removed node
                        let node = Box::from_raw(head_ptr);
                        return node.data;
                    }
                    Err(_) => continue,
                }
            }
        }
    }

    /// Get current length (approximate)
    pub fn len(&self) -> usize {
        let mut count = 0;
        let mut current = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);

        while current.ptr() != tail.ptr() && count < 1000 { // Prevent infinite loop
            unsafe {
                let node = current.ptr() as *mut Node<T>;
                if !node.is_null() {
                    current = TaggedPtr::new(
                        (*node).next as *mut u8,
                        current.version() + 1
                    );
                    count += 1;
                } else {
                    break;
                }
            }
        }

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tagged_ptr_operations() {
        let data = vec![1, 2, 3];
        let ptr = data.as_ptr() as *mut u8;

        let tagged = TaggedPtr::new(ptr, 42);
        assert_eq!(tagged.ptr(), ptr);
        assert_eq!(tagged.version(), 42);

        let incremented = tagged.with_incremented_version();
        assert_eq!(incremented.ptr(), ptr);
        assert_eq!(incremented.version(), 43);
    }

    #[test]
    fn test_lockfree_queue_basic() {
        let queue = LockFreeQueue::new();

        // Initially empty
        assert_eq!(queue.pop(), None);
        assert_eq!(queue.len(), 0);

        // Push and pop
        queue.push(42);
        queue.push(43);

        assert_eq!(queue.pop(), Some(42));
        assert_eq!(queue.pop(), Some(43));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;

        let queue = Arc::new(LockFreeQueue::new());
        let mut handles = vec![];

        // Producers
        for i in 0..4 {
            let queue = Arc::clone(&queue);
            handles.push(thread::spawn(move || {
                for j in 0..100 {
                    queue.push(i * 100 + j);
                }
            }));
        }

        // Consumers
        for _ in 0..4 {
            let queue = Arc::clone(&queue);
            handles.push(thread::spawn(move || {
                let mut results = vec![];
                loop {
                    if let Some(item) = queue.pop() {
                        results.push(item);
                    } else {
                        break;
                    }
                }
                results
            }));
        }

        // Wait for all producers
        for handle in handles.drain(4..8) {
            handle.join().unwrap();
        }

        // Wait for consumers and collect results
        let mut all_results = vec![];
        for handle in handles.drain(0..4) {
            all_results.extend(handle.join().unwrap());
        }

        // Verify we got all 400 items
        assert_eq!(all_results.len(), 400);
        all_results.sort();

        for expected in 0..400 {
            assert!(all_results.contains(&expected));
        }
    }
}