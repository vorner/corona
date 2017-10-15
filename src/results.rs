use std::any::Any;

use futures::{Async, Future, Poll, Stream};
use futures::unsync::oneshot::Receiver;
use futures::unsync::mpsc::Receiver as ChannelReceiver;

/*
use errors::{Dropped, TaskFailed, TaskResult};
use super::Await;

/// A wrapper to asynchronously iterate through a stream.
///
/// This wraps a `future::Stream` in a way it can be directly used as an iterator. When waiting for
/// the next item it yields control to other coroutines and tasks on the current
/// `tokio::reactor::Core` and will get woken up once the next item (or the end) is available.
///
/// The caller probably doesn't have to come into the contact with this type directly, as the usual
/// way of operation through the [`Await::stream`](../struct.Await.html#method.stream).
///
/// # Panics
///
/// In case the related `Core` which the current coroutine runs on is dropped without the coroutine
/// terminates, operations with this type may panic to unwind the coroutine's stack and clean up
/// memory (depending on the configuration, once it is implemented).
///
/// # Examples
///
/// ```
/// # extern crate corona;
/// # extern crate futures;
/// # extern crate tokio_core;
/// use corona::Coroutine;
/// use futures::stream;
/// use tokio_core::reactor::Core;
///
/// # fn main() {
/// let mut core = Core::new().unwrap();
/// let coroutine = Coroutine::with_defaults(core.handle(), |await| {
///     let stream = stream::empty::<(), ()>();
///     for item in await.stream(stream) {
///         // Streams can contain errors, so it iterates over `Result`s.
///         let item = item.unwrap();
///         // Process them here
///     }
/// });
/// core.run(coroutine).unwrap();
/// # }
/// ```
pub struct StreamIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    await: &'a Await,
    stream: Option<S>,
}

impl<'a, I, E, S> StreamIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    pub(crate) fn new(await: &'a Await, stream: S) -> Self {
        Self {
            await,
            stream: Some(stream),
        }
    }
}

impl<'a, I, E, S> Iterator for StreamIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    type Item = Result<I, E>;
    fn next(&mut self) -> Option<Self::Item> {
        let fut = self.stream.take().unwrap().into_future();
        let resolved = self.await.future(fut);
        let (result, stream) = match resolved {
            Ok((None, stream)) => (None, stream),
            Ok((Some(ok), stream)) => (Some(Ok(ok)), stream),
            Err((e, stream)) => (Some(Err(e)), stream),
        };
        self.stream = Some(stream);
        result
    }
}

/// A wrapper to asynchronously iterate through a stream, handling coroutine cleanup.
///
/// This is like [`StreamIterator`](struct.StreamIterator.html), but it doesn't panic when the
/// coroutine needs to be cleaned up. Instead, it returns `Err(Dropped)` in such case and leaves
/// the stack cleanup to the caller.
///
/// See [`Await::stream_cleanup`](../struct.Await.html#method.stream_cleanup).
pub struct StreamCleanupIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    await: &'a Await,
    stream: Option<S>,
}

impl <'a, I, E, S> StreamCleanupIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    pub(crate) fn new(await: &'a Await, stream: S) -> Self {
        Self {
            await,
            stream: Some(stream),
        }
    }
}

impl<'a, I, E, S> Iterator for StreamCleanupIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    type Item = Result<Result<I, E>, Dropped>;
    fn next(&mut self) -> Option<Self::Item> {
        let fut = match self.stream.take() {
            Some(s) => s.into_future(),
            None => return Some(Err(Dropped)),
        };
        let resolved = self.await.future_cleanup(fut);
        let (result, stream) = match resolved {
            Ok(Ok((None, stream))) => (None, stream),
            Ok(Ok((Some(ok), stream))) => (Some(Ok(Ok(ok))), stream),
            Ok(Err((e, stream))) => (Some(Ok(Err(e))), stream),
            Err(Dropped) => return Some(Err(Dropped)),
        };
        self.stream = Some(stream);
        result
    }
}

/// A `Stream` representing the produced items from a generator.
///
/// The stream will produce the items and then terminate when the generator coroutine terminates.
/// If the coroutine panics, it produces an error.
pub struct GeneratorResult<Item> {
    receiver: ChannelReceiver<Result<Item, Box<Any + Send + 'static>>>,
}

impl<Item> GeneratorResult<Item> {
    pub(crate) fn new(receiver: ChannelReceiver<Result<Item, Box<Any + Send + 'static>>>) -> Self {
        Self { receiver }
    }
}

impl<Item> Stream for GeneratorResult<Item> {
    type Item = Item;
    type Error = Box<Any + Send + 'static>;
    fn poll(&mut self) -> Poll<Option<Item>, Self::Error> {
        match self.receiver.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Ok(Async::Ready(Some(Ok(item)))) => Ok(Async::Ready(Some(item))),
            Ok(Async::Ready(Some(Err(e)))) => Err(e),
            Err(_) => unreachable!("Error from mpsc channel ‒ Can Not Happen"),
        }
    }
}
*/
