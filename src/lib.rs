extern crate futures;
extern crate tokio_core;

use futures::{Async, Poll};
use futures::future::{Future, IntoFuture};
use futures::unsync::oneshot::Receiver;
use tokio_core::reactor::Handle as TokioHandle;

pub struct Spawned<S, E> {
    recv: Receiver<Result<S, E>>,
    alive: bool,
}

impl<S, E> Future for Spawned<S, E> {
    type Item = S;
    type Error = E;
    fn poll(&mut self) -> Poll<S, E> {
        if self.alive {
            match self.recv.poll() {
                Ok(Async::Ready(Ok(s))) => Ok(Async::Ready(s)),
                Ok(Async::Ready(Err(e))) => Err(e),
                Ok(Async::NotReady) => Ok(Async::NotReady),
                Err(_) => {
                    // This one will never be ready :-(
                    self.alive = false;
                    Ok(Async::NotReady)
                },
            }
        } else {
            Ok(Async::NotReady)
        }
    }
}

#[derive(Debug)]
struct Internal {
    handle: TokioHandle,

}

impl Internal {
    fn spawn<R, F>(&self, f: F) -> Spawned<R::Item, R::Error>
        where
            F: FnOnce(Handle) -> R,
            R: IntoFuture,
    {
        drop(f);
        unimplemented!()
    }
}

#[derive(Debug)]
pub struct Scheduler(Box<Internal>);

impl Scheduler {
    pub fn new(handle: TokioHandle) -> Self {
        let internal = Internal {
            handle
        };
        Scheduler(Box::new(internal))
    }
    pub fn spawn<R, F>(&self, f: F) -> Spawned<R::Item, R::Error>
        where
            F: FnOnce(Handle) -> R,
            R: IntoFuture,
    {
        self.0.spawn(f)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Handle<'a>(&'a Internal);

impl<'a> Handle<'a> {
    pub fn spawn<R, F>(&self, f: F) -> Spawned<R::Item, R::Error>
        where
            F: FnOnce(Handle) -> R,
            R: IntoFuture,
    {
        self.0.spawn(f)
    }
}
