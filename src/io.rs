use std::io::{Read, Write, Result as IoResult};
use tokio_io::{AsyncRead, AsyncWrite};
use tokio_io::io;

use super::prelude::*;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IoWrapper<T>(T);

impl<T> IoWrapper<T> {
    pub fn new(stream: T) -> Self {
        IoWrapper(stream)
    }
    pub fn inner(&self) -> &T {
        &self.0
    }
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.0
    }
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for IoWrapper<T> {
    fn from(stream: T) -> Self {
        Self::new(stream)
    }
}

impl<T: AsyncRead> Read for IoWrapper<T> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        io::read(&mut self.0, buf)
            .coro_wait()
            .map(|(_stream, _buf, size)| size)
    }
}

impl<T: AsyncWrite> Write for IoWrapper<T> {
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
