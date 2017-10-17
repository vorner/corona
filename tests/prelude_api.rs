extern crate corona;
extern crate futures;
#[macro_use]
extern crate galvanic_test;
extern crate tokio_core;

test_suite! {
    name prelude_api;
    use std::cell::Cell;

    use futures::{future, stream};
    use tokio_core::reactor::Core;

    use corona::Coroutine;
    use corona::prelude::*;

    type Coro = fn() -> u32;

    fixture coro(coro: Cell<Coro>) -> () {
        params {
            let params: Vec<Coro> = vec![
                || future::ok::<_, ()>(42).coro_wait().unwrap(),
                || stream::once::<_, ()>(Ok(42)).iter_ok().sum(),
                || stream::iter(vec![Ok(42), Err(()), Ok(100)]).iter_ok().sum(),
                || {
                    stream::iter(vec![Ok(12), Err(()), Ok(30)])
                        .iter_result()
                        .filter_map(Result::ok)
                        .sum()
                },
            ];
            params.into_iter().map(Cell::new)
        }

        setup(&mut self) { }
    }

    test fourty_two(coro) {
        let mut core = Core::new().unwrap();
        let coro = coro.params.coro.replace(|| panic!());
        let all_done = Coroutine::with_defaults(core.handle(), coro);
        assert_eq!(42, core.run(all_done).unwrap());
    }
}
