#![doc(html_root_url = "https://docs.rs/corona/0.2.1/corona/")]

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
//! Unlike the coroutines planned for core Rust, these coroutines are stack-full (eg. you can
//! suspend them from within deep stack frame) and they are available now.
//!
//! # The cost
//!
//! The coroutines are *not* zero cost, at least not now. There are these costs:
//!
//! * Each coroutine needs a stack. The stack takes some space and is allocated dynamically.
//!   Furthermore, a stack can't be allocated through the usual allocator, but is mapped directly
//!   by the OS and manipulating the memory mapping of pages is relatively expensive operation (a
//!   syscall, TLB needs to be flushed, ...). The stacks are cached and reused, but still, creating
//!   them has a cost.
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
//! Still, if you want to test it, it should probably work and might be useful. It is experimental
//! in a sense the API is not stabilized, but it is not expected to eat data or crash.
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
//! * No support for threads (probably not even possible ‒ Rust's type system doesn't expect a
//!   stack to move from one thread to another).
//! * It relies on the tokio. It would be great if it worked with other future executors as well.
//! * Cleaning up of stacks (and things on them) when the coroutines didn't finish yet is done
//!   through panicking. This has some ugly side effects.
//! * It is possible to create a deadlock when moving the driving tokio core inside a coroutine,
//!   like this (eg. this is an example what *not* to do):
//!
//! ```rust,no_run
//! # extern crate corona;
//! # extern crate futures;
//! # extern crate tokio_core;
//! # use std::time::Duration;
//! # use corona::Coroutine;
//! # use futures::Future;
//! # use futures::unsync::oneshot;
//! # use tokio_core::reactor::{Core, Timeout};
//! #
//! # fn main() {
//! let mut core = Core::new().unwrap();
//! let (sender, receiver) = oneshot::channel();
//! let handle = core.handle();
//! let c = Coroutine::with_defaults(handle.clone(), move |_await| {
//!     core.run(receiver).unwrap();
//! });
//! Coroutine::with_defaults(handle, |await| {
//!     let timeout = Timeout::new(Duration::from_millis(50), await.handle()).unwrap();
//!     await.future(timeout).unwrap();
//!     drop(sender.send(42));
//! });
//! c.wait().unwrap();
//! # }
//! ```
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

// XXX Finish cleanups
// XXX Add docs
// XXX Switch submodules to pub(crate) where possible/appropriate
// XXX Move the Coroutine to a submodule and provide this as a facade

extern crate context;
extern crate futures;
extern crate tokio_core;

pub mod errors;
pub mod prelude;
pub mod wrappers;

mod stack_cache;
mod switch;

use std::any::Any;
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};

use context::Context;
use context::stack::{Stack, ProtectedFixedSizeStack};
use futures::{Async, Future, Poll};
use futures::unsync::oneshot::{self, Receiver};
use tokio_core::reactor::Handle;

pub use errors::{Dropped, TaskFailed};

use switch::Switch;

pub enum TaskResult<R> {
    Panicked(Box<Any + Send + 'static>),
    Finished(R),
}

/// A `Future` representing a completion of a coroutine.
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

struct CoroutineContext {
    /// Use this to spawn waiting coroutines
    handle: Handle,
    /// The context that called us and we'll switch back to it when we wait for something.
    parent_context: Context,
    /// Our own stack. We keep ourselvel alive.
    stack: ProtectedFixedSizeStack,
}

thread_local! {
    static CONTEXTS: RefCell<Vec<CoroutineContext>> = RefCell::new(Vec::new());
}

/// A builder of coroutines.
///
/// This struct is the main entry point and a way to start coroutines of various kinds. It allows
/// both starting them with default parameters and configuring them with the builder pattern.
#[derive(Clone)]
pub struct Coroutine {
    handle: Handle,
    stack_size: usize,
    leak_on_panic: bool,
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
    /// # extern crate tokio_core;
    /// use corona::Coroutine;
    /// use tokio_core::reactor::Core;
    ///
    /// # fn main() {
    /// let core = Core::new().unwrap();
    /// let builder = Coroutine::new(core.handle());
    ///
    /// let coroutine = builder.spawn(|| { });
    /// # }
    ///
    /// ```
    pub fn new(handle: Handle) -> Self {
        Coroutine {
            handle,
            stack_size: Stack::default_size(),
            leak_on_panic: false,
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
    /// # extern crate tokio_core;
    /// use corona::Coroutine;
    /// use tokio_core::reactor::Core;
    ///
    /// # fn main() {
    /// let core = Core::new().unwrap();
    ///
    /// let coroutine = Coroutine::with_defaults(core.handle(), || { });
    /// # }
    ///
    /// ```
    pub fn with_defaults<R, Task>(handle: Handle, task: Task) -> CoroutineResult<R>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        Coroutine::new(handle).spawn(task)
    }
    pub fn spawn<R, Task>(&self, task: Task) -> CoroutineResult<R>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        let (sender, receiver) = oneshot::channel();

        let handle = self.handle.clone();

        let perform = move |context, stack| {
            let my_context = CoroutineContext {
                handle,
                parent_context: context,
                stack,
            };
            CONTEXTS.with(|c| c.borrow_mut().push(my_context));
            let result = match panic::catch_unwind(AssertUnwindSafe(|| task())) {
                Ok(res) => TaskResult::Finished(res),
                Err(panic) => TaskResult::Panicked(panic),
            };
            // We are not interested in errors. They just mean the receiver is no longer
            // interested, which is fine by us.
            drop(sender.send(result));
            let my_context = CONTEXTS.with(|c| c.borrow_mut().pop().unwrap());
            (my_context.parent_context, my_context.stack)
        };
        Switch::run_new_coroutine(self.stack_size, Box::new(Some(perform)));

        CoroutineResult { receiver }
    }

    pub fn wait<I, E, Fut>(fut: Fut) -> Result<Result<I, E>, Dropped>
    where
        I: 'static,
        E: 'static,
        Fut: Future<Item = I, Error = E> + 'static,
    {
        let (sender, receiver) = oneshot::channel();
        let task = fut.then(move |r| {
            // Errors are uninteresting. That just means nobody is listening for the answer.
            drop(sender.send(r));
            Ok(())
        });
        let my_context = CONTEXTS.with(|c| {
            c.borrow_mut().pop().expect("Can't wait outside of a coroutine")
        });
        let instruction = Switch::ScheduleWakeup {
            after: Box::new(task),
            handle: my_context.handle.clone(),
        };
        let (reply_instruction, context) = instruction.exchange(my_context.parent_context);
        let new_context = CoroutineContext {
            parent_context: context,
            stack: my_context.stack,
            handle: my_context.handle,
        };
        CONTEXTS.with(|c| c.borrow_mut().push(new_context));
        match reply_instruction {
            Switch::Resume => (),
            Switch::Cleanup => return Err(Dropped),
            _ => unreachable!("Invalid instruction on wakeup"),
        }
        // It is safe to .wait() here, because we are resumed and we get resumed only after the
        // future finished.
        // TODO: Dropping futures?
        Ok(receiver.wait().expect("A future should never get dropped"))
    }
}

/*

use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::thread;

use context::Context;
use futures::{Future, Sink, Stream};
use futures::future;
use futures::unsync::mpsc::{self, Sender as ChannelSender};
use tokio_core::reactor::Handle;

pub mod results;

mod errors;

pub use errors::{Dropped, TaskFailed};

use errors::TaskResult;
use results::{CoroutineResult, GeneratorResult, StreamCleanupIterator, StreamIterator};

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
pub struct Await {
    context: RefCell<Option<Context>>,
    dropped: Cell<bool>,
    leak_on_panic: bool,
    stack: RefCell<Option<ProtectedFixedSizeStack>>,
    handle: Handle,
}

impl Await {
    /// Accesses the handle to the corresponding reactor core.
    ///
    /// This is simply a convenience method, since it is possible to get the handle explicitly into
    /// every place where this can be used. But it is convenient not to have to pass another
    /// variable and the `Await` and the handle are usually used together.
    pub fn handle(&self) -> &Handle {
        &self.handle
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
        match self.future_cleanup(fut) {
            Ok(result) => result,
            Err(Dropped) => {
                if self.leak_on_panic && thread::panicking() {
                    let stack = self.stack
                        .borrow_mut()
                        .take()
                        .unwrap();
                    self.switch(Switch::Destroy { stack });
                    unreachable!();
                } else {
                    panic!("Cleaning up the coroutine stack because the reactor Core got dropped");
                }
            }
        }
    }
    // Switch out of the current coroutine and back
    fn switch(&self, switch: Switch) -> Switch {
        let context = self.context
            .borrow_mut()
            .take()
            .unwrap();
        let (reply, context) = switch.exchange(context);
        *self.context.borrow_mut() = Some(context);
        reply
    }
    /// Blocks the current coroutine, just like [`future`](#method.future), but doesn't panic.
    ///
    /// This works similar to the `future` method. However, it signals if the reactor `Core` has
    /// been destroyed by returning `Err(Dropped)` instead of panicking. This can be used to
    /// manually clean up the coroutine instead of letting a panic do that.
    ///
    /// This is important especially in cases when clean shutdown is needed even when the `Core` in
    /// the main coroutine is destroyed during a panic, since the `future` method either causes a
    /// double panic (making the program abort) or doesn't do any cleanup at all, depending on the
    /// configuration.
    pub fn future_cleanup<I, E, Fut>(&self, fut: Fut) -> Result<Result<I, E>, Dropped>
        where
            I: 'static,
            E: 'static,
            Fut: Future<Item = I, Error = E> + 'static,
    {
        if self.dropped.get() {
            return Err(Dropped);
        }
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
        match self.switch(switch) {
            Switch::Resume => (),
            Switch::Cleanup => {
                self.dropped.set(true);
                return Err(Dropped);
            },
            _ => panic!("Invalid instruction on wakeup"),
        }
        // It is safe to .wait(), because once we are resumed, the future already went through.
        // It shouldn't happen that we got canceled under normal circumstances (may need API
        // changes to actually ensure that).
        Ok(receiver.wait().expect("A future should never get dropped"))
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
    /// Blocks the current coroutine to get each element of the stream.
    ///
    /// This is the same as the [`stream`](#method.stream) method, but it doesn't panic when the
    /// core is dropped and the coroutine's stack needs to be cleaned up. Instead it returns
    /// `Err(Dropped)` and leaves the cleanup to the caller.
    ///
    /// The advantage is this works even in case the reactor core is dropped during a panic. See
    /// [`Coroutine::leak_on_panic`](struct.Coroutine.html#method.leak_on_panic) for more details.
    pub fn stream_cleanup<I, E, S>(&self, stream: S) -> StreamCleanupIterator<I, E, S>
        where
            S: Stream<Item = I, Error = E> + 'static,
            I: 'static,
            E: 'static,
    {
        StreamCleanupIterator::new(self, stream)
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
    /// Switches to another coroutine.
    ///
    /// This is the same as [`yield_now`](#method.yield_now), but instead of panicking when the
    /// reactor core is dropped, it returns `Err(Dropped)`.
    pub fn yield_now_cleanup(&self) -> Result<(), Dropped> {
        let fut = future::ok::<_, ()>(());
        self.future_cleanup(fut).map(|_| ())
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
pub struct Producer<I: 'static> {
    await: Await,
    sink: RefCell<Option<ItemSender<I>>>,
}

// TODO: Is this an abuse of Deref? Any better ways?
impl<I: 'static> Deref for Producer<I> {
    type Target = Await;

    fn deref(&self) -> &Await {
        &self.await
    }
}

impl<'a, I: 'static> Producer<I> {
    /// Creates a new producer.
    ///
    /// While the usual way to get a producer is through the
    /// [`Coroutine::generator`](struct.Coroutine.html#method.generator), it is also possible to
    /// create one manually, from an [`Await`](struct.Await.html) and a channel sender of the right
    /// type.
    pub fn new(await: Await, sink: ItemSender<I>) -> Self {
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

/// A builder of coroutines.
///
/// This struct is the main entry point and a way to start coroutines of various kinds. It allows
/// both starting them with default parameters and configuring them with the builder pattern.
#[derive(Clone)]
pub struct Coroutine {
    handle: Handle,
    stack_size: usize,
    leak_on_panic: bool,
}

impl Coroutine {
    fn spawn_inner<Task>(&self, task: Task)
        where
            Task: FnOnce(Handle, RefCell<Option<Context>>, RefCell<Option<ProtectedFixedSizeStack>>)
                -> (RefCell<Option<Context>>, RefCell<Option<ProtectedFixedSizeStack>>) + 'static
    {
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
        let leak = self.leak_on_panic;

        let perform_and_send = move |handle, context, stack| {
            let await = Await {
                context: context,
                dropped: Cell::new(false),
                leak_on_panic: leak,
                stack: stack,
                handle: handle,
            };
            let result = match panic::catch_unwind(AssertUnwindSafe(|| task(&await))) {
                Ok(res) => TaskResult::Finished(res),
                Err(panic) => TaskResult::Panicked(panic),
            };
            // We are not interested in errors. They just mean the receiver is no longer
            // interested, which is fine by us.
            drop(sender.send(result));
            (await.context, await.stack)
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
        let leak = self.leak_on_panic;

        let generate = move |handle, context, stack| {
            let await = Await {
                context: context,
                dropped: Cell::new(false),
                leak_on_panic: leak,
                stack: stack,
                handle: handle,
            };
            let producer = Producer::new(await, sender.clone());
            match panic::catch_unwind(AssertUnwindSafe(|| task(&producer))) {
                Ok(_) => (),
                Err(panic) => drop(producer.await.future(sender.send(Err(panic)))),
            }
            (producer.await.context, producer.await.stack)
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
    /// Configures the leak on panic option.
    ///
    /// If the reactor `Core` is dropped, any outstanding coroutines are cleaned up by panicking
    /// from the function they block on (if it is not one of the `_cleanup` variants). However, if
    /// the `Core` is dropped during panick, panicking inside the coroutine would abort the
    /// program.
    ///
    /// This option allows skipping the cleanups. Instead of aborting the program, the resources on
    /// the coroutines' stacks are leaked.
    ///
    /// The `_cleanup` routines still return `Err(Dropped)` and allow for manual cleanup.
    pub fn leak_on_panic(&mut self, leak: bool) -> &mut Self {
        self.leak_on_panic = leak;
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

    /*
     * XXX Revive
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
    */

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
*/

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::rc::Rc;
    use std::time::Duration;

    use futures::future;
    use tokio_core::reactor::{Core, Timeout};

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
        // builder.stack_size(40960); XXX
        let builder_inner = builder.clone();

        let result = builder.spawn(move || {
            let result = builder_inner.spawn(move || {
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

    /// Wait for a future to complete.
    #[test]
    fn future_wait() {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        let (sender, receiver) = oneshot::channel();
        let all_done = Coroutine::with_defaults(core.handle(), move || {
            let msg = Coroutine::wait(receiver).unwrap().unwrap();
            msg
        });
        Coroutine::with_defaults(core.handle(), move || {
            let timeout = Timeout::new(Duration::from_millis(50), &handle).unwrap();
            Coroutine::wait(timeout).unwrap().unwrap();
            sender.send(42).unwrap();
        });
        assert_eq!(42, core.run(all_done).unwrap());
    }

    /// The panic doesn't kill the main thread, but is reported.
    #[test]
    fn panics() {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        match core.run(Coroutine::with_defaults(handle, || panic!("Test"))) {
            Err(TaskFailed::Panicked(_)) => (),
            _ => panic!("Panic not reported properly"),
        }
        let handle = core.handle();
        assert_eq!(42, core.run(Coroutine::with_defaults(handle, || 42)).unwrap());
    }

    /// It's impossible to wait on a future outside of a coroutine
    #[test]
    #[should_panic]
    fn panic_without_coroutine() {
        Coroutine::wait(future::ok::<_, ()>(42));
    }

    // XXX Tests with dropping stuff
}
