// XXX Docs
use futures::{Future, Stream};

use errors::Dropped;
use wrappers::{CleanupIterator, OkIterator, ResultIterator};
use super::Coroutine;

pub trait CoroutineFuture: Sized {
    type Item;
    type Error;
    fn coro_wait_cleanup(self) -> Result<Result<Self::Item, Self::Error>, Dropped>;
    fn coro_wait(self) -> Result<Self::Item, Self::Error> {
        self.coro_wait_cleanup().unwrap()
    }
}

impl<I, E, F> CoroutineFuture for F
where
    I: 'static,
    E: 'static,
    F: Future<Item = I, Error = E> + 'static,
{
    type Item = I;
    type Error = E;
    fn coro_wait_cleanup(self) -> Result<Result<I, E>, Dropped> {
        Coroutine::wait(self)
    }
}

pub trait CoroutineStream: Sized {
    type Item;
    type Error;
    fn iter_cleanup(self) -> CleanupIterator<Self>;
    fn iter_ok(self) -> OkIterator<CleanupIterator<Self>> {
        OkIterator::new(self.iter_cleanup())
    }
    fn iter_result(self) -> ResultIterator<CleanupIterator<Self>> {
        ResultIterator::new(self.iter_cleanup())
    }
}

impl<I, E, S> CoroutineStream for S
where
    I: 'static,
    E: 'static,
    S: Stream<Item = I, Error = E> + 'static,
{
    type Item = I;
    type Error = E;
    fn iter_cleanup(self) -> CleanupIterator<Self> {
        CleanupIterator::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::{future, stream};
    use tokio_core::reactor::Core;

    /// Just a convenience macro to unify bunch of very similar tests
    macro_rules! fourty_two {
        ($($(#[$meta: meta])* fn $name: ident () $coro: block)*) => {
            $(
            $(#[$meta])*
            #[test]
            fn $name() {
                let mut core = Core::new().unwrap();
                let all_done = Coroutine::with_defaults(core.handle(), || $coro);
                assert_eq!(42, core.run(all_done).unwrap());
            }
            )*
        }
    }

    fourty_two! {
        /// Wait for a future result.
        fn coro_wait() {
            future::ok::<_, ()>(42).coro_wait().unwrap()
        }

        /// Asynchronous iteration of successful items.
        fn coro_iter() {
            stream::once::<_, ()>(Ok(42)).iter_ok().sum()
        }

        /// Asynchronous iteration of successful items stops at the first error.
        fn coro_iter_err() {
            stream::iter(vec![Ok(42), Err(()), Ok(100)]).iter_ok().sum()
        }

        /// All results are passed on through the iterator.
        fn coro_iter_result() {
            stream::iter(vec![Ok(12), Err(()), Ok(30)])
                .iter_result()
                .filter_map(Result::ok)
                .sum()
        }
    }
}
