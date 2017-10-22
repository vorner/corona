// XXX Docs

use std::iter;

use futures::{Future, Sink, Stream};

use errors::Dropped;
use wrappers::{CleanupIterator, OkIterator, ResultIterator, SinkSender, StreamExtractor};

pub use coroutine::Coroutine;

pub trait CoroutineFuture: Sized {
    type Item;
    type Error;
    fn coro_wait_cleanup(self) -> Result<Result<Self::Item, Self::Error>, Dropped>;
    fn coro_wait(self) -> Result<Self::Item, Self::Error> {
        self.coro_wait_cleanup().unwrap()
    }
}

impl<I, E, F: Future<Item = I, Error = E>> CoroutineFuture for F {
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
    fn extractor(&mut self) -> StreamExtractor<Self>;
    fn coro_next_cleanup(&mut self) -> Result<Result<Option<Self::Item>, Self::Error>, Dropped>;
    fn coro_next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        self.coro_next_cleanup().unwrap()
    }
}

impl<I, E, S: Stream<Item = I, Error = E>> CoroutineStream for S {
    type Item = I;
    type Error = E;
    fn iter_cleanup(self) -> CleanupIterator<Self> {
        CleanupIterator::new(self)
    }
    fn extractor(&mut self) -> StreamExtractor<Self> {
        StreamExtractor::new(self)
    }
    fn coro_next_cleanup(&mut self) -> Result<Result<Option<Self::Item>, Self::Error>, Dropped> {
        self.extractor().coro_wait_cleanup()
    }
}

pub trait CoroutineSink: Sized {
    type Item;
    type Error;
    // Yay, that's a generic mouthful…
    fn coro_sender<Iter, I>(&mut self, iter: I) -> SinkSender<Self::Item, Self, Iter>
    where
        Iter: Iterator<Item = Self::Item>,
        I: IntoIterator<Item = Self::Item, IntoIter = Iter>;
    fn coro_send(&mut self, item: Self::Item) -> Result<(), Self::Error>;
    fn coro_send_cleanup(&mut self, item: Self::Item) -> Result<Result<(), Self::Error>, Dropped>;
    fn coro_send_many<Iter, I>(&mut self, iter: I) -> Result<Result<(), Self::Error>, Dropped>
    where
        Iter: Iterator<Item = Self::Item>,
        I: IntoIterator<Item = Self::Item, IntoIter = Iter>;
}

impl<I, E, S: Sink<SinkItem = I, SinkError = E>> CoroutineSink for S {
    type Item = I;
    type Error = E;
    fn coro_sender<Iter, Src>(&mut self, iter: Src) -> SinkSender<Self::Item, Self, Iter>
    where
        Iter: Iterator<Item = Self::Item>,
        Src: IntoIterator<Item = Self::Item, IntoIter = Iter>
    {
        SinkSender::new(self, iter)
    }
    fn coro_send(&mut self, item: Self::Item) -> Result<(), Self::Error> {
        self.coro_sender(iter::once(item)).coro_wait()
    }
    fn coro_send_cleanup(&mut self, item: Self::Item) -> Result<Result<(), Self::Error>, Dropped> {
        self.coro_sender(iter::once(item)).coro_wait_cleanup()
    }
    fn coro_send_many<Iter, Src>(&mut self, iter: Src) -> Result<Result<(), Self::Error>, Dropped>
    where
        Iter: Iterator<Item = Self::Item>,
        Src: IntoIterator<Item = Self::Item, IntoIter = Iter>
    {
        self.coro_sender(iter).coro_wait_cleanup()
    }
}
