#![no_std]

use core::future::{poll_fn, Future};
use core::marker::PhantomData;
use core::task::{Context, Poll};

use alloc::rc::Rc;

use async_task::Runnable;

pub use async_task::{FallibleTask, Task};

use atomic_waker::AtomicWaker;
use futures_lite::FutureExt;

#[cfg(not(feature = "portable-atomic"))]
use alloc::sync::Arc;
#[cfg(feature = "portable-atomic")]
use portable_atomic_util::Arc;

use once_cell::sync::OnceCell;

#[cfg(feature = "std")]
pub use futures_lite::future::block_on;

pub use queue::*;

extern crate alloc;

mod queue;

/// An async executor.
///
/// # Examples
///
/// A multi-threaded executor:
///
/// ```ignore
/// use async_channel::unbounded;
/// use easy_parallel::Parallel;
///
/// use edge_executor::{Executor, block_on};
///
/// let ex: Executor = Default::default();
/// let (signal, shutdown) = unbounded::<()>();
///
/// Parallel::new()
///     // Run four executor threads.
///     .each(0..4, |_| block_on(ex.run(shutdown.recv())))
///     // Run the main future on the current thread.
///     .finish(|| block_on(async {
///         println!("Hello world!");
///         drop(signal);
///     }));
/// ```
pub struct Executor<'a, Q = BoundQueue> {
    state: OnceCell<Arc<State<Q>>>,
    queue_ctor: fn() -> Q,
    _marker: PhantomData<core::cell::UnsafeCell<&'a ()>>,
}

impl<'a, Q: ExecutorQueue> Executor<'a, Q> {
    /// Creates a new executor.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::Executor;
    ///
    /// let ex: Executor = Default::default();
    /// ```
    pub const fn new() -> Self {
        Self::new_with(Q::new)
    }

    /// Creates a new executor with the provided queue constructor.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::{Executor, UnboundQueue};
    ///
    /// let ex = Executor::new_with(|| { let queue: UnboundQueue = UnboundQueue::with_capacity(100); queue });
    /// ```
    pub const fn new_with(queue_ctor: fn() -> Q) -> Self {
        Self {
            state: OnceCell::new(),
            queue_ctor,
            _marker: PhantomData,
        }
    }

    /// Spawns a task onto the executor.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::Executor;
    ///
    /// let ex: Executor = Default::default();
    ///
    /// let task = ex.spawn(async {
    ///     println!("Hello world");
    /// });
    /// ```
    ///
    /// Note that if the executor's queue size is equal to the number of currently
    /// spawned and running tasks, spawning this additional task might cause the executor to panic
    /// later, when the task is scheduled for polling.
    pub fn spawn<F>(&self, fut: F) -> Task<F::Output>
    where
        F: Future + Send + 'a,
        F::Output: Send + 'a,
    {
        unsafe { self.spawn_unchecked(fut) }
    }

    /// Attempts to run a task if at least one is scheduled.
    ///
    /// Running a scheduled task means simply polling its future once.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::Executor;
    ///
    /// let ex: Executor = Default::default();
    /// assert!(!ex.try_tick()); // no tasks to run
    ///
    /// let task = ex.spawn(async {
    ///     println!("Hello world");
    /// });
    /// assert!(ex.try_tick()); // a task was found
    /// ```    
    pub fn try_tick(&self) -> bool {
        if let Some(runnable) = self.try_runnable() {
            runnable.run();

            true
        } else {
            false
        }
    }

    /// Runs a single task asynchronously.
    ///
    /// Running a task means simply polling its future once.
    ///
    /// If no tasks are scheduled when this method is called, it will wait until one is scheduled.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::{Executor, block_on};
    ///
    /// let ex: Executor = Default::default();
    ///
    /// let task = ex.spawn(async {
    ///     println!("Hello world");
    /// });
    /// block_on(ex.tick()); // runs the task
    /// ```
    pub async fn tick(&self) {
        self.runnable().await.run();
    }

    /// Runs the executor asynchronously until the given future completes.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::{Executor, block_on};
    ///
    /// let ex: Executor = Default::default();
    ///
    /// let task = ex.spawn(async { 1 + 2 });
    /// let res = block_on(ex.run(async { task.await * 2 }));
    ///
    /// assert_eq!(res, 6);
    /// ```
    pub async fn run<F>(&self, fut: F) -> F::Output
    where
        F: Future,
    {
        unsafe { self.run_unchecked(fut).await }
    }

    /// Waits for the next runnable task to run.
    async fn runnable(&self) -> Runnable {
        poll_fn(|ctx| self.poll_runnable(ctx)).await
    }

    /// Polls the first task scheduled for execution by the executor.
    fn poll_runnable(&self, ctx: &Context<'_>) -> Poll<Runnable> {
        self.state().waker.register(ctx.waker());

        if let Some(runnable) = self.try_runnable() {
            Poll::Ready(runnable)
        } else {
            Poll::Pending
        }
    }

    /// Pops the first task scheduled for execution by the executor.
    ///
    /// Returns
    /// - `None` - if no task was scheduled for execution
    /// - `Some(Runnnable)` - the first task scheduled for execution. Calling `Runnable::run` will
    ///   execute the task. In other words, it will poll its future.
    fn try_runnable(&self) -> Option<Runnable> {
        self.state().queue.pop()
    }

    unsafe fn spawn_unchecked<F>(&self, fut: F) -> Task<F::Output>
    where
        F: Future,
    {
        let schedule = {
            let state = self.state().clone();

            move |runnable| {
                state.queue.push(runnable);

                if let Some(waker) = state.waker.take() {
                    waker.wake();
                }
            }
        };

        let (runnable, task) = unsafe { async_task::spawn_unchecked(fut, schedule) };

        runnable.schedule();

        task
    }

    async unsafe fn run_unchecked<F>(&self, fut: F) -> F::Output
    where
        F: Future,
    {
        let run_forever = async {
            loop {
                self.tick().await;
            }
        };

        run_forever.or(fut).await
    }

    /// Returns a reference to the inner state.
    fn state(&self) -> &Arc<State<Q>> {
        self.state
            .get_or_init(|| Arc::new(State::new((self.queue_ctor)())))
    }
}

impl<Q: ExecutorQueue> Default for Executor<'_, Q> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<Q: Send> Send for Executor<'_, Q> {}
unsafe impl<Q: Sync> Sync for Executor<'_, Q> {}

/// A thread-local executor.
///
/// The executor can only be run on the thread that created it.
///
/// # Examples
///
/// ```
/// use edge_executor::{LocalExecutor, block_on};
///
/// let local_ex: LocalExecutor = Default::default();
///
/// block_on(local_ex.run(async {
///     println!("Hello world!");
/// }));
/// ```
pub struct LocalExecutor<'a, Q = BoundQueue> {
    executor: Executor<'a, Q>,
    _marker: PhantomData<Rc<()>>,
}

impl<'a, Q: ExecutorQueue> LocalExecutor<'a, Q> {
    /// Creates a single-threaded executor.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::LocalExecutor;
    ///
    /// let local_ex: LocalExecutor = Default::default();
    /// ```
    pub const fn new() -> Self {
        Self::new_with(Q::new)
    }

    /// Creates a single-threaded executor with the provided queue constructor.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::{LocalExecutor, UnboundQueue};
    ///
    /// let local_ex = LocalExecutor::new_with(|| { let queue: UnboundQueue = UnboundQueue::with_capacity(100); queue });
    /// ```
    pub const fn new_with(queue_ctor: fn() -> Q) -> Self {
        Self {
            executor: Executor::<Q>::new_with(queue_ctor),
            _marker: PhantomData,
        }
    }

    /// Spawns a task onto the executor.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::LocalExecutor;
    ///
    /// let local_ex: LocalExecutor = Default::default();
    ///
    /// let task = local_ex.spawn(async {
    ///     println!("Hello world");
    /// });
    /// ```
    ///
    /// Note that if the executor's queue size is equal to the number of currently
    /// spawned and running tasks, spawning this additional task might cause the executor to panic
    /// later, when the task is scheduled for polling.
    pub fn spawn<F>(&self, fut: F) -> Task<F::Output>
    where
        F: Future + 'a,
        F::Output: 'a,
    {
        unsafe { self.executor.spawn_unchecked(fut) }
    }

    /// Attempts to run a task if at least one is scheduled.
    ///
    /// Running a scheduled task means simply polling its future once.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::LocalExecutor;
    ///
    /// let local_ex: LocalExecutor = Default::default();
    /// assert!(!local_ex.try_tick()); // no tasks to run
    ///
    /// let task = local_ex.spawn(async {
    ///     println!("Hello world");
    /// });
    /// assert!(local_ex.try_tick()); // a task was found
    /// ```    
    pub fn try_tick(&self) -> bool {
        self.executor.try_tick()
    }

    /// Runs a single task asynchronously.
    ///
    /// Running a task means simply polling its future once.
    ///
    /// If no tasks are scheduled when this method is called, it will wait until one is scheduled.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::{LocalExecutor, block_on};
    ///
    /// let local_ex: LocalExecutor = Default::default();
    ///
    /// let task = local_ex.spawn(async {
    ///     println!("Hello world");
    /// });
    /// block_on(local_ex.tick()); // runs the task
    /// ```
    pub async fn tick(&self) {
        self.executor.tick().await
    }

    /// Runs the executor asynchronously until the given future completes.
    ///
    /// # Examples
    ///
    /// ```
    /// use edge_executor::{LocalExecutor, block_on};
    ///
    /// let local_ex: LocalExecutor = Default::default();
    ///
    /// let task = local_ex.spawn(async { 1 + 2 });
    /// let res = block_on(local_ex.run(async { task.await * 2 }));
    ///
    /// assert_eq!(res, 6);
    /// ```
    pub async fn run<F>(&self, fut: F) -> F::Output
    where
        F: Future,
    {
        unsafe { self.executor.run_unchecked(fut) }.await
    }
}

impl<'a, Q: ExecutorQueue> Default for LocalExecutor<'a, Q> {
    fn default() -> Self {
        Self::new()
    }
}

struct State<Q> {
    queue: Q,
    waker: AtomicWaker,
}

impl<Q> State<Q> {
    const fn new(queue: Q) -> Self {
        Self {
            queue,
            waker: AtomicWaker::new(),
        }
    }
}
