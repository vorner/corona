//! A module for wildcard import.
//!
//! This contains some traits and general types that are meant to be wildcard imported. These are
//! extension traits, attaching more methods to existing types.
//!
//! Each of the main `futures` trait has one extension crate here. Also, the
//! [`Coroutine`](../coroutine/struct.Coroutine.html) is included, as the main type of the library.
//!
//! All of these things are internally delegated to the
//! [`Coroutine::wait`](../coroutine/struct.Coroutine.html#method.wait) method and are mostly for
//! convenience.

use std::iter;
use std::panic;

use futures::{Future, Sink, Stream};

use errors::Dropped;
use wrappers::{CleanupIterator, OkIterator, ResultIterator, SinkSender, StreamExtractor};

pub use coroutine::Coroutine;

/// An extension crate for the `Future` trait.
///
/// This is auto-implemented for everything that implements the `Future` trait, attaching more
/// methods to them.
pub trait CoroutineFuture: Future + Sized {
    /// A coroutine aware wait on the result.
    ///
    /// This blocks the current coroutine until the future resolves and returns the result. This is
    /// similar to `Future::wait`. However, this allows other coroutines to run when this one
    /// waits.
    ///
    /// Note that the future does *not* have to be `'static`.
    ///
    /// # Panics
    ///
    /// This'll panic if the reactor the coroutine was spawned onto is dropped while the method
    /// runs.
    ///
    /// It also panics when called outside of the coroutine and any panics from the coroutine
    /// itself will be propagated to the calling coroutine.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate tokio;
    /// # use std::time::Duration;
    /// # use corona::prelude::*;
    /// # use tokio::clock;
    /// # use tokio::timer::Delay;
    /// # fn main() {
    /// Coroutine::new().run(move || {
    ///     let timeout = Delay::new(clock::now() + Duration::from_millis(50));
    ///     // This would switch to another coroutine if there was one ready.
    ///     // We unwrap, since the error doesn't happen on timeouts.
    ///     timeout.coro_wait().unwrap();
    /// });
    /// # }
    /// ```
    fn coro_wait(self) -> Result<Self::Item, Self::Error> {
       self.coro_wait_cleanup().unwrap_or_else(|_| panic::resume_unwind(Box::new(Dropped)))
    }

    /// A coroutine aware wait on the result that doesn't panic.
    ///
    /// This is just like [`coro_wait`](#method.coro_wait), but instead of panicking when the
    /// reactor is unexpectedly dropped, it returns `Err(Dropped)`. This might be used to implement
    /// manual coroutine cleanup when needed.
    ///
    /// # Panics
    ///
    /// When called outside of the coroutine. Also, panics from within the future are propagated to
    /// the calling (current) coroutine.
    fn coro_wait_cleanup(self) -> Result<Result<Self::Item, Self::Error>, Dropped>;
}

impl<F: Future> CoroutineFuture for F {
    fn coro_wait_cleanup(self) -> Result<Result<Self::Item, Self::Error>, Dropped> {
        Coroutine::wait(self)
    }
}

/// An extension trait for `Stream`s.
///
/// This is auto-implemented for `Stream`s and adds some convenient coroutine-aware methods to
/// them.
pub trait CoroutineStream: Stream + Sized {
    /// Produces an iterator through the successful items of the stream.
    ///
    /// This allows iterating comfortably through the stream. It produces only the successful
    /// items and stops when the stream terminates or when it reaches the first error. The error is
    /// thrown away (you may want to use [`iter_result`](#method.iter_result) if you care about the
    /// errors).
    ///
    /// When it waits for another item to come out of the stream, the coroutine suspends and
    /// switches to others if there are some ready.
    ///
    /// # Panics
    ///
    /// If the reactor is dropped during the iteration, this method panics to clean up the
    /// coroutine.
    ///
    /// It also panics when called from outside of a coroutine. Panics from within the stream are
    /// propagated into the calling coroutine.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate futures;
    /// # use corona::prelude::*;
    /// # use futures::unsync::mpsc;
    /// # fn main() {
    /// let (sender, receiver) = mpsc::unbounded();
    /// sender.unbounded_send(21);
    /// sender.unbounded_send(21);
    /// // Make sure the channel is terminated, or it would wait forever.
    /// drop(sender);
    ///
    /// let result = Coroutine::new().run(move || {
    ///     let mut sum = 0;
    ///     for num in receiver.iter_ok() {
    ///         sum += num;
    ///     }
    ///     sum
    /// });
    /// assert_eq!(42, result.unwrap());
    /// # }
    /// ```
    fn iter_ok(self) -> OkIterator<CleanupIterator<Self>> {
        OkIterator::new(self.iter_cleanup())
    }

    /// Produces an iterator through results.
    ///
    /// This is similar to [`iter_ok`](#method.iter_ok). However, instead of terminating on errors,
    /// the items produced by the iterator are complete `Result`s. The iterator always runs to the
    /// end of the stream (or break out of the `for`).
    ///
    /// # Notes
    ///
    /// In general, streams don't guarantee to be usable past their first error. So, when working
    /// with an unknown stream, it is reasonable to break the `for` on the first error. This is
    /// similar to [`iter_ok`](#method.iter_ok), but allows inspecting the error.
    ///
    /// However, there are some specific streams that are usable past errors. Such example is
    /// `TcpListener::incoming`, which may signal an error accepting one connection, but then keeps
    /// trying.
    ///
    /// # Panics
    ///
    /// This panics when the reactor the current coroutine runs on is dropped while iterating.
    ///
    /// It panics when called outside of a coroutine and any panics from within the stream are
    /// propagated to the calling coroutine.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate futures;
    /// # use corona::prelude::*;
    /// # use futures::unsync::mpsc;
    /// # fn main() {
    /// let (sender, receiver) = mpsc::unbounded();
    /// sender.unbounded_send(21);
    /// sender.unbounded_send(21);
    /// // Make sure the channel is terminated, or it would wait forever.
    /// drop(sender);
    ///
    /// let result = Coroutine::new().run(move || {
    ///     let mut sum = 0;
    ///     for num in receiver.iter_result() {
    ///         sum += num.expect("MPSC should not error");
    ///     }
    ///     sum
    /// });
    /// assert_eq!(42, result.unwrap());
    /// # }
    /// ```
    fn iter_result(self) -> ResultIterator<CleanupIterator<Self>> {
        ResultIterator::new(self.iter_cleanup())
    }

    /// Produces an iterator that doesn't panic on reactor drop.
    ///
    /// This acts like [`iter_result`](#method.iter_result). However, the produced items are
    /// wrapped inside another level of `Result` and it returns `Err(Dropped)` if the reactor is
    /// dropped while iterating instead of panicking. This allows manual coroutine cleanup when
    /// needed, but is probably less convenient for casual use.
    ///
    /// # Panics
    ///
    /// If called outside of a coroutine, or if the stream itself panics internally.
    fn iter_cleanup(self) -> CleanupIterator<Self>;

    /// A future that pulls one item out of the stream.
    ///
    /// This is like `Stream::into_future`, but it doesn't consume and re-produce the stream.
    /// Instead it borrows the stream mutably. Such thing is usable with coroutines, since
    /// coroutines can easily wait on futures that are not `'static`.
    ///
    /// Unlike the other methods here, this only builds the future, doesn't run it.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate futures;
    /// # use corona::prelude::*;
    /// # use futures::unsync::mpsc;
    /// # fn main() {
    /// let (sender, mut receiver) = mpsc::unbounded();
    /// sender.unbounded_send(21);
    /// // The second item is unused
    /// sender.unbounded_send(21);
    /// drop(sender);
    ///
    /// let result = Coroutine::new().run(move || {
    ///     receiver.extractor()
    ///         .coro_wait() // Block until the item actually falls out
    ///         .unwrap() // Unwrap the outer result
    ///         .unwrap() // Unwrap the option, since it gives `Option<T>`
    /// });
    /// assert_eq!(21, result.unwrap());
    /// # }
    fn extractor(&mut self) -> StreamExtractor<Self>;

    /// Pulls one item out of the stream.
    ///
    /// This extracts one item out of the stream, returning either the streams error or the item or
    /// `None` on end of the stream.
    ///
    /// It blocks the current coroutine when waiting for the item to appear.
    ///
    /// # Panics
    ///
    /// It panics when the reactor is dropped while waiting for the item.
    ///
    /// It also panics when called outside of a coroutine or when the stream itself panics.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate futures;
    /// # use corona::prelude::*;
    /// # use futures::unsync::mpsc;
    /// # fn main() {
    /// let (sender, mut receiver) = mpsc::unbounded();
    /// sender.unbounded_send(21);
    /// sender.unbounded_send(21);
    /// drop(sender);
    ///
    /// let result = Coroutine::new().run(move || {
    ///     let mut sum = 0;
    ///     while let Some(num) = receiver.coro_next().unwrap() {
    ///         sum += num;
    ///     }
    ///     sum
    /// });
    /// assert_eq!(42, result.unwrap());
    /// # }
    /// ```
    fn coro_next(&mut self) -> Result<Option<Self::Item>, Self::Error> {
        self.coro_next_cleanup().unwrap()
    }

    /// Pulls one item out of the stream without panicking.
    ///
    /// This is like [`coro_next`](#method.coro_next), but returns `Err(Dropped)` when the reactor
    /// is dropped during the waiting instead of panicking. That allows manual coroutine cleanup.
    ///
    /// # Panics
    ///
    /// When called outside of a coroutine or when the stream itself panics.
    fn coro_next_cleanup(&mut self) -> Result<Result<Option<Self::Item>, Self::Error>, Dropped> {
        self.extractor().coro_wait_cleanup()
    }
}

impl<S: Stream> CoroutineStream for S {
    fn iter_cleanup(self) -> CleanupIterator<Self> {
        CleanupIterator::new(self)
    }
    fn extractor(&mut self) -> StreamExtractor<Self> {
        StreamExtractor::new(self)
    }
}

/// An extension trait for `Sink`.
///
/// This is automatically implemented for `Sink`s and adds some convenience methods to them.
pub trait CoroutineSink: Sink + Sized {
    /// Sends one item into the sink.
    ///
    /// This is similar to `Sink::send`, but doesn't consume the sink, only borrows it mutably.
    /// This is more convenient with the coroutines, because they can wait on something that is not
    /// `'static`.
    ///
    /// # Parameters
    ///
    /// * `item`: The item to be sent.
    ///
    /// # Panics
    ///
    /// If the reactor is dropped before the sending is done.
    ///
    /// If it is called outside of a coroutine or if the sink panics internally.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate futures;
    /// # use corona::prelude::*;
    /// # use futures::unsync::mpsc;
    /// # fn main() {
    /// let (mut sender, mut receiver) = mpsc::channel(1);
    /// let result = Coroutine::new().run(move || {
    ///     corona::spawn(move || {
    ///         sender.coro_send(42).unwrap();
    ///     });
    ///     receiver.coro_next().unwrap()
    /// });
    /// assert_eq!(Some(42), result.unwrap());
    /// # }
    /// ```
    fn coro_send(&mut self, item: Self::SinkItem) -> Result<(), Self::SinkError> {
        self.coro_sender(iter::once(item)).coro_wait()
    }

    /// Sends one item into the sink without panicking on dropped reactor.
    ///
    /// This sends one item into the sink, similar to [`coro_send`](#method.coro_send). The
    /// difference is it doesn't panic on dropped reactor. Instead, it returns `Err(Dropped)` and
    /// allows manual cleanup of the coroutine.
    ///
    /// # Parameters
    ///
    /// * `item`: The item to be sent.
    ///
    /// # Panics
    ///
    /// If it is called outside of a coroutine or if the sink itself panics.
    fn coro_send_cleanup(&mut self, item: Self::SinkItem)
        -> Result<Result<(), Self::SinkError>, Dropped>
    {
        self.coro_sender(iter::once(item)).coro_wait_cleanup()
    }

    /// Sends multiple items into the sink.
    ///
    /// This is like [`coro_send_cleanup`](#method.coro_send_cleanup). However, it sends multiple
    /// items instead of one. This is potentially faster than pushing them one by one, since the
    /// sink „flushes“ just once after the whole batch.
    ///
    /// # Parameters
    ///
    /// * `iter`: Iterator over the items to send.
    ///
    /// # Panics
    ///
    /// If it is called outside of a coroutine or if the sink panics internally.
    fn coro_send_many<I>(&mut self, iter: I) -> Result<Result<(), Self::SinkError>, Dropped>
    where
        I: IntoIterator<Item = Self::SinkItem>
    {
        self.coro_sender(iter).coro_wait_cleanup()
    }

    /// Creates a future that sends multiple items into the sink.
    ///
    /// This is the internal future of [`coro_send_many`](#method.coro_send_many). The difference
    /// is, it doesn't wait for the future to resolve, only returns it.
    ///
    /// It can be used to combine the future with something else, like sending to multiple sinks in
    /// parallel.
    ///
    /// # Parameters
    ///
    /// * `iter`: The iterator over items to be sent.
    fn coro_sender<I>(&mut self, iter: I) -> SinkSender<Self::SinkItem, Self, I::IntoIter>
    where
        I: IntoIterator<Item = Self::SinkItem>;
}

impl<S: Sink> CoroutineSink for S {
    fn coro_sender<Src>(&mut self, iter: Src) -> SinkSender<Self::SinkItem, Self, Src::IntoIter>
    where
        Src: IntoIterator<Item = Self::SinkItem>
    {
        SinkSender::new(self, iter)
    }
}
