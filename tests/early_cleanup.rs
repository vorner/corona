extern crate corona;
extern crate futures;
extern crate tokio_core;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use corona::Coroutine;
use futures::Future;
use futures::future;
use tokio_core::reactor::{Core, Timeout};

/// Check cleaning up the coroutines if the core is dropped and the coroutines haven't resolved
/// yet.
#[test]
fn cleanup_panic() {
    let core = Core::new().unwrap();
    let handle = core.handle();
    let called_first = Rc::new(Cell::new(false));
    let called_first_cp = called_first.clone();
    let called_second = Rc::new(Cell::new(false));
    let called_second_cp = called_second.clone();
    let holds_rc = Rc::new(());
    let holds_weak = Rc::downgrade(&holds_rc);
    let finished = Coroutine::with_defaults(core.handle(), move |await| {
        let timeout = Timeout::new(Duration::from_millis(100), await.handle()).unwrap();
        called_first_cp.set(true);
        let _holds_rc = holds_rc; // Make sure this one gets stolen to our stack
        drop(await.future(timeout)); // Ignore the result
        called_second_cp.set(true);
    });
    // Check it got to the place where it waits for the future
    assert!(called_first.get());
    assert!(!called_second.get());
    // And the RC is still held by it
    assert!(holds_weak.upgrade().is_some());
    // When we drop the core, it also drops the future and cleans up the coroutine
    drop(core);
    assert!(!called_second.get());
    assert!(holds_weak.upgrade().is_none());
    finished.wait().unwrap_err();

    // If we start another similar coroutine now, it gets destroyed on the call to the await right
    // away.
    called_first.set(false);
    let called_first_cp = called_first.clone();
    let called_second_cp = called_second.clone();
    let holds_rc = Rc::new(());
    let holds_weak = Rc::downgrade(&holds_rc);
    let finished = Coroutine::with_defaults(handle, move |await| {
        let fut = future::lazy(|| Ok::<_, ()>(()));
        called_first_cp.set(true);
        let _holds_rc = holds_rc;
        drop(await.future(fut));
        called_second_cp.set(true);
    });
    assert!(called_first.get());
    assert!(!called_second.get());
    assert!(holds_weak.upgrade().is_none());
    finished.wait().unwrap_err();
}
