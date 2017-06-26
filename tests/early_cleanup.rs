extern crate corona;
extern crate futures;
extern crate tokio_core;

use std::cell::Cell;
use std::rc::Rc;

use corona::Coroutine;
use futures::Future;
use futures::future;
use tokio_core::reactor::Core;

#[derive(Clone, Default)]
struct Status(Rc<Cell<bool>>, Rc<Cell<bool>>);

/// Check cleaning up the coroutines if the core is dropped and the coroutines haven't resolved
/// yet.
#[test]
fn cleanup_panic() {
    let coroutine_get = |handle, status: Status| {
        Coroutine::with_defaults(handle, move |await| {
            let fut = future::lazy(|| Ok::<_, ()>(()));
            status.0.set(true);
            drop(await.future(fut));
            status.1.set(true);
        })
    };
    let core = Core::new().unwrap();
    let handle = core.handle();
    let status = Status::default();
    let finished = coroutine_get(core.handle(), status.clone());
    // Check it got to the place where it waits for the future
    assert!(status.0.get());
    assert!(!status.1.get());
    // And the RC is still held by it
    assert_eq!(2, Rc::strong_count(&status.0));
    // When we drop the core, it also drops the future and cleans up the coroutine
    drop(core);
    assert!(!status.1.get());
    assert_eq!(1, Rc::strong_count(&status.0));
    finished.wait().unwrap_err();

    // If we start another similar coroutine now, it gets destroyed on the call to the await right
    // away.
    status.0.set(false);
    let finished = coroutine_get(handle, status.clone());
    assert!(status.0.get());
    assert!(!status.1.get());
    assert_eq!(1, Rc::strong_count(&status.0));
    finished.wait().unwrap_err();
}

/// Check cleaning up with manual handling of being dropped.
#[test]
fn cleanup_nopanic() {
    let core = Core::new().unwrap();
    let status = Status::default();
    let status_cp = status.clone();
    let finished = Coroutine::with_defaults(core.handle(), move |await| {
        let fut = future::lazy(|| Ok::<_, ()>(()));
        status_cp.0.set(true);
        await.future_cleanup(fut).unwrap_err();
        status_cp.1.set(true);
        // Another one still returns error
        let fut2 = future::lazy(|| Ok::<_, ()>(()));
        await.future_cleanup(fut2).unwrap_err();
    });
    assert!(status.0.get());
    assert!(!status.1.get());
    // And the RC is still held by it
    assert_eq!(2, Rc::strong_count(&status.0));

    // The coroutine finishes once we drop the core. Note that it finishes sucessfully, not
    // panicking.
    drop(core);
    assert!(status.1.get());
    // And the RC is still held by it
    assert_eq!(1, Rc::strong_count(&status.0));
    finished.wait().unwrap();
}
