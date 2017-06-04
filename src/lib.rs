extern crate context;
extern crate futures;
extern crate tokio_core;

use std::any::Any;
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::{Future, Async, Poll};
use futures::unsync::oneshot::{self, Receiver};
use tokio_core::reactor::Handle;

enum TaskResult<R> {
    Panicked(Box<Any + Send + 'static>),
    Finished(R),
}

#[derive(Debug)]
pub enum TaskFailed {
    Panicked(Box<Any + Send + 'static>),
    // Can this actually happen?
    Lost,
}

pub struct Await<'a> {
    transfer: &'a mut Transfer,
    handle: &'a Handle,
}

impl<'a> Await<'a> {
    pub fn handle(&self) -> &Handle {
        self.handle
    }
}

trait BoxableTask {
    fn perform(&mut self, Transfer) -> Transfer;
}

impl<F: FnOnce(Transfer) -> Transfer> BoxableTask for Option<F> {
    fn perform(&mut self, transfer: Transfer) -> Transfer {
        self.take().unwrap()(transfer)
    }
}

type BoxedTask = Box<BoxableTask>;

// TODO: We could actually pass this through the data field of the transfer
enum Switch {
    StartTask {
        stack: ProtectedFixedSizeStack,
        task: BoxedTask,
    },
    Destroy {
        stack: ProtectedFixedSizeStack,
    },
}

thread_local! {
    static SWITCH: RefCell<Option<Switch>> = RefCell::new(None);
}

impl Switch {
    fn put(self) {
        SWITCH.with(|s| {
            let mut s = s.borrow_mut();
            assert!(s.is_none(), "Leftover switch instruction");
            *s = Some(self);
        });
    }
    fn get() -> Self {
        SWITCH.with(|s| s.borrow_mut().take().expect("Missing switch instruction"))
    }
}

extern "C" fn coroutine(mut transfer: Transfer) -> ! {
    let switch = Switch::get();
    let result = match switch {
        Switch::StartTask { stack, mut task } => {
            transfer = task.perform(transfer);
            Switch::Destroy { stack }
        },
        _ => panic!("Invalid switch instruction on coroutine entry"),
    };
    result.put();
    transfer.context.resume(0);
    unreachable!();
}

pub struct Coroutine<R> {
    receiver: Receiver<TaskResult<R>>,
}

impl<R: 'static> Coroutine<R> {
    pub fn spawn<Task: FnOnce(&mut Await) -> R + 'static>(handle: Handle, task: Task) -> Self {
        let (sender, receiver) = oneshot::channel();

        let stack = ProtectedFixedSizeStack::default();
        let context = Context::new(&stack, coroutine);

        let perform_and_send = move |mut transfer| {
            {
                let mut await = Await {
                    transfer: &mut transfer,
                    handle: &handle,
                };
                // TODO: Think about that AssertUnwindSafe.
                let result = match panic::catch_unwind(AssertUnwindSafe(|| task(&mut await))) {
                    Ok(res) => TaskResult::Finished(res),
                    Err(panic) => TaskResult::Panicked(panic),
                };
                // We are not interested in errors. They just mean the receiver is no longer
                // interested, which is fine by us.
                drop(sender.send(result));
            }
            transfer
        };

        Self::run_child(context, Switch::StartTask {
            stack,
            task: Box::new(Some(perform_and_send)),
        });

        Coroutine {
            receiver
        }
    }
    fn run_child(context: Context, switch: Switch) {
        switch.put();
        let transfer = context.resume(0);
        let switch = Switch::get();
        match switch {
            Switch::Destroy { stack } => {
                drop(transfer.context);
                drop(stack);
            },
            _ => unreachable!("Invalid switch instruction when switching out"),
        }
    }
}

impl<R> Future for Coroutine<R> {
    type Item = R;
    type Error = TaskFailed;
    fn poll(&mut self) -> Poll<R, TaskFailed> {
        match self.receiver.poll() {
            Ok(Async::Ready(TaskResult::Panicked(reason))) => Err(TaskFailed::Panicked(reason)),
            Ok(Async::Ready(TaskResult::Finished(result))) => Ok(Async::Ready(result)),
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Err(_) => Err(TaskFailed::Lost),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::rc::Rc;

    use tokio_core::reactor::Core;

    use super::*;

    /// Test spawning and execution of tasks.
    #[test]
    fn spawn_some() {
        let mut core = Core::new().unwrap();
        let s1 = Rc::new(AtomicBool::new(false));
        let s2 = Rc::new(AtomicBool::new(false));
        let s1c = s1.clone();
        let s2c = s2.clone();
        let handle = core.handle();

        let result = Coroutine::spawn(handle, move |await| {
            let result = Coroutine::spawn(await.handle().clone(), move |_| {
                s2c.store(true, Ordering::Relaxed);
                42
            });
            s1c.store(true, Ordering::Relaxed);
            result
        });

        // Both coroutines run to finish
        assert!(s1.load(Ordering::Relaxed), "The outer closure didn't run");
        assert!(s2.load(Ordering::Relaxed), "The inner closure didn't run");
        // The result gets propagated through.
        let extract = result.and_then(|r| r);
        assert_eq!(42, core.run(extract).unwrap());
    }

    /// The panic doesn't kill the main thread, but is reported.
    #[test]
    fn panics() {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        match core.run(Coroutine::spawn(handle, |_| panic!("Test"))) {
            Err(TaskFailed::Panicked(_)) => (),
            _ => panic!("Panic not reported properly"),
        }
        let handle = core.handle();
        assert_eq!(42, core.run(Coroutine::spawn(handle, |_| 42)).unwrap());
    }
}
