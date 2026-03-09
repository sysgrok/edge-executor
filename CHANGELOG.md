# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]
* Potential fix for #3 (long running futures eventually stall):
  * Background:
    * Turns out this is likely related to ALL executor queues used by this crate (`crossbeam-queue`, `heapless MPMC`) being incompatible with OSes spawning threads at different priorities (since these queues do not use the OS mutexes, they are amenable to priority inversion issues - https://github.com/crossbeam-rs/crossbeam/issues/675#issuecomment-4003522816)
    * Note that very likely [`async-executor`](https://github.com/smol-rs/async-executor) is also affected, as it uses `concurrent-queue`, which has the same issue
    * For more tetails, check https://github.com/esp-rs/esp-idf-svc/issues/630
  * Solution:
    * Use our own queue based on a mutex + `alloc::VeqDeque` (unbounded) or `heapless::Deque` (bounded)
    * Since the queue is now protected by a mutex (via `embassy-sync`) - as long as the mutex is the OS/Platform one - and it is by default - there should be no priority inversion issues anymore
    * Furter, allow the user to plug her own executor queue, via the new `ExecutorQueue` trait

## [0.4.1] - 2023-11-09
* Update to heapless 0.8, and use heapless `MpMcQueue` for targets that do not have atomics, as `crossbeam` does not support such targets yet

## [0.4.0] - 2023-10-17
* Crate goals clarified
* Crate redesigned to follow the API of `async-executor`
* Re-implemented as a completely portable executor which is itself a `Future` (platform/OS is expected to provide a `block_on` or "spawn a `Future` onto an event loop" primitive, but the executor is unaware of it)
* Documentation, examples, tests
* More user-friendly README
