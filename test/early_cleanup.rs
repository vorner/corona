use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use corona::Coroutine;
use corona::prelude::*;
use futures::{self, Future, Stream};
use futures::future;
use futures::stream;
use tokio_core::reactor::Core;

#[derive(Clone, Default)]
struct Status(Rc<Cell<bool>>, Rc<Cell<bool>>);

impl Status {
    fn before_drop(&self) {
        // Check it got to the place where it waits for the future
        assert!(self.0.get());
        assert!(!self.1.get());
        // And the RC is still held by it
        assert_eq!(2, Rc::strong_count(&self.0));
    }
    fn after_drop(&self, panicked: bool) {
        // The coroutine got cleaned up
        assert!(self.0.get());
        assert_eq!(!panicked, self.1.get());
        assert_eq!(1, Rc::strong_count(&self.0));
    }
}

fn fut_get() -> Box<Future<Item = (), Error = ()>> {
    Box::new(future::lazy(|| Ok::<_, ()>(())))
}

fn coroutine_panic(status: &Status) {
    let fut = fut_get();
    status.0.set(true);
    let _ = fut.coro_wait();
    status.1.set(true);
}

/// Check cleaning up the coroutines if the core is dropped and the coroutines haven't resolved
/// yet.
#[test]
fn cleanup_panic() {
    let coroutine_get = |handle, status: Status| {
            Coroutine::with_defaults(handle, move || coroutine_panic(&status))
        };
        let core = Core::new().unwrap();
        let handle = core.handle();
        let status = Status::default();
        let finished = coroutine_get(core.handle(), status.clone());
        status.before_drop();
        // When we drop the core, it also drops the future and cleans up the coroutine
        drop(core);
        status.after_drop(true);
        finished.wait().unwrap_err();

        // If we start another similar coroutine now, it gets destroyed on the call to the
        // coro_wait right away.
        status.0.set(false);
        let finished = coroutine_get(handle, status.clone());
        status.after_drop(true);
        finished.wait().unwrap_err();
}

fn coroutine_nopanic(status: &Status) {
    let fut = fut_get();
    status.0.set(true);
    fut.coro_wait_cleanup().unwrap_err();
    status.1.set(true);
    // Another one still returns error
    let fut2 = fut_get();
    fut2.coro_wait_cleanup().unwrap_err();
}

/// Check cleaning up with manual handling of being dropped.
#[test]
fn cleanup_nopanic() {
    let core = Core::new().unwrap();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move || coroutine_nopanic(&status_cp));

    status.before_drop();
    // The coroutine finishes once we drop the core. Note that it finishes successfully, not
    // panicking.
    drop(core);
    status.after_drop(false);
    finished.wait().unwrap();
}

/// The cleanup method handles panicking in the main thread correctly.
#[test]
fn cleanup_main_panic() {
    let core = Core::new().unwrap();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move || coroutine_nopanic(&status_cp));
    status.before_drop();
    panic_core(core);
    status.after_drop(false);
    finished.wait().unwrap();
}

/// Make the core go away by panicking inside a closure that holds it
fn panic_core(core: Core) {
    panic::catch_unwind(AssertUnwindSafe(|| {
            // Steal the core into the closure
            let _core = core;
            // And panic here, so the core gets destroyed during an unwind
            panic!();
        }))
        .unwrap_err();
}

/// Just a testing stream.
fn no_stream() -> Box<Stream<Item = (), Error = ()>> {
    Box::new(stream::futures_unordered(vec![futures::empty::<(), ()>()]))
}

/// Tests the explicit no-panic cleanup of a coroutine blocked on a stream.
#[test]
fn stream_cleanup() {
    let core = Core::new().unwrap();
    let no_stream = no_stream();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move || {
        status_cp.0.set(true);
        for item in no_stream.iter_cleanup() {
            if item.is_err() {
                status_cp.1.set(true);
                return;
            }
        }
        unreachable!();
    });
    status.before_drop();
    panic_core(core);
    status.after_drop(false);
    finished.wait().unwrap();
}

/// Tests the implicit panic-based cleanup of a stream
#[test]
fn stream_panic() {
    let core = Core::new().unwrap();
    let no_stream = no_stream();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move || {
        status_cp.0.set(true);
        for _ in no_stream.iter_result() {
            // It'll not get here
            status_cp.1.set(true);
        }
        // And it'll not get here
        status_cp.1.set(true);
    });
    status.before_drop();
    drop(core);
    status.after_drop(true);
    finished.wait().unwrap_err();
}
