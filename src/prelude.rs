// XXX Docs
use futures::Future;

use errors::Dropped;
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

#[cfg(test)]
mod tests {
    use super::*;

    use futures::future;
    use tokio_core::reactor::Core;

    #[test]
    fn coro_wait() {
        let mut core = Core::new().unwrap();
        let all_done = Coroutine::with_defaults(core.handle(), || {
            future::ok::<_, ()>(42).coro_wait().unwrap() // 42
        });
        assert_eq!(42, core.run(all_done).unwrap());
    }
}
