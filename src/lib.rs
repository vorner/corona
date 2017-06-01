extern crate context;
extern crate futures;
extern crate tokio_core;
extern crate typed_arena;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::{Async, Poll};
use futures::future::{Future, IntoFuture};
use futures::unsync::oneshot::{self, Receiver};
use tokio_core::reactor::Handle as TokioHandle;
use typed_arena::Arena;

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

extern "C" fn coroutine_function(t: Transfer) -> ! {
    // TODO: Extract the information we need
    // TODO: Loop through picking up the unstarted tasks
    unreachable!();
}

struct Internal {
    handle: TokioHandle,
    in_coroutine: Cell<bool>,
    unstarted: RefCell<VecDeque<Box<FnOnce()>>>,

    stacks: Arena<ProtectedFixedSizeStack>,
}

pub struct Scheduler(Rc<Internal>);

impl Scheduler {
    pub fn new(handle: TokioHandle) -> Self {
        let internal = Internal {
            handle,
            in_coroutine: Cell::new(false),
            unstarted: RefCell::new(VecDeque::new()),
            stacks: Arena::new(),
        };
        Scheduler(Rc::new(internal))
    }
    pub fn spawn<R, F>(&self, f: F) -> Spawned<R::Item, R::Error>
        where
            F: FnOnce() -> R + 'static,
            R: IntoFuture,
            R::Item: 'static,
            R::Error: 'static,
            R::Future: 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let weak = Rc::downgrade(&self.0);
        let task = move || {
            let done = f()
                .into_future()
                .then(|r| {
                    drop(sender.send(r));
                    Ok(())
                });
            weak.upgrade().map(|internal| {
                internal.handle.spawn(done);
            });
        };
        self.0.unstarted.borrow_mut().push_back(Box::new(task));
        self.try_running();
        Spawned {
            recv: receiver,
            alive: true,
        }
    }
    pub fn handle(&self) -> Handle {
        Handle(Rc::downgrade(&self.0))
    }
    fn try_running(&self) {
        if self.0.in_coroutine.get() {
            // Run just one coroutine at a time. Start another once we return.
            return;
        }
        if !self.0.unstarted.borrow().is_empty() {
            // We have an unstarted task, start a new coroutine that'll eat it.
            // TODO: Reuse the contexts if there are any unused
            let stack = self.0.stacks.alloc(ProtectedFixedSizeStack::default());
            let context = Context::new(stack, coroutine_function);
            self.run_context(context);
        }
    }
    fn run_context(&self, context: Context) {
        // TODO: Prepare to start (eg. set thread-local variable, etc)
        let transfer = context.resume(0);
        // TODO: Do something with the transfer we got
    }
}

#[derive(Clone)]
pub struct Handle(Weak<Internal>);

impl Handle {
    // TODO: Better error
    pub fn spawn<R, F>(&self, f: F) -> Result<Spawned<R::Item, R::Error>, ()>
        where
            F: FnOnce() -> R + 'static,
            R: IntoFuture,
            R::Item: 'static,
            R::Error: 'static,
            R::Future: 'static,
    {
        self.0
            .upgrade()
            .map(|internal| Scheduler(internal).spawn(f))
            .ok_or(())
    }
}
