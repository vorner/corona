//! Experiments and tests around reentrant API abuses.
//!
//! As pointed out by matklad (for which he has big thanks), combination of `unsafe`, callback and
//! reentrance can often lead to unwanted results, including UB. This tests how far the API allows
//! one to go.

use corona::prelude::*;
use futures::{Async, Future, Poll};
use tokio_core::reactor::Core;

#[derive(Debug, Default)]
struct ReentrantPoll {
    reentered: bool,
}

impl Future for ReentrantPoll {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Poll<(), ()> {
        if self.reentered {
            return Ok(Async::Ready(()));
        }
        self.reentered = true;
        // Wait on self
        match self.coro_wait() {
            Ok(()) => Ok(Async::Ready(())),
            Err(()) => Err(()),
        }
    }
}

/// A future directly tries to wait on itself from its own poll. That should panic, since *usually*
/// the poll is called from the core, that should live outside of the coroutines.
///
/// The panic prevents any potential problems caused by this.
#[test]
fn directly_reentrant() {
    let mut core = Core::new().unwrap();
    let coroutine = Coroutine::new(core.handle())
        .spawn_catch_panic(|| {
            ReentrantPoll::default().coro_wait()
        })
        .unwrap();
    // It is expected the coroutine panics
    core.run(coroutine).unwrap_err();
}

/// We try to cheat the restriction of waiting outside of a coroutine.
///
/// We place the core into a coroutine itself. Unfortunately, the suspension point inside the
/// `core.run` leads to a deadlockish situation ‒ the second coroutine will never resume and won't
/// be finished.
///
/// Any idea how to cheat the deadlock thing?
///
/// Still, even if it did resolve, the only thing it could do is to create multiple `Task` wrappers
/// around the same future and call it from multiple tasks, which is also legal (though most likely
/// not what you want).
#[test]
fn coro_reentrant() {
    let mut core = Core::new().unwrap();
    let first = Coroutine::new(core.handle())
        .spawn_catch_panic(|| {
            ReentrantPoll::default().coro_wait()
        })
        .unwrap();
    let handle = core.handle();
    Coroutine::with_defaults(handle, move || {
        core.run(first).unwrap().unwrap();
    });
}
