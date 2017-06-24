//! A library combining futures and coroutines.
//!
//! The current aim of Rust in regards to asynchronous programming is on
//! [`futures`](https://crates.io/crates/futures). They have many good properties. However, they
//! tend to result in very functional-looking code (eg. monadic chaining of closures through their
//! modifiers). Some tasks are more conveniently done throuch imperative approaches.
//!
//! The aim of this library is to integrate coroutines with futures. It is possible to start a
//! coroutine. The coroutine can wait for a future to complete (which'll suspend its execution, but
//! will not block the thread ‒ the execution will switch to other futures or coroutines on the
//! same thread). A spawned coroutine is represented through a handle that acts as a future,
//! representing its completion. Therefore, other coroutines can wait for it, or it can be used in
//! the usual functional way and compose it with other futures.
//!
//! # The cost
//!
//! The coroutines are *not* zero cost, at least not now. There are these costs:
//!
//! * Each coroutine needs a stack. The stack takes some space and is allocated dynamically.
//!   Furthermore, a stack can't be allocated through the usual allocator, but is mapped directly
//!   by the OS and manipulating the memory mapping of pages is relatively expensive operation (a
//!   syscall, TLB needs to be flushed, ...). This currently happens for each spawned coroutine.
//! * Each wait for a future inside a coroutine allocates dynamically (because it spawns a task
//!   inside [Tokio's](https://crates.io/crates/tokio-core) reactor core.
//!
//! Some of these costs might be mitigated or lowered in future, but for now, expect to get
//! somewhat lower performance with coroutines compared to using only futures.
//!
//! # API Stability
//!
//! Currently, the crate is in an experimental state. It exists mostly as a research if it is
//! possible to provide something like async/await in Rust and integrate it with the current
//! asynchronous stack, without adding explicit support to the language.
//!
//! It is obviously possible, but the ergonomics is something that needs some work. Therefore,
//! expect the API to change, possibly in large ways.
//!
//! # Known problems
//!
//! These are the problems I'm aware of and which I want to find a solution some day.
//!
//! * The current API is probably inconvinient.
//! * Many abstractions are missing. Things like waiting for a future with a timeout, or waiting
//!   for the first of many futures or streams would come handy.
//! * Many places have `'static` bounds on the types, even though these shouldn't be needed in
//!   theory.
//! * The relation with unwind safety is unclear.
//! * No support for threads.
//! * It relies on the tokio. It would be great if it worked with other future executors as well.
//! * When the reactor core is dropped with some coroutines yet unfinished, their stacks and
//!   everything on them leak.
//!
//! # Contribution
//!
//! All kinds of contributions are welcome, including reporting bugs, improving the documentation,
//! submitting code, etc. However, the most valuable contribution for now would be trying it out
//! and providing some feedback ‒ if the thing works, where the API needs improvements, etc.
//!
//! # Examples
//!
//! One that shows the API.
//!
//! ```
//! # extern crate corona;
//! # extern crate futures;
//! # extern crate tokio_core;
//! use std::time::Duration;
//! use corona::Coroutine;
//! use futures::Future;
//! use tokio_core::reactor::{Core, Timeout};
//!
//! # fn main() {
//! let mut core = Core::new().unwrap();
//! let builder = Coroutine::new(core.handle());
//! let generator = builder.generator(|producer| {
//!     producer.produce(1);
//!     producer.produce(2);
//! });
//! let coroutine = builder.spawn(move |await| {
//!     for item in await.stream(generator) {
//!         println!("{}", item.unwrap());
//!     }
//!
//!     let timeout = Timeout::new(Duration::from_millis(100), await.handle()).unwrap();
//!     await.future(timeout).unwrap();
//!
//!     42
//! });
//! assert_eq!(42, core.run(coroutine).unwrap());
//! # }
//! ```
//!
//! Further examples can be found in the
//! [repository](https://github.com/vorner/corona/tree/master/examples).

extern crate context;
extern crate futures;
extern crate tokio_core;

use std::any::Any;
use std::cell::RefCell;
use std::ops::Deref;
use std::panic::{self, AssertUnwindSafe};

use context::{Context, Transfer};
use context::stack::{ProtectedFixedSizeStack, Stack};
use futures::{Future, Sink, Stream};
use futures::future;
use futures::unsync::oneshot;
use futures::unsync::mpsc::{self, Sender as ChannelSender};
use tokio_core::reactor::Handle;

pub mod results;

mod errors;
mod switch;

pub use errors::TaskFailed;

use errors::TaskResult;
use results::{CoroutineResult, GeneratorResult, StreamIterator};
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
    /// Similar notes as with the [`future`](#method.future) apply.
    ///
    /// # Panics
    ///
    /// The same ones as with the [`future`](#method.future) method.
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

/// This is an extended [`Await`](struct.Await.html) with ability to items.
///
/// Just like an ordinary coroutine returns produces a single return value when it finishes and can
/// suspend its execution using the [`Await`](struct.Await.html) parameter, a generator can do all
/// this and, in addition, produce a serie of items of a given type through this parameter.
///
/// See [`Coroutine::generator`](struct.Coroutine.html#method.generator).
pub struct Producer<'a, I: 'static> {
    await: &'a Await<'a>,
    sink: RefCell<Option<ItemSender<I>>>,
}

// TODO: Is this an abuse of Deref? Any better ways?
impl<'a, I: 'static> Deref for Producer<'a, I> {
    type Target = Await<'a>;

    fn deref(&self) -> &Await<'a> {
        self.await
    }
}

impl<'a, I: 'static> Producer<'a, I> {
    /// Creates a new producer.
    ///
    /// While the usual way to get a producer is through the
    /// [`Coroutine::generator`](struct.Coroutine.html#method.generator), it is also possible to
    /// create one manually, from an [`Await`](struct.Await.html) and a channel sender of the right
    /// type.
    pub fn new(await: &'a Await<'a>, sink: ItemSender<I>) -> Self {
        Producer {
            await,
            sink: RefCell::new(Some(sink)),
        }
    }
    /// Pushes another value through the internal channel, effectively sending it to another
    /// coroutine.
    ///
    /// This takes a value and pushes it through a channel to another coroutine. It may suspend the
    /// execution of the current coroutine and yield to another one.
    ///
    /// The same notes and panics as with the [`future`](struct.Await.html#method.future) method
    /// apply.
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

/// A builder of coroutines.
///
/// This struct is the main entry point and a way to start coroutines of various kinds. It allows
/// both starting them with default parameters and configuring them with the builder pattern.
#[derive(Clone)]
pub struct Coroutine {
    handle: Handle,
    stack_size: usize,
}

impl Coroutine {
    /// Starts building a coroutine.
    ///
    /// This constructor produces a new builder for coroutines. The builder can then be used to
    /// specify configuration of the coroutines.
    ///
    /// It is possible to spawn multiple coroutines from the same builder.
    ///
    /// # Parameters
    ///
    /// * `handle`: The coroutines need a reactor core to run on and schedule their control
    ///   switches. This is the handle to the reactor core to be used.
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
    /// let core = Core::new().unwrap();
    /// let builder = Coroutine::new(core.handle());
    ///
    /// let coroutine = builder.spawn(|await| { });
    /// # }
    ///
    /// ```
    pub fn new(handle: Handle) -> Self {
        Coroutine {
            handle,
            stack_size: Stack::default_size(),
        }
    }
    /// Spawns a coroutine directly.
    ///
    /// This constructor spawns a coroutine with default parameters without the inconvenience of
    /// handling a builder. It is equivalent to spawning it with an unconfigured builder.
    ///
    /// Unlike the [`spawn`](#method.spawn.html), this one can't fail, since the default parameters
    /// of the builder are expected to always work (if they don't, file a bug).
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
    /// let core = Core::new().unwrap();
    ///
    /// let coroutine = Coroutine::with_defaults(core.handle(), |await| { });
    /// # }
    ///
    /// ```
    pub fn with_defaults<R, Task>(handle: Handle, task: Task) -> CoroutineResult<R>
        where
            R: 'static,
            Task: FnOnce(&Await) -> R + 'static,
    {
        Coroutine::new(handle).spawn(task)
    }
    fn spawn_inner<Task>(&self, task: Task)
        where
            Task: FnOnce(Handle, RefCell<Option<Transfer>>) -> RefCell<Option<Transfer>> + 'static
    {
        let stack = ProtectedFixedSizeStack::new(self.stack_size).expect("Invalid stack size");
        let context = unsafe { Context::new(&stack, coroutine) };
        let handle = self.handle.clone();

        let perform = move |transfer| {
            let transfer = RefCell::new(Some(transfer));
            let transfer = task(handle, transfer);
            transfer.into_inner().unwrap()
        };

        let switch = Switch::StartTask {
            stack,
            task: Box::new(Some(perform)),
        };
        switch.run_child(context);
    }
    /// Spawns a coroutine.
    ///
    /// Spawns the given closure as a coroutine with the parameters configured in the current
    /// builder.
    ///
    /// The closure is started right away and is run inside the call until it either yields the
    /// control or terminates. If it yields (for whatever reason, not only through the
    /// [`Await::yield_now`](struct.Await.html#method.yield_now) method), it'll get a chance to
    /// continue only through running the reactor core.
    ///
    /// # Parameters
    ///
    /// * `task`: The closure to run.
    ///
    /// # Panics
    ///
    /// * In case an invalid stack size has been configured. This is a panic and not an error for
    ///   two reasons. It's very unlikely an application using coroutines could continue if it
    ///   can't spawn them. Also, configuring invalid stack size is a programmer bug.
    ///
    /// # Result
    ///
    /// On successful call to this method, a `Future` representing the completion of the task is
    /// provided. The future resolves either to the result of the closure or an error if the
    /// closure panics.
    pub fn spawn<R, Task>(&self, task: Task) -> CoroutineResult<R>
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

        self.spawn_inner(perform_and_send);

        CoroutineResult::new(receiver)
    }
    /// Spawns a generator.
    ///
    /// A generator is just like a coroutine (and this method is very similar to the
    /// [`spawn`](#method.spawn) method, so most of its notes apply). It can, however, produce a
    /// stream of items of a certain kind and has no direct return value. The return value is not a
    /// `Future`, but a `Stream` of the produced items.
    pub fn generator<Item, Task>(&self, task: Task) -> GeneratorResult<Item>
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

        self.spawn_inner(generate);

        GeneratorResult::new(receiver)
    }
    /// Configures a stack size for the coroutines.
    ///
    /// The method sets the stack size of the coroutines that'll be spawned from this builder. The
    /// default stack size is platform dependent, but usually something relatively small. It is
    /// fine for most uses that don't use recursion or big on-stack allocations.
    ///
    /// Also, using too many different stack sizes in the same thread is inefficient. The library
    /// caches and reuses stacks, but it can do so only with stacks of the same size.
    ///
    /// # Notes
    ///
    /// If the configured stack size is invalid, attempts to spawn coroutines will fail with a
    /// panic. However, it is platform dependent what is considered valid (multiples of 4096
    /// usually work).
    pub fn stack_size(&mut self, size: usize) -> &mut Self {
        self.stack_size = size;
        self
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

        let mut builder = Coroutine::new(handle);
        builder.stack_size(40960);
        let builder_inner = builder.clone();

        let result = builder.spawn(move |_| {
            let result = builder_inner.spawn(move |_| {
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
        let builder = Coroutine::new(core.handle());
        let stream1 = builder.generator(|await| {
            await.produce(42);
            await.produce(12);
        });
        let stream2 = builder.generator(|await| {
            for item in await.stream(stream1) {
                await.produce(item.unwrap());
            }
        });
        let done = builder.spawn(move |await| {
            let mut result = Vec::new();
            for item in await.stream(stream2) {
                result.push(item.unwrap());
            }
            assert_eq!(vec![42, 12], result);
        });
        core.run(done).unwrap();
    }

    /*
     TODO: This thing deadlocks. Any chance of preventing it from compilation?
    #[test]
    fn blocks() {
        let mut core = Core::new().unwrap();
        let (sender, receiver) = oneshot::channel();
        let handle = core.handle();
        let c = Coroutine::with_defaults(handle.clone(), move |_await| {
            core.run(receiver).unwrap();
        });
        Coroutine::with_defaults(handle, |await| {
            let timeout = Timeout::new(Duration::from_millis(50), await.handle()).unwrap();
            await.future(timeout).unwrap();
            drop(sender.send(42));
        });
        c.wait().unwrap();
    }
    */
}
