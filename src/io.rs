//! Primitives to turn `AsyncRead` and `AsyncWrite` into (coroutine) blocking `Read` and `Write`.

use std::io::{Read, Write, Result as IoResult};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::io;

use super::prelude::*;

/// A wrapper to turn async IO streams into sync ones.
///
/// This can be used to wrap an asynchronous stream ‒ anything that is `AsyncRead` or `AsyncWrite`
/// (like tokio's `TcpStream`) into a sync one. When performing IO, the current coroutine is
/// suspended, but the thread isn't blocked.
///
/// This makes it possible to use blocking API (for example `serde_json::from_reader`) on
/// asynchronous primitives.
///
/// Note that if `T` is `AsyncRead` (or `AsyncWrite`), `&mut T` is too. Therefore, it is possible
/// both to turn the stream into a sync one permanently (or, until the wrapper is unwrapped with
/// [`into_inner`](#method.into_inner)), or just temporarily.
///
/// # Examples
///
/// ```
/// # extern crate corona;
/// # extern crate tokio;
/// use std::io::{Read, Result as IoResult};
/// use corona::io::BlockingWrapper;
/// use tokio::net::TcpStream;
///
/// fn blocking_read(connection: &mut TcpStream) -> IoResult<()> {
///     let mut connection = BlockingWrapper::new(connection);
///     let mut buf = [0u8; 64];
///     // This will block the coroutine, but not the thread
///     connection.read_exact(&mut buf)
/// }
///
/// # fn main() {}
/// ```
///
/// # Panics
///
/// Using the wrapped object may panic in these circumstances:
///
/// * If it is used outside of a coroutine (as there's nothing to suspend at that time).
/// * If the tokio core is dropped while waiting for data.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BlockingWrapper<T>(T);

impl<T> BlockingWrapper<T> {
    /// Wraps the stream and turns it to synchronous one.
    pub fn new(stream: T) -> Self {
        BlockingWrapper(stream)
    }
    /// Accesses the inner stream.
    pub fn inner(&self) -> &T {
        &self.0
    }
    /// Accesses the inner stream mutably.
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.0
    }
    /// Consumes the wrapper and produces the original stream.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for BlockingWrapper<T> {
    fn from(stream: T) -> Self {
        Self::new(stream)
    }
}

impl<T: AsyncRead> Read for BlockingWrapper<T> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        io::read(&mut self.0, buf)
            .coro_wait()
            .map(|(_stream, _buf, size)| size)
    }
}

impl<T: AsyncWrite> Write for BlockingWrapper<T> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        // TODO: The crate contains only write_all, not write that may be short. Implement our own
        // when we have time? Writing everything is surely allowed, but returning earlier might be
        // better for performance in some cases.
        io::write_all(&mut self.0, buf)
            .coro_wait()
            .map(|_| buf.len())
    }
    fn flush(&mut self) -> IoResult<()> {
        io::flush(&mut self.0)
            .coro_wait()
            .map(|_| ())
    }
}
