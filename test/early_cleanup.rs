use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;

use corona::prelude::*;
use tokio::prelude::*;
use tokio::runtime::current_thread::Runtime;

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

fn fut_get() -> impl Future<Item = (), Error = ()> {
    future::empty()
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
    let mut rt = Runtime::new().unwrap();
    let mut finished = None;
    let status = Status::default();
    // This starts the coroutine, spawns waiting onto the executor, but doesn't finish.
    rt.block_on(future::lazy(|| {
        let status = status.clone();
        finished = Some(Coroutine::with_defaults(move || coroutine_panic(&status)));
        future::ok::<(), ()>(())
    })).unwrap();
    status.before_drop();
    drop(rt);
    status.after_drop(true);
    finished.wait().unwrap_err();
}

fn coroutine_nopanic(status: &Status) {
    let fut = fut_get();
    status.0.set(true);
    fut.coro_wait_cleanup().unwrap_err();
    status.1.set(true);
    let fut2 = fut_get();
    fut2.coro_wait_cleanup().unwrap_err();
}

/// Check cleaning up with manual handling of being dropped.
#[test]
fn cleanup_nopanic() {
    let mut rt = Runtime::new().unwrap();
    let status = Status::default();
    let mut finished = None;
    rt.block_on(future::lazy(|| {
        let status = status.clone();
        finished = Some(Coroutine::with_defaults(move || coroutine_nopanic(&status)));
        future::ok::<(), ()>(())
    })).unwrap();
    status.before_drop();
    // The coroutine finishes once we drop the runtime. Note that it finishes successfully, not
    // panicking.
    drop(rt);
    finished.wait().unwrap();
    status.after_drop(false);
}

/// The cleanup method handles panicking in the main thread correctly.
#[test]
fn cleanup_main_panic() {
    let mut rt = Runtime::new().unwrap();
    let status = Status::default();
    let mut finished = None;
    rt.block_on(future::lazy(|| {
        let status = status.clone();
        finished = Some(Coroutine::with_defaults(move || coroutine_nopanic(&status)));
        future::ok::<(), ()>(())
    })).unwrap();
    status.before_drop();
    panic_rt(rt);
    status.after_drop(false);
    finished.wait().unwrap();
}

/// Make the runtime go away by panicking inside a closure that holds it. We check clean up during
/// dropping things.
fn panic_rt(rt: Runtime) {
    panic::catch_unwind(AssertUnwindSafe(|| {
            // Steal the rt into the closure
            let _rt = rt;
            // And panic here, so the rt gets destroyed during an unwind
            panic!();
        }))
        .unwrap_err();
}

/// Just a testing stream.
fn no_stream() -> Box<Stream<Item = (), Error = ()>> {
    Box::new(stream::futures_unordered(vec![future::empty::<(), ()>()]))
}

#[test]
fn stream_cleanup() {
    let mut rt = Runtime::new().unwrap();
    let no_stream = no_stream();
    let status = Status::default();
    let mut finished = None;
    rt.block_on(future::lazy(|| {
        let status = status.clone();
        finished = Some(Coroutine::with_defaults(move || {
            status.0.set(true);
            for item in no_stream.iter_cleanup() {
                if item.is_err() {
                    status.1.set(true);
                    return;
                }
            }
            unreachable!();
        }));
        future::ok::<(), ()>(())
    })).unwrap();
    status.before_drop();
    panic_rt(rt);
    status.after_drop(false);
    finished.wait().unwrap();
}

/// Tests the implicit panic-based cleanup of a stream
#[test]
fn stream_panic() {
    let mut rt = Runtime::new().unwrap();
    let no_stream = no_stream();
    let status = Status::default();
    let mut finished = None;
    rt.block_on(future::lazy(|| {
        let status = status.clone();
        finished = Some(Coroutine::with_defaults(move || {
            status.0.set(true);
            for _ in no_stream.iter_result() {
                // It'll not get here
                status.1.set(true);
            }
            // And it'll not get here
            status.1.set(true);
        }));
        future::ok::<(), ()>(())
    })).unwrap();
    status.before_drop();
    drop(rt);
    status.after_drop(true);
    finished.wait().unwrap_err();
}
