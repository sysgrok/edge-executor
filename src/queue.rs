use core::cell::RefCell;

use alloc::collections::vec_deque::VecDeque;

use async_task::Runnable;
use embassy_sync::blocking_mutex::{
    raw::{CriticalSectionRawMutex, RawMutex},
    Mutex,
};

extern crate alloc;

/// A trait abstracting the async executor queue.
/// The queue MUST be thread-safe (and could be ISR-safe if the executor is used in an embedded context).
///
/// Pushing and popping items from the queue must NOT block the current thread / the ISR interrupt.
///
/// Allocating memory in the push and pop operations is only allowed if the underlying operating system
/// allows allocations from an ISR context, and if the queue is ISR-safe in the first place.
pub trait ExecutorQueue: Sized {
    /// Creates a new queue.
    fn new() -> Self;

    /// Pushes a runnable into the queue.
    fn push(&self, runnable: Runnable);

    /// Pop an item from the queue.
    ///
    /// Return `Some(runnable)` if a runnable was successfully popped, or `None` if the queue is empty.
    fn pop(&self) -> Option<Runnable>;
}

/// An unbounded queue implementation using `VecDeque` from the `alloc` crate.
///
/// Note that this queue does allocate memory on push, and deallocates memory on pop,
/// so it may not be suitable for all use cases
/// (e.g. embedded contexts without an allocator, or contexts where allocations are not allowed in ISRs).
pub struct UnboundQueue<M: RawMutex = CriticalSectionRawMutex>(
    Mutex<M, RefCell<VecDeque<Runnable>>>,
);

impl<M: RawMutex> Default for UnboundQueue<M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex> UnboundQueue<M> {
    /// Creates a new empty queue.
    pub const fn new() -> Self {
        Self(Mutex::new(RefCell::new(VecDeque::new())))
    }

    /// Create a new queue with the specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self(Mutex::new(RefCell::new(VecDeque::with_capacity(capacity))))
    }
}

impl<M: RawMutex> ExecutorQueue for UnboundQueue<M>
where
    M: RawMutex,
{
    fn new() -> Self {
        Self::new()
    }

    fn push(&self, runnable: Runnable) {
        self.0.lock(|rc| {
            let mut queue = rc.borrow_mut();

            queue.push_back(runnable);
        })
    }

    fn pop(&self) -> Option<Runnable> {
        self.0.lock(|rc| {
            let mut queue = rc.borrow_mut();

            queue.pop_front()
        })
    }
}

/// A fixed-capacity queue implementation using `heapless::Deque`.
pub struct BoundQueue<const C: usize = 64, M: RawMutex = CriticalSectionRawMutex>(
    Mutex<M, RefCell<heapless::Deque<Runnable, C>>>,
);

impl<const C: usize, M: RawMutex> Default for BoundQueue<C, M> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const C: usize, M: RawMutex> BoundQueue<C, M> {
    /// Creates a new empty queue.
    pub const fn new() -> Self {
        Self(Mutex::new(RefCell::new(heapless::Deque::new())))
    }
}

impl<const C: usize, M: RawMutex> ExecutorQueue for BoundQueue<C, M> {
    fn new() -> Self {
        Self::new()
    }

    fn push(&self, runnable: Runnable) {
        self.0.lock(|rc| {
            let mut queue = rc.borrow_mut();

            queue
                .push_back(runnable)
                .expect("BoundQueue capacity exceeded");
        })
    }

    fn pop(&self) -> Option<Runnable> {
        self.0.lock(|rc| {
            let mut queue = rc.borrow_mut();

            queue.pop_front()
        })
    }
}
