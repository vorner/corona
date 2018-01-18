//! This uses multiple cores at once on the same thread.
//!
//! It checks there's no mixup in the TLS.

extern crate corona;
extern crate futures;
extern crate tokio_core;

use corona::prelude::*;
use futures::future;
use tokio_core::reactor::Core;

fn recurse(depth: u8) {
    if depth == 0 {
        return;
    }
    let mut core = Core::new().unwrap();
    let mut builder = Coroutine::new(core.handle());
    builder.stack_size(4096 * 20);
    let coros = (0..4).map(move |_| {
        let d = depth - 1;
        builder.spawn(move || {
                future::ok::<_, ()>(()).coro_wait().unwrap();
                recurse(d);
            })
            .unwrap()
    });
    core.run(future::join_all(coros)).unwrap();
}

#[test]
fn recursive() {
    recurse(4);
}
