extern crate context;
extern crate futures;
extern crate tokio_core;

use std::any::Any;
use std::cell::RefCell;
use std::ops::Deref;
use std::panic::{self, AssertUnwindSafe};

use context::{Context, Transfer};
use context::stack::{ProtectedFixedSizeStack, Stack, StackError};
use futures::{Future, Async, Poll, Sink, Stream};
use futures::future;
use futures::unsync::oneshot::{self, Receiver};
use futures::unsync::mpsc::{self, Receiver as ChannelReceiver, Sender as ChannelSender};
use tokio_core::reactor::Handle;

pub mod results;

mod errors;
mod switch;

pub use errors::TaskFailed;

use errors::TaskResult;
use results::StreamIterator;
use switch::Switch;

/// An asynchronous context.
///
/// This is passed to each coroutine closure and can be used to pause (or block) the coroutine,
/// waiting for a future or something similar to complete.
///
/// The context is explicit, for two reasons. One is, it is possible to ensure nobody tries to
/// wait for a future and block outside of coroutine. The other is, it is more obvious what happens
/// from the code than with some thread-local magic behind the scenes.
///
/// The downside is a little bit less convenience on use.
pub struct Await<'a> {
    transfer: &'a RefCell<Option<Transfer>>,
    handle: &'a Handle,
}

impl<'a> Await<'a> {
    /// Accesses the handle to the corresponding reactor core.
    ///
    /// This is simply a convenience method, since it is possible to get the handle explicitly into
    /// every place where this can be used. But it is convenient not to have to pass another
    /// variable and the `Await` and the handle are usually used together.
    pub fn handle(&self) -> &Handle {
        self.handle
    }
    /// Blocks the current coroutine until the future resolves.
    ///
    /// This blocks or parks the current coroutine (and lets other coroutines run) until the
    /// provided future completes. The result of the coroutine is returned.
    ///
    /// # Notes
    ///
    /// For the switching between coroutines to work, the reactor must be running.
    ///
    /// # Panics
    ///
    /// This may panic if the reactor core is dropped before the waited-for future resolves. The
    /// panic is meant to unwind the coroutine's stack so all the memory can be cleaned up.
    ///
    /// # Examples
    ///
    /// ```
    /// # extern crate corona;
    /// # extern crate futures;
    /// # extern crate tokio_core;
    /// use std::time::Duration;
    /// use corona::Coroutine;
    /// use futures::Future;
    /// use tokio_core::reactor::{Core, Timeout};
    ///
    /// # fn main() {
    /// let mut core = Core::new().unwrap();
    /// let coroutine = Coroutine::with_defaults(core.handle(), |await| {
    ///     let timeout = Timeout::new(Duration::from_millis(100), await.handle()).unwrap();
    ///     await.future(timeout).unwrap();
    /// });
    /// core.run(coroutine).unwrap();
    /// # }
    /// ```
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
        let context = self.transfer
            .borrow_mut()
            .take()
            .unwrap()
            .context;
        let transfer = unsafe { context.resume(0) };
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
    /// Blocks the current coroutine to get each element of the stream.
    ///
    /// This acts in a very similar way as the [`future`](#method.future) method. The difference is
    /// it acts on a stream and produces an iterator instead of a single result. Therefore it may
    /// (and usually will) switch the coroutines more than once.
    ///
    /// Similar notes as with [`future`](#method.future) apply.
    ///
    /// # Examples
    ///
    /// ```
    /// # extern crate corona;
    /// # extern crate futures;
    /// # extern crate tokio_core;
    /// use corona::Coroutine;
    /// use futures::stream;
    /// use tokio_core::reactor::Core;
    ///
    /// # fn main() {
    /// let mut core = Core::new().unwrap();
    /// let coroutine = Coroutine::with_defaults(core.handle(), |await| {
    ///     let stream = stream::empty::<(), ()>();
    ///     for item in await.stream(stream) {
    ///         // Streams can contain errors, so it iterates over `Result`s.
    ///         let item = item.unwrap();
    ///         // Process them here
    ///     }
    /// });
    /// core.run(coroutine).unwrap();
    /// # }
    /// ```
    pub fn stream<I, E, S>(&self, stream: S) -> StreamIterator<I, E, S>
        where
            S: Stream<Item = I, Error = E> + 'static,
            I: 'static,
            E: 'static,
    {
        StreamIterator::new(self, stream)
    }
    /// Switches to another coroutine.
    ///
    /// This allows another coroutine to run. However, if there's no other coroutine ready to run,
    /// this may terminate right away and continue execution. Also, it does *not* guarantee getting
    /// more external events -- the other coroutines can do work on the data that is already
    /// received, for example, but network events will probably arrive only after the coroutine
    /// really waits on something or terminates. Therefore, doing CPU-intensive work in a coroutine
    /// and repeatedly call `yield_now` is not guaranteed to work well. Use a separate thread or
    /// something like the [`futures-cpupool`](https://crates.io/crates/futures-cpupool) crate to
    /// off-load the heavy work.
    pub fn yield_now(&self) {
        let fut = future::ok::<_, ()>(());
        self.future(fut).unwrap();
    }
}

type ItemOrPanic<I> = Result<I, Box<Any + Send + 'static>>;
type ItemSender<I> = ChannelSender<ItemOrPanic<I>>;

pub struct Producer<'a, I: 'static> {
    await: &'a Await<'a>,
    sink: RefCell<Option<ItemSender<I>>>,
}

impl<'a, I: 'static> Deref for Producer<'a, I> {
    type Target = Await<'a>;

    fn deref(&self) -> &Await<'a> {
        self.await
    }
}

impl<'a, I: 'static> Producer<'a, I> {
    pub fn new(await: &'a Await<'a>, sink: ItemSender<I>) -> Self {
        Producer {
            await,
            sink: RefCell::new(Some(sink)),
        }
    }
    pub fn produce(&self, item: I) {
        let sink = self.sink.borrow_mut().take();
        if let Some(sink) = sink {
            let future = sink.send(Ok(item));
            if let Ok(s) = self.await.future(future) {
                *self.sink.borrow_mut() = Some(s);
            }
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
    let context = transfer.context;
    unsafe { context.resume(0) };
    unreachable!("Woken up after termination!");
}

#[derive(Clone)]
pub struct Coroutine {
    handle: Handle,
    stack_size: usize,
}

impl Coroutine {
    pub fn build(handle: Handle) -> Self {
        Coroutine {
            handle,
            stack_size: Stack::default_size(),
        }
    }
    pub fn with_defaults<R, Task>(handle: Handle, task: Task) -> CoroutineResult<R>
        where
            R: 'static,
            Task: FnOnce(&Await) -> R + 'static,
    {
        Coroutine::build(handle).spawn(task).unwrap()
    }
    fn spawn_inner<Task>(&self, task: Task) -> Result<(), StackError>
        where
            Task: FnOnce(Handle, RefCell<Option<Transfer>>) -> RefCell<Option<Transfer>> + 'static
    {
        let stack = ProtectedFixedSizeStack::new(self.stack_size)?;
        let context = unsafe { Context::new(&stack, coroutine) };
        let handle = self.handle.clone();

        let perform = move |transfer| {
            let transfer = RefCell::new(Some(transfer));
            let transfer = task(handle, transfer);
            transfer.into_inner().unwrap()
        };

        Coroutine::run_child(context, Switch::StartTask {
            stack,
            task: Box::new(Some(perform)),
        });

        Ok(())
    }
    // TODO: Do we want to make StackError part of our public API? Maybe not.
    pub fn spawn<R, Task>(&self, task: Task) -> Result<CoroutineResult<R>, StackError>
        where
            R: 'static,
            Task: FnOnce(&Await) -> R + 'static,
    {
        let (sender, receiver) = oneshot::channel();

        let perform_and_send = move |handle, transfer| {
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
            transfer
        };

        self.spawn_inner(perform_and_send)?;

        Ok(CoroutineResult {
            receiver
        })
    }
    pub fn generator<Item, Task>(&self, task: Task) -> Result<GeneratorResult<Item>, StackError>
        where
            Item: 'static,
            Task: FnOnce(&Producer<Item>) + 'static,
    {
        let (sender, receiver) = mpsc::channel(1);

        let generate = move |handle, transfer| {
            {
                let await = Await {
                    transfer: &transfer,
                    handle: &handle,
                };
                let producer = Producer::new(&await, sender.clone());

                match panic::catch_unwind(AssertUnwindSafe(move || task(&producer))) {
                    Ok(_) => (),
                    Err(panic) => drop(await.future(sender.send(Err(panic)))),
                }
            }
            transfer
        };

        self.spawn_inner(generate)?;

        Ok(GeneratorResult {
            receiver,
        })
    }
    pub fn stack_size(&mut self, size: usize) -> &mut Self {
        self.stack_size = size;
        self
    }
    fn run_child(context: Context, switch: Switch) {
        switch.put();
        let transfer = unsafe { context.resume(0) };
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

pub struct CoroutineResult<R> {
    receiver: Receiver<TaskResult<R>>,
}

impl<R> Future for CoroutineResult<R> {
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

pub struct GeneratorResult<Item> {
    receiver: ChannelReceiver<Result<Item, Box<Any + Send + 'static>>>,
}

impl<Item> Stream for GeneratorResult<Item> {
    type Item = Item;
    type Error = Box<Any + Send + 'static>;
    fn poll(&mut self) -> Poll<Option<Item>, Self::Error> {
        match self.receiver.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Ok(Async::Ready(Some(Ok(item)))) => Ok(Async::Ready(Some(item))),
            Ok(Async::Ready(Some(Err(e)))) => Err(e),
            Err(_) => unreachable!("Error from mpsc channel ‒ Can Not Happen"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::rc::Rc;
    use std::time::Duration;

    use futures::unsync::mpsc;
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

        let mut builder = Coroutine::build(handle);
        builder.stack_size(40960);
        let builder_inner = builder.clone();

        let result = builder.spawn(move |_| {
            let result = builder_inner.spawn(move |_| {
                s2c.store(true, Ordering::Relaxed);
                42
            }).unwrap();
            s1c.store(true, Ordering::Relaxed);
            result
        }).unwrap();

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
        match core.run(Coroutine::with_defaults(handle, |_| panic!("Test"))) {
            Err(TaskFailed::Panicked(_)) => (),
            _ => panic!("Panic not reported properly"),
        }
        let handle = core.handle();
        assert_eq!(42, core.run(Coroutine::with_defaults(handle, |_| 42)).unwrap());
    }

    /// Wait for a future to complete.
    #[test]
    fn future_wait() {
        let mut core = Core::new().unwrap();
        let (sender, receiver) = oneshot::channel();
        let all_done = Coroutine::with_defaults(core.handle(), move |await| {
            let msg = await.future(receiver).unwrap();
            msg
        });
        Coroutine::with_defaults(core.handle(), move |await| {
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
        let done = Coroutine::with_defaults(core.handle(), move |await| {
            let mut sum = 0;
            for i in await.stream(stream) {
                sum += i.unwrap();
            }
            sum
        });
        assert_eq!(3, core.run(done).unwrap());
    }

    /// A smoke test for yield_now() (that it gets resumed and doesn't crash)
    #[test]
    fn yield_now() {
        let mut core = Core::new().unwrap();
        let done = Coroutine::with_defaults(core.handle(), |await| {
            await.yield_now();
            await.yield_now();
        });
        core.run(done).unwrap();
    }

    #[test]
    fn producer() {
        let mut core = Core::new().unwrap();
        let (sender, receiver) = mpsc::channel(1);
        let done_sender = Coroutine::with_defaults(core.handle(), move |await| {
            let producer = Producer::new(await, sender);
            producer.produce(42);
            producer.produce(12);
        });
        let done_receiver = Coroutine::with_defaults(core.handle(), |await| {
            let result = await.stream(receiver).map(Result::unwrap).collect::<Result<Vec<_>, _>>().unwrap();
            assert_eq!(vec![42, 12], result);
        });
        let done = Coroutine::with_defaults(core.handle(), move |await| {
            await.future(done_sender).unwrap();
            await.future(done_receiver).unwrap();
        });
        core.run(done).unwrap();
    }

    #[test]
    fn generator() {
        let mut core = Core::new().unwrap();
        let builder = Coroutine::build(core.handle());
        let stream1 = builder.generator(|await| {
            await.produce(42);
            await.produce(12);
        }).unwrap();
        let stream2 = builder.generator(|await| {
            for item in await.stream(stream1) {
                await.produce(item.unwrap());
            }
        }).unwrap();
        let done = builder.spawn(move |await| {
            let mut result = Vec::new();
            for item in await.stream(stream2) {
                result.push(item.unwrap());
            }
            assert_eq!(vec![42, 12], result);
        }).unwrap();
        core.run(done).unwrap();
    }
}
