//! Experiments and tests around reentrant API abuses.
//!
//! As pointed out by matklad (for which he has big thanks), combination of `unsafe`, callback and
//! reentrance can often lead to unwanted results, including UB. This tests how far the API allows
//! one to go.

use corona::prelude::*;
use tokio::prelude::*;
use tokio::runtime::current_thread;

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
    current_thread::block_on_all(future::lazy(|| {
        Coroutine::new()
            .spawn_catch_panic(|| {
                ReentrantPoll::default().coro_wait()
            }).unwrap()
    })).unwrap_err(); // It is expected the coroutine panics
}
