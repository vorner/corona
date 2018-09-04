extern crate corona;
extern crate futures;
extern crate tokio_core;

use std::fmt::Debug;

use futures::{future, stream, Future};
use futures::sync::mpsc;
use tokio_core::reactor::Core;

use corona::Coroutine;
use corona::prelude::*;

/// A coroutine test fixture, for convenience and shared methods.
struct Cor {
    core: Core,
    coroutine: Coroutine,
}

impl Cor {
    fn new() -> Cor {
        let core = Core::new().unwrap();
        let handle = core.handle();
        let coroutine = Coroutine::new();
        Cor {
            core,
            coroutine,
        }
    }
    /// Checks that the future resolves to 42
    fn ft<E, F>(&mut self, f: F)
    where
        E: Debug,
        F: Future<Item = u32, Error = E>
    {
        assert_eq!(42, self.core.run(f).unwrap());
    }
    /// Starts a coroutine containing F and checks it returns 42
    fn cor_ft<F: FnOnce() -> u32 + 'static>(&mut self, f: F) {
        let coro = self.coroutine
            .spawn(f)
            .unwrap();
        self.ft(coro);
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
    let mut cor = Cor::new();
    let (mut sender, receiver) = mpsc::channel(1);
    let producer = cor.coroutine.spawn(move || {
            sender.coro_send(2).unwrap();
            sender.coro_send_many(vec![20, 20]).unwrap().unwrap();
        })
        .unwrap();
    cor.cor_ft(move || {
        receiver.iter_ok().sum()
    });
    // The producer is done by now
    producer.wait().unwrap();
}

/// Taking one thing out of a stream
#[test]
fn extract() {
    let mut cor = Cor::new();
    let mut s = stream::once::<_, ()>(Ok(42));
    cor.cor_ft(move || s.coro_next().unwrap().unwrap());
}
