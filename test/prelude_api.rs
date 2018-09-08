extern crate corona;
extern crate futures;
extern crate tokio_core;

use futures::{future, stream};
use futures::sync::mpsc;
use tokio::runtime::current_thread::Runtime;

use corona::Coroutine;
use corona::prelude::*;

/// A coroutine test fixture, for convenience and shared methods.
struct Cor {
    coroutine: Coroutine,
    runtime: Runtime,
}

impl Cor {
    fn new() -> Cor {
        let runtime = Runtime::new().unwrap();
        let coroutine = Coroutine::new();
        Cor {
            coroutine,
            runtime,
        }
    }
    /// Starts a coroutine containing F and checks it returns 42
    fn cor_ft<F: FnOnce() -> u32 + 'static>(&mut self, f: F) {
        let coroutine = self.coroutine.clone();
        let result = self.runtime.block_on(future::lazy(move || coroutine.spawn(f).unwrap()));
        assert_eq!(42, result.unwrap());
    }
}

/// One future to wait on
#[test]
fn coro_wait() {
    Cor::new().cor_ft(|| future::ok::<_, ()>(42).coro_wait().unwrap());
}

/// A stream with single Ok element
#[test]
fn iter_ok() {
    Cor::new().cor_ft(|| stream::once::<_, ()>(Ok(42)).iter_ok().sum());
}

/// Stream with multiple elements, some errors. This one terminates at the first error.
#[test]
fn iter_ok_many() {
    Cor::new().cor_ft(|| stream::iter_result(vec![Ok(42), Err(()), Ok(100)]).iter_ok().sum());
}

/// A stream with multiple elements, some errors. This one *skips* errors.
#[test]
fn iter_result() {
    Cor::new()
        .cor_ft(|| {
            stream::iter_result(vec![Ok(12), Err(()), Ok(30)])
                .iter_result()
                .filter_map(Result::ok)
                .sum()
        });
}

/// Makes sure we work with non-'static futures. This one touches stuff on the stack.
#[test]
fn reference() {
    Cor::new()
        .cor_ft(|| {
            struct Num(u32);
            let num = Num(42);
            let num_ref = &num;
            future::ok::<_, ()>(num_ref)
                .coro_wait()
                .map(|&Num(num)| num)
                .unwrap()
        });
}

/// Pushing things into a sink, which must switch between the coroutines.
#[test]
fn push_sink() {
    let sum = Coroutine::new().run(|| {
        let (mut sender, receiver) = mpsc::channel(1);
        corona::spawn(move || {
            sender.coro_send(2).unwrap();
            sender.coro_send_many(vec![20, 20]).unwrap().unwrap();
        });

        receiver.iter_ok().sum()
    }).unwrap();
    assert_eq!(42, sum);
}

/// Taking one thing out of a stream
#[test]
fn extract() {
    let mut cor = Cor::new();
    let mut s = stream::once::<_, ()>(Ok(42));
    cor.cor_ft(move || s.coro_next().unwrap().unwrap());
}
