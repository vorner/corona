extern crate corona;
extern crate futures;
extern crate tokio_core;

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::rc::Rc;
use std::time::Duration;

use corona::{Await, Coroutine};
use futures::{Future, Stream};
use futures::future::{self, BoxFuture, Either};
use futures::stream::{self, BoxStream};
use tokio_core::reactor::{Core, Timeout};

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

fn fut_get() -> BoxFuture<(), ()> {
    future::lazy(|| Ok::<_, ()>(())).boxed()
}

fn coroutine_panic(await: &Await, status: Status) {
    let fut = fut_get();
    status.0.set(true);
    drop(await.future(fut));
    status.1.set(true);
}

/// Check cleaning up the coroutines if the core is dropped and the coroutines haven't resolved
/// yet.
#[test]
fn cleanup_panic() {
    // Do the test with a configuration parameter ‒ it should not play any role, but check anyway
    fn do_test(leak_on_panic: bool) {
        let coroutine_get = |handle, status: Status| {
            Coroutine::new(handle)
                .leak_on_panic(leak_on_panic)
                .spawn(move |await| coroutine_panic(await, status))
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

        // If we start another similar coroutine now, it gets destroyed on the call to the await right
        // away.
        status.0.set(false);
        let finished = coroutine_get(handle, status.clone());
        status.after_drop(true);
        finished.wait().unwrap_err();
    }

    do_test(true);
    do_test(false);
}

fn coroutine_nopanic(await: &Await, status: Status) {
    let fut = fut_get();
    status.0.set(true);
    await.future_cleanup(fut).unwrap_err();
    status.1.set(true);
    // Another one still returns error
    let fut2 = fut_get();
    await.future_cleanup(fut2).unwrap_err();
}

/// Check cleaning up with manual handling of being dropped.
#[test]
fn cleanup_nopanic() {
    let core = Core::new().unwrap();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move |await| {
        coroutine_nopanic(await, status_cp);
    });

    status.before_drop();
    // The coroutine finishes once we drop the core. Note that it finishes sucessfully, not
    // panicking.
    drop(core);
    status.after_drop(false);
    finished.wait().unwrap();
}

// Make the core go away by panicking inside a closure that holds it
fn panic_core(core: Core) {
    panic::catch_unwind(AssertUnwindSafe(|| {
            // Steal the core into the closure
            let _core = core;
            // And panic here, so the core gets destroyed during an unwind
            panic!();
        }))
        .unwrap_err();
}

/// The cleanup method handles panicking in the main thread correctly.
#[test]
fn cleanup_main_panic() {
    fn do_test(leak_on_panic: bool) {
        let core = Core::new().unwrap();
        let status = Status::default();
        let status_cp = status.clone();
        let finished = Coroutine::new(core.handle())
            .leak_on_panic(leak_on_panic)
            .spawn(move |await| coroutine_nopanic(await, status_cp));
        status.before_drop();
        panic_core(core);
        status.after_drop(false);
        finished.wait().unwrap();
    }
    do_test(true);
    do_test(false);
}

/// When we panic and drop the core and we ask to leak instead of double-panic, make sure it is
/// done so.
#[test]
fn leak_main_panic() {
    let core = Core::new().unwrap();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::new(core.handle())
        .leak_on_panic(true)
        .spawn(move |await| coroutine_panic(await, status_cp));
    status.before_drop();
    panic_core(core);
    // The status hasn't changed, as no unwinding in the coroutine happened
    status.before_drop();
    // And the future is *not* resolved. Test it by creating a new core and providing a timeout
    // that overtakes it.
    let mut core = Core::new().unwrap();
    let timeout = Timeout::new(Duration::from_millis(200), &core.handle()).unwrap();
    let combined = finished.select2(timeout);
    match core.run(combined) {
        Err(_) => unreachable!(),
        Ok(Either::A(_)) => panic!("The coroutine resolved"),
        Ok(Either::B(_)) => (),
    }
}

fn no_stream() -> BoxStream<(), ()> {
    stream::futures_unordered(vec![futures::empty::<(), ()>()]).boxed()
}

/// Tests the explicit no-panic cleanup of a coroutine blocked on a stream.
#[test]
fn stream_cleanup() {
    fn do_test(leak_on_panic: bool) {
        let core = Core::new().unwrap();
        let no_stream = no_stream();
        let status = Status::default();
        let status_cp = status.clone();
        let finished = Coroutine::new(core.handle())
            .leak_on_panic(leak_on_panic)
            .spawn(move |await| {
                status_cp.0.set(true);
                for item in await.stream_cleanup(no_stream) {
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
    do_test(true);
    do_test(false);
}

/// Tests the implicit panic-based cleanup of a stream
#[test]
fn stream_panic() {
    let core = Core::new().unwrap();
    let no_stream = no_stream();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move |await| {
        status_cp.0.set(true);
        for _ in await.stream(no_stream) {
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
