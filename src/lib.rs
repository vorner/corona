#![doc(html_root_url = "https://docs.rs/corona/0.3.1/corona/")]
#![warn(missing_docs)]

//! A library combining futures and coroutines.
//!
//! This library brings stack-full coroutines. Each coroutine can asynchronously wait on futures
//! and is a future itself.
//!
//! # Motivation
//!
//! The current aim of Rust in regards to asynchronous programming is on
//! [`futures`](https://crates.io/crates/futures). They have some good properties, but some tasks
//! are more conveniently done in a more imperative way.
//!
//! There's the work in progress of [async-await](https://github.com/alexcrichton/futures-await).
//! But it requires nightly (for now), provides stack-less coroutines (which means the asynchronous
//! waiting can be done in a top-level function only) and there are too many `'static` bounds.
//!
//! This library brings a more convenient interface. However, it comes with a run-time cost, so you
//! might want to consider if you prefer ease of development or memory efficiency. Often, the
//! asynchronous communication isn't the bottleneck and you won't be handling millions of
//! concurrent connections, so this might be OK.
//!
//! # Pros
//!
//! * Easier to use than futures.
//! * Can integrate with futures.
//! * Allows working with borrowed futures.
//! * Provides safe interface.
//!
//! # Cons
//!
//! * Each coroutine needs its own stack, which is at least few memory pages large. This makes the
//!   library unsuitable when there are many concurrent coroutines.
//! * The coroutines can't move between threads once created safely.
//!
//! # How to use
//!
//! By bringing the `corona::prelude::*` into scope, all `Future`s, `Stream`s and `Sink`s get new
//! methods for asynchronous waiting on their completion or progress.
//!
//! The [`Coroutine`](coroutine/struct.Coroutine.html) is used to start a new coroutine. It acts a
//! bit like `std::thread::spawn`. However, all the coroutines run on the current thread and switch
//! to other coroutines whenever they wait on something.
//!
//! Each new coroutine returns a future. It resolves whenever the coroutine terminates. However,
//! the coroutine is *eager* ‒ it doesn't wait with the execution for the future to be polled. The
//! future can be dropped and the coroutine will still execute.
//!
//! ```rust
//! extern crate corona;
//! extern crate tokio_core;
//!
//! use std::time::Duration;
//! use corona::prelude::*;
//! use tokio_core::reactor::{Core, Timeout};
//!
//! fn main() {
//!     let mut core = Core::new().unwrap();
//!     let handle = core.handle();
//!     let coro = Coroutine::with_defaults(core.handle(), move || {
//!         let timeout = Timeout::new(Duration::from_millis(50), &handle).unwrap();
//!         // This will suspend the current coroutine. If there is some other one that is
//!         // ready to continue, it switches into that. If not, the current thread is
//!         // blocked until something can make progress.
//!         //
//!         // Don't confuse with .wait(), which blocks the whole thread.
//!         timeout.coro_wait().unwrap(); // Timeouts don't error.
//!         42 // Return value of the whole coroutine
//!     });
//!     // The reactor must be run so coroutines that wait on something get woken up.
//!     // We would get `Err(_)` if the coroutine panicked.
//!     assert_eq!(42, core.run(coro).unwrap());
//! }
//! ```
//!
//! Few things of note:
//!
//! * All the coroutine-aware methods panic outside of a coroutine.
//! * You can freely mix future and coroutine approach. Therefore, you can use combinators to build
//!   a future and then `coro_wait` on it.
//! * A coroutine spawned by [`spawn`](coroutine/struct.Coroutine.html#method.spawn) or
//!   [`with_defaults`](coroutine/struct.Coroutine.html#method.with_defaults) will propagate panics
//!   outside. One spawned with
//!   [`spawn_catch_panic`](coroutine/struct.Coroutine.html#method.spawn_catch_panic) captures the
//!   panic and passes it on through its result.
//! * Panicking outside of the coroutine where the reactor runs may lead to ugly things, like
//!   aborting the program (this'd usually lead to a double panic).
//! * Any of the waiting methods may switch to a different coroutine. Therefore it is not a good
//!   idea to hold a `RefCell` borrowed or a `Mutex` locked around that if another coroutine could
//!   also borrow it.
//!
//! The new methods are here:
//!
//! * [`Future`s](prelude/trait.CoroutineFuture.html)
//! * [`Stream`s](prelude/trait.CoroutineStream.html)
//! * [`Sink`s](prelude/trait.CoroutineSink.html)
//!
//! # Cleaning up
//!
//! If the reactor is dropped while a coroutine waits on something, the waiting method will panic.
//! That way the coroutine's stack is unwinded, releasing resources on its stack (there doesn't
//! seem to be a better way to drop the whole stack).
//!
//! However, if the reactor is dropped because of a panic, Rust abort the whole program because of
//! a double-panic. Ideas how to overcome this (since the second panic is on a different stack, but
//! Rust doesn't know that) are welcome.
//!
//! There are waiting methods that return an error instead of panicking, but they are less
//! convenient to use.
//!
//! # Pitfalls
//!
//! If the coroutine is created with default configuration, it gets really small stack. If you
//! overflow it, you get a segfault (it happens more often on debug builds than release ones) with
//! really useless backtrace. Try making the stack bigger in that case.
//!
//! # API Stability
//!
//! The API is likely to get stabilized soon (I hope it won't change any more). But I still  want
//! to do more experimentation before making it official.
//!
//! Other experiments are also welcome.
//!
//! # Known problems
//!
//! These are the problems I'm aware of and which I want to find a solution some day.
//!
//! * Many handy abstractions are still missing, like waiting for a future with a timeout, or
//!   conveniently waiting for a first of a set of futures or streams.
//! * A global configuration for the whole coroutine to leak on panic might be a convenient
//!   solution.
//! * The coroutines can't move between threads. This is likely impossible, since Rust's type
//!   system doesn't expect whole stacks with all local data to move.
//! * It relies on Tokio. This might change after the Tokio reform.
//! * The API doesn't prevent some footguns ‒ leaving a `RefCell` borrowed across coroutine switch,
//!   deadlocking, calling the waiting methods outside of a coroutine or using `.wait()` by a
//!   mistake and blocking the whole thread. These manifest during runtime.
//! * The cleaning up of coroutines when the reactor is dropped is done through panics.
//!
//! This is an example what *will* deadlock (eg. don't do this):
//!
//! ```rust,no_run
//! # extern crate corona;
//! # extern crate futures;
//! # extern crate tokio_core;
//! # use std::time::Duration;
//! # use corona::Coroutine;
//! # use corona::prelude::*;
//! # use futures::Future;
//! # use futures::unsync::oneshot;
//! # use tokio_core::reactor::{Core, Timeout};
//! #
//! # fn main() {
//! let mut core = Core::new().unwrap();
//! let (sender, receiver) = oneshot::channel();
//! let handle = core.handle();
//! let c = Coroutine::with_defaults(handle.clone(), move || {
//!     core.run(receiver).unwrap();
//! });
//! Coroutine::with_defaults(handle.clone(), move || {
//!     let timeout = Timeout::new(Duration::from_millis(50), &handle).unwrap();
//!     timeout.coro_wait().unwrap();
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
//! use futures::Sink;
//! use futures::unsync::mpsc;
//! use corona::prelude::*;
//!
//! use tokio_core::reactor::{Core, Timeout};
//!
//! # fn main() {
//! let mut core = Core::new().unwrap();
//! let handle = core.handle();
//! let builder = Coroutine::new(core.handle());
//! let (sender, receiver) = mpsc::channel(1);
//! builder.spawn(|| {
//!         let mut sender = sender;
//!         sender = sender.send(1).coro_wait().unwrap();
//!         sender = sender.send(2).coro_wait().unwrap();
//!     })
//!     .unwrap();
//! let coroutine = builder.spawn(move || {
//!         for item in receiver.iter_ok() {
//!             println!("{}", item);
//!         }
//!
//!         let timeout = Timeout::new(Duration::from_millis(100), &handle).unwrap();
//!         timeout.coro_wait().unwrap();
//!
//!         42
//!     })
//!     .unwrap();
//! assert_eq!(42, core.run(coroutine).unwrap());
//! # }
//! ```
//!
//! Further examples can be found in the
//! [repository](https://github.com/vorner/corona/tree/master/examples).
//!
//! # Behind the scenes
//!
//! There are few things that might help understanding how the library works inside.
//!
//! First, there's some thread-local state. This state is used for caching currently unused stacks
//! as well as the state that is used when waiting for something and switching coroutines (eg. this
//! state contains the handle to the reactor).
//!
//! Whenever one of the waiting methods is used, a wrapper future is created. After the original
//! future resolves, it resumes the execution to the current stack. This future is spawned onto the
//! reactor and a switch is made to the parent coroutine (it's the coroutine that started or
//! resumed the current one). This way, the „outside“ coroutine is reached eventually. It is
//! expected this outside coroutine will run the reactor.
//!
//! That's about it, the rest of the library are just implementation details about what is stored
//! where and how to pass the information around without breaking any lifetime bounds.

extern crate context;
extern crate futures;
extern crate tokio_core;

pub mod errors;
pub mod prelude;
pub mod wrappers;

mod coroutine;
mod stack_cache;
mod switch;

pub use errors::{Dropped, TaskFailed};
pub use coroutine::{Coroutine, CoroutineResult};
