// XXX Docs

use futures::Stream;

use errors::Dropped;
use prelude::*;

pub struct CleanupIterator<S>(Option<S>);

impl<S> CleanupIterator<S> {
    pub fn new(stream: S) -> Self {
        CleanupIterator(Some(stream))
    }
    // XXX into_inner
}

impl<I, E, S> Iterator for CleanupIterator<S>
where
    I: 'static,
    E: 'static,
    S: Stream<Item = I, Error = E> + 'static,
{
    type Item = Result<Result<I, E>, Dropped>;
    fn next(&mut self) -> Option<Result<Result<I, E>, Dropped>> {
        let resolved = match self.0.take() {
            Some(stream) => stream.into_future().coro_wait_cleanup(),
            None => return Some(Err(Dropped)), // Dropped in previous attempt to iterate. Still dead.
        };
        let (result, stream) = match resolved {
            Ok(Ok((None, stream))) => (None, Some(stream)),
            Ok(Ok((Some(ok), stream))) => (Some(Ok(Ok(ok))), Some(stream)),
            Ok(Err((err, stream))) => (Some(Ok(Err(err))), Some(stream)),
            Err(Dropped) => (Some(Err(Dropped)), None),
        };
        self.0 = stream;
        result
    }
}

pub struct OkIterator<I>(I);

impl<I> OkIterator<I> {
    pub fn new(inner: I) -> Self {
        OkIterator(inner)
    }
    // XXX into_inner
}

impl<I, E, S> Iterator for OkIterator<CleanupIterator<S>>
where
    I: 'static,
    E: 'static,
    S: Stream<Item = I, Error = E> + 'static,
{
    type Item = I;
    fn next(&mut self) -> Option<I> {
        self.0
            .next()
            .map(Result::unwrap)
            .and_then(Result::ok)
    }
}

pub struct ResultIterator<I>(I);

impl<I> ResultIterator<I> {
    pub fn new(inner: I) -> Self {
        ResultIterator(inner)
    }
    // XXX into_inner
}

impl<I, E, S> Iterator for ResultIterator<CleanupIterator<S>>
where
    I: 'static,
    E: 'static,
    S: Stream<Item = I, Error = E> + 'static,
{
    type Item = Result<I, E>;
    fn next(&mut self) -> Option<Result<I, E>> {
        self.0
            .next()
            .map(Result::unwrap)
    }
}
