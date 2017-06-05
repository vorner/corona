extern crate context;
extern crate futures;
extern crate tokio_core;

use std::any::Any;
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::{Future, Async, Poll, Stream};
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
    ScheduleWakeup {
        after: Box<Future<Item = (), Error = ()>>,
        handle: Handle,
    },
    Resume,
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

pub struct StreamIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    await: &'a Await<'a>,
    stream: Option<S>,
}

impl<'a, I, E, S> Iterator for StreamIterator<'a, I, E, S>
    where
        S: Stream<Item = I, Error = E> + 'static,
        I: 'static,
        E: 'static,
{
    type Item = Result<I, E>;
    fn next(&mut self) -> Option<Self::Item> {
        let fut = self.stream.take().unwrap().into_future();
        let resolved = self.await.future(fut);
        let (result, stream) = match resolved {
            Ok((None, stream)) => (None, stream),
            Ok((Some(ok), stream)) => (Some(Ok(ok)), stream),
            Err((e, stream)) => (Some(Err(e)), stream),
        };
        self.stream = Some(stream);
        result
    }
}

pub struct Await<'a> {
    transfer: &'a RefCell<Option<Transfer>>,
    handle: &'a Handle,
}

impl<'a> Await<'a> {
    pub fn handle(&self) -> &Handle {
        self.handle
    }
    pub fn future<I, E, Fut>(&self, fut: Fut) -> Result<I, E>
        where
            I: 'static,
            E: 'static,
            Fut: Future<Item = I, Error = E> + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let task = fut.then(move |r| {
            // Errors are uninteresting - just the listener missing
            // TODO: Is it even possible?
            drop(sender.send(r));
            Ok(())
        });
        let switch = Switch::ScheduleWakeup {
            after: Box::new(task),
            handle: self.handle.clone(),
        };
        switch.put();
        let transfer = self.transfer
            .borrow_mut()
            .take()
            .unwrap()
            .context
            .resume(0);
        *self.transfer.borrow_mut() = Some(transfer);
        match Switch::get() {
            Switch::Resume => (),
            _ => panic!("Invalid instruction on wakeup"),
        }
        // It is safe to .wait(), because once we are resumed, the future already went through.
        // It shouldn't happen that we got canceled under normal circumstances (may need API
        // changes to actually ensure that).
        receiver.wait().expect("A future got canceled")
    }
    pub fn stream<I, E, S>(&self, stream: S) -> StreamIterator<I, E, S>
        where
            S: Stream<Item = I, Error = E> + 'static,
            I: 'static,
            E: 'static,
    {
        StreamIterator {
            await: self,
            stream: Some(stream),
        }
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
    unreachable!("Woken up after termination!");
}

pub struct Coroutine<R> {
    receiver: Receiver<TaskResult<R>>,
}

impl<R: 'static> Coroutine<R> {
    pub fn spawn<Task>(handle: Handle, task: Task) -> Self
        where
            Task: FnOnce(&Await) -> R + 'static,
    {
        let (sender, receiver) = oneshot::channel();

        let stack = ProtectedFixedSizeStack::default();
        let context = Context::new(&stack, coroutine);

        let perform_and_send = move |transfer| {
            let transfer = RefCell::new(Some(transfer));
            {
                let await = Await {
                    transfer: &transfer,
                    handle: &handle,
                };
                let result = match panic::catch_unwind(AssertUnwindSafe(move || task(&await))) {
                    Ok(res) => TaskResult::Finished(res),
                    Err(panic) => TaskResult::Panicked(panic),
                };
                // We are not interested in errors. They just mean the receiver is no longer
                // interested, which is fine by us.
                drop(sender.send(result));
            }
            transfer.into_inner().unwrap()
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
            Switch::ScheduleWakeup { after, handle } => {
                // TODO: We may want some kind of our own future here and implement Drop, so we can
                // unwind the stack and destroy it.
                let wakeup = after.then(move |_| {
                    Self::run_child(transfer.context, Switch::Resume);
                    Ok(())
                });
                handle.spawn(wakeup);
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
    use std::time::Duration;

    use tokio_core::reactor::{Core, Interval, Timeout};

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

    /// Wait for a future to complete.
    #[test]
    fn future_wait() {
        let mut core = Core::new().unwrap();
        let (sender, receiver) = oneshot::channel();
        let all_done = Coroutine::spawn(core.handle(), move |await| {
            let msg = await.future(receiver).unwrap();
            msg
        });
        Coroutine::spawn(core.handle(), move |await| {
            let timeout = Timeout::new(Duration::from_millis(50), await.handle()).unwrap();
            await.future(timeout).unwrap();
            sender.send(42).unwrap();
        });
        assert_eq!(42, core.run(all_done).unwrap());
    }

    /// Stream can be iterated asynchronously.
    #[test]
    fn stream_iter() {
        let mut core = Core::new().unwrap();
        let stream = Interval::new(Duration::from_millis(10), &core.handle())
            .unwrap()
            .take(3)
            .map(|_| 1);
        let done = Coroutine::spawn(core.handle(), move |await| {
            let mut sum = 0;
            for i in await.stream(stream) {
                sum += i.unwrap();
            }
            sum
        });
        assert_eq!(3, core.run(done).unwrap());
    }
}
