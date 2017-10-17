// XXX Docs
use futures::{Future, Stream};

use errors::Dropped;
use wrappers::{CleanupIterator, OkIterator, ResultIterator};

pub use super::Coroutine;

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

// Once we have non-static futures, we want...
// TODO: Getting one element out of a stream
// TODO: Pushing one element into a sink
