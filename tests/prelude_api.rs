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
                // One future to wait on
                || future::ok::<_, ()>(42).coro_wait().unwrap(),
                // A stream with single Ok element
                || stream::once::<_, ()>(Ok(42)).iter_ok().sum(),
                // Stream with multiple elements, some errors. This one terminates at the first
                // error.
                || stream::iter(vec![Ok(42), Err(()), Ok(100)]).iter_ok().sum(),
                // A stream with multiple elements, some errors. This one *skips* errors.
                || {
                    stream::iter(vec![Ok(12), Err(()), Ok(30)])
                        .iter_result()
                        .filter_map(Result::ok)
                        .sum()
                },
                // Makes sure we work with non-'static futures. This one touches stuff on the
                // stack.
                || {
                    struct Num(u32);
                    let num = Num(42);
                    let num_ref = &num;
                    future::ok::<_, ()>(num_ref)
                        .coro_wait()
                        .map(|&Num(num)| num)
                        .unwrap()
                },
            ];
            params.into_iter().map(Cell::new)
        }

        setup(&mut self) { }
    }

    /// Runs a coroutine that likely waits on something.
    ///
    /// It checks the output is 42. Runs with different coroutines, see the fixture.
    test fourty_two(coro) {
        let mut core = Core::new().unwrap();
        let coro = coro.params.coro.replace(|| panic!());
        let all_done = Coroutine::with_defaults(core.handle(), coro);
        assert_eq!(42, core.run(all_done).unwrap());
    }
}
