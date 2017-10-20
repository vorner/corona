// XXX Docs

use futures::{Async, AsyncSink, Future, Poll, Sink, Stream};

use errors::Dropped;
use prelude::*;

pub struct CleanupIterator<S>(Option<S>);

impl<S> CleanupIterator<S> {
    pub fn new(stream: S) -> Self {
        CleanupIterator(Some(stream))
    }
    // XXX into_inner
}

impl<I, E, S: Stream<Item = I, Error = E>> Iterator for CleanupIterator<S> {
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

impl<I, E, S: Stream<Item = I, Error = E>> Iterator for OkIterator<CleanupIterator<S>> {
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

impl<I, E, S: Stream<Item = I, Error = E>> Iterator for ResultIterator<CleanupIterator<S>> {
    type Item = Result<I, E>;
    fn next(&mut self) -> Option<Result<I, E>> {
        self.0
            .next()
            .map(Result::unwrap)
    }
}

pub struct SinkSender<'a, V, S: 'a, I: Iterator<Item = V>> {
    sink: &'a mut S,
    iter: Option<I>,
    value: Option<V>,
}

impl<'a, V, S: 'a, I: Iterator<Item = V>> SinkSender<'a, V, S, I> {
    pub fn new<Src: IntoIterator<IntoIter = I, Item = V>>(sink: &'a mut S, src: Src) -> Self {
        let iter = src.into_iter();
        Self {
            sink,
            iter: Some(iter),
            value: None,
        }
    }
    // Pull the next value from somewhere.
    fn next(&mut self) -> Option<V> {
        // A postponed value
        if self.value.is_some() {
            eprintln!("Postponed value");
            return self.value.take();
        }
        // If we have nothing postponed, try pulling it from an iterator, if we have one.
        let result = self.iter.as_mut().and_then(Iterator::next);
        // If we got nothing, then make sure we don't call the iterator again.
        if result.is_none() {
            self.iter = None;
        }
        result
    }
}

impl<'a, V, E, S, I> Future for SinkSender<'a, V, S, I>
where
    S: Sink<SinkItem = V, SinkError = E> + 'a,
    I: Iterator<Item = V>,
{
    type Item = ();
    type Error = E;
    fn poll(&mut self) -> Poll<(), E> {
        // First, try to push as much inside as possible.
        while let Some(value) = self.next() {
            match self.sink.start_send(value) {
                Err(e) => return Err(e), // Early abort on errors.
                Ok(AsyncSink::NotReady(returned)) => {
                    // This item doesn't fit. Hold onto it until we are called again.
                    self.value = Some(returned);
                    return Ok(Async::NotReady);
                },
                Ok(AsyncSink::Ready) => (), // Accepted, try next one.
            }
        }
        // By now, we put everything into the sink. Try flushing it.
        self.sink.poll_complete()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::sync::mpsc;
    use tokio_core::reactor::Core;

    /// A test checking that sink_sender feeds everything to the sink.
    ///
    /// This one doesn't do much async things, though, as everything fits inside right away.
    #[test]
    fn sink_sender() {
        let (mut sender, receiver) = mpsc::unbounded();
        let data = vec![1, 2, 3];
        {
            let sender_fut = SinkSender::new(&mut sender, data.clone());
            // Just plain old future's wait. No coroutines here.
            sender_fut.wait().unwrap();
        }
        drop(sender); // EOF the channel
        // The data is there.
        let received = receiver.wait().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(data, received);
    }

    /// An async version of the above.
    ///
    /// It needs to switch between the two futures to complete, because not everything fits.
    #[test]
    fn async_sink_sender() {
        let (mut sender, receiver) = mpsc::channel(1);
        let mut core = Core::new().unwrap();
        let sending_fut = Coroutine::with_defaults(core.handle(), move || {
            let data = vec![1, 2, 3];
            Coroutine::wait(SinkSender::new(&mut sender, data))
                .unwrap()
                .unwrap();
        });
        let receiving_fut = Coroutine::with_defaults(core.handle(), move || {
            let mut result = Vec::new();
            Coroutine::wait(receiver.for_each(|val| {
                    result.push(val);
                    Ok(())
                }))
                .unwrap()
                .unwrap();
            assert_eq!(vec![1, 2, 3], result);
        });
        core.run(receiving_fut).unwrap();
        core.run(sending_fut).unwrap();
    }
}
