//! The [`Coroutine`](struct.Coroutine.html) and related things.

use std::any::Any;
use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe, UnwindSafe};

use context::Context;
use context::stack::{Stack, ProtectedFixedSizeStack};
use futures::{Async, Future, Poll};
use futures::unsync::oneshot::{self, Receiver};

use errors::{Dropped, StackError, TaskFailed};
use switch::{Switch, WaitTask};

enum TaskResult<R> {
    Panicked(Box<Any + Send + 'static>),
    PanicPropagated,
    Lost,
    Finished(R),
}

/// A `Future` representing a completion of a coroutine.
///
/// Returns the result of the task the coroutine runs or the reason why it failed (it got lost
/// during shutdown or panicked).
pub struct CoroutineResult<R> {
    receiver: Receiver<TaskResult<R>>,
}

impl<R> Future for CoroutineResult<R> {
    type Item = R;
    type Error = TaskFailed;
    fn poll(&mut self) -> Poll<R, TaskFailed> {
        match self.receiver.poll() {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(TaskResult::Finished(result))) => Ok(Async::Ready(result)),
            Ok(Async::Ready(TaskResult::Panicked(reason))) => Err(TaskFailed::Panicked(reason)),
            Ok(Async::Ready(TaskResult::PanicPropagated)) => Err(TaskFailed::PanicPropagated),
            Ok(Async::Ready(TaskResult::Lost)) | Err(_) => Err(TaskFailed::Lost),
        }
    }
}

/// Controls how a cleanup happens if the driving executor is dropped while a coroutine lives.
///
/// If an executor is dropped and there is a coroutine that haven't finished yet, there's no chance
/// for it to make further progress and it becomes orphaned. The question is how to go about
/// cleaning it up, as there's a stack allocated somewhere, full of data that may need a destructor
/// run.
///
/// Some primitives for waiting for a future resolution in the coroutine return an error in such
/// case, but most of them panic ‒ panicking in the stack does exactly what is needed.
///
/// However, there are problems when the executor is dropped due to a panic itself ‒ then this would
/// double panic and abort the whole program.
///
/// This describes a strategy taken for the cleaning up of a coroutine (configured with
/// [`Coroutine::cleanup_strategy`](struct.Coroutine.html#method.cleanup_strategy).
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CleanupStrategy {
    /// Always perform cleanup.
    ///
    /// No matter the situation, perform a cleanup ‒ return the error or panic. This is the
    /// default.
    CleanupAlways,
    /// Cleanup the coroutine unless it is already panicking.
    ///
    /// This opts out to *not* clean up the coroutine and leak the data on its stack in case a
    /// panic is already happening. This prevents the double-panic problem, but doesn't free some
    /// resources or run destructors (this may lead to files not being flushed, for example).
    ///
    /// This is probably a good idea when panics lead to program termination anyway ‒ that way a
    /// better error message is output if only one panic happens.
    LeakOnPanic,
    /// Do not perform any cleanup.
    ///
    /// This is suitable if you never drop the executor until the end of the program. This doesn't
    /// perform the cleanup at all. This may lead to destructors not being run, which may lead to
    /// effects like files not being flushed.
    ///
    /// It may also be appropriate if your application is compiled with `panic = "abort"`.
    LeakAlways,
    /// Perform a cleanup in normal situation, abort if already panicking.
    ///
    /// This is similar to how `CleanupAlways` acts, but instead of relying on the double-panic to
    /// abort the program, do the abort right away.
    AbortOnPanic,
    /// Abort on any attempt to drop a living coroutine.
    ///
    /// This is mostly for completeness sake, but it may make some sense if you're sure the
    /// executor is never dropped while coroutines live.
    AbortAlways,
}

struct CoroutineContext {
    /// The context that called us and we'll switch back to it when we wait for something.
    parent_context: Context,
    /// Our own stack. We keep ourselves alive.
    stack: ProtectedFixedSizeStack,
    /// How do we clean up the coroutine if it doesn't end before dropping the core?
    cleanup_strategy: CleanupStrategy,
}

thread_local! {
    static CONTEXTS: RefCell<Vec<CoroutineContext>> = RefCell::new(Vec::new());
    static BUILDER: RefCell<Coroutine> = RefCell::new(Coroutine::new());
}

/// A builder of coroutines.
///
/// This struct is the main entry point and a way to start coroutines of various kinds. It allows
/// both starting them with default parameters and configuring them with the builder pattern.
#[derive(Clone, Debug)]
pub struct Coroutine {
    stack_size: usize,
    cleanup_strategy: CleanupStrategy,
}

impl Coroutine {
    /// Starts building a coroutine.
    ///
    /// This constructor produces a new builder for coroutines. The builder can then be used to
    /// specify configuration of the coroutines.
    ///
    /// It is possible to spawn multiple coroutines from the same builder.
    ///
    /// # Examples
    ///
    /// ```
    /// # extern crate corona;
    /// # extern crate tokio;
    /// use corona::Coroutine;
    /// use tokio::prelude::*;
    /// use tokio::runtime::current_thread;
    ///
    /// # fn main() {
    /// Coroutine::new()
    ///     .run(|| {})
    ///     .unwrap();
    /// # }
    ///
    /// ```
    pub fn new() -> Self {
        Coroutine {
            stack_size: Stack::default_size(),
            cleanup_strategy: CleanupStrategy::CleanupAlways,
        }
    }

    /// Configures the stack size used for coroutines.
    ///
    /// Coroutines spawned from this builder will get stack of this size. The default is something
    /// small, so if you use recursion, you might want to use this.
    ///
    /// Note that the size must be a valid stack size. This is platform dependent, but usually
    /// must be multiple of a page size. That usually means a multiple of 4096.
    ///
    /// If an invalid size is set, attempts to spawn coroutines will fail with an error.
    ///
    /// # Parameters
    ///
    /// * `size`: The stack size to use for newly spawned coroutines, in bytes.
    pub fn stack_size(&mut self, size: usize) -> &mut Self {
        self.stack_size = size;
        self
    }

    /// Configures how the coroutines should be cleaned up if the core is dropped before the
    /// coroutine resolves.
    ///
    /// See the details of [`CleanupStrategy`](enum.CleanupStrategy.html).
    pub fn cleanup_strategy(&mut self, strategy: CleanupStrategy) -> &mut Self {
        self.cleanup_strategy = strategy;
        self
    }

    /// Spawns a coroutine directly.
    ///
    /// This constructor spawns a coroutine with default parameters without the inconvenience of
    /// handling a builder. It is equivalent to spawning it with an unconfigured builder.
    ///
    /// Unlike the [`spawn`](#method.spawn.html), this one can't fail, since the default parameters
    /// of the builder are expected to always work (if they don't, file a bug).
    ///
    /// # Parameters
    ///
    /// * `task`: The closure to run inside the coroutine.
    ///
    /// # Returns
    ///
    /// A future that'll resolve once the coroutine completes, with the result of the `task`, or
    /// with an error explaining why the coroutine failed.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate tokio;
    /// use corona::Coroutine;
    /// use tokio::prelude::*;
    /// use tokio::runtime::current_thread;
    ///
    /// # fn main() {
    /// current_thread::block_on_all(future::lazy(|| {
    ///     Coroutine::with_defaults(|| {})
    /// })).unwrap();
    /// # }
    ///
    /// ```
    pub fn with_defaults<R, Task>(task: Task) -> CoroutineResult<R>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        Coroutine::new().spawn(task).unwrap()
    }

    /// The inner workings of `spawn` and `spawn_catch_panic`.
    fn spawn_inner<R, Task>(&self, task: Task, propagate_panic: bool)
        -> Result<CoroutineResult<R>, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + UnwindSafe + 'static,
    {
        let (sender, receiver) = oneshot::channel();

        let cleanup_strategy = self.cleanup_strategy;

        let perform = move |context, stack| {
            let my_context = CoroutineContext {
                parent_context: context,
                stack,
                cleanup_strategy,
            };
            CONTEXTS.with(|c| c.borrow_mut().push(my_context));
            let mut panic_result = None;
            let result = match panic::catch_unwind(AssertUnwindSafe(task)) {
                Ok(res) => TaskResult::Finished(res),
                Err(panic) => {
                    if panic.is::<Dropped>() {
                        TaskResult::Lost
                    } else if propagate_panic {
                        panic_result = Some(panic);
                        TaskResult::PanicPropagated
                    } else {
                        TaskResult::Panicked(panic)
                    }
                },
            };
            // We are not interested in errors. They just mean the receiver is no longer
            // interested, which is fine by us.
            drop(sender.send(result));
            let my_context = CONTEXTS.with(|c| c.borrow_mut().pop().unwrap());
            (my_context.parent_context, my_context.stack, panic_result)
        };
        Switch::run_new_coroutine(self.stack_size, Box::new(Some(perform)))?;

        Ok(CoroutineResult { receiver })
    }

    /// Spawns a coroutine with configuration from the builder.
    ///
    /// This spawns a new coroutine with the values previously set in the builder.
    ///
    /// # Parameters
    ///
    /// * `task`: The closure to run inside the coroutine.
    ///
    /// # Returns
    ///
    /// A future that'll resolve once the coroutine terminates and will yield the result of
    /// `task`, or an error explaining why the coroutine failed.
    ///
    /// This returns a `StackError` if the configured stack size is invalid.
    ///
    /// # Examples
    ///
    /// ```rust
    /// # extern crate corona;
    /// # extern crate tokio;
    /// use corona::Coroutine;
    /// use tokio::prelude::*;
    /// use tokio::runtime::current_thread;
    ///
    /// # fn main() {
    /// Coroutine::new()
    ///     .stack_size(40_960)
    ///     .run(|| { })
    ///     .unwrap();
    /// # }
    /// ```
    ///
    /// # Panic handling
    ///
    /// If the coroutine panics, the panic is propagated. This usually means the executor, unless
    /// the panic happens before the first suspension point, in which case it is the `spawn` itself
    /// which panics.
    pub fn spawn<R, Task>(&self, task: Task) -> Result<CoroutineResult<R>, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        // That AssertUnwindSafe is OK. We just pause the panic, teleport it to the callers thread
        // and then let it continue.
        self.spawn_inner(AssertUnwindSafe(task), true)
    }

    /// Spawns a coroutine, preventing the panics in it from killing the parent task.
    ///
    /// This is just like [spawn](#method.spawn), but any panic in the coroutine is captured and
    /// returned through the result instead of propagating.
    ///
    /// Note that you need to ensure the `task` is [unwind
    /// safe](https://doc.rust-lang.org/std/panic/trait.UnwindSafe.html) for that reason.
    pub fn spawn_catch_panic<R, Task>(&self, task: Task) -> Result<CoroutineResult<R>, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + UnwindSafe + 'static,
    {
        self.spawn_inner(task, false)
    }

    /// Waits for completion of a future.
    ///
    /// This suspends the execution of the current coroutine until the provided future is
    /// completed, possibly switching to other coroutines in the meantime.
    ///
    /// This is the low-level implementation of the waiting. It is expected user code will use the
    /// interface in [`prelude`](../prelude/index.html) instead.
    ///
    /// # Parameters
    ///
    /// * `fut`: The future to wait on.
    ///
    /// # Returns
    ///
    /// * `Ok(result)` with the result the future resolved to (which is in itself a `Result`).
    /// * `Err(Dropped)` when the executor was dropped before the future had a chance to resolve.
    ///
    /// # Panics
    ///
    /// If called outside of a coroutine (there's nothing to suspend).
    ///
    /// Also, panics from within the provided future are propagated into the calling coroutine.
    pub fn wait<I, E, Fut>(mut fut: Fut) -> Result<Result<I, E>, Dropped>
    where
        Fut: Future<Item = I, Error = E>,
    {
        // Grimoire marginalia (eg. a sidenote on the magic here).
        //
        // This is probably the hearth of the library both in the importance and complexity to
        // understand. We want to wait for the future to finish.
        //
        // To do that we do the following:
        // • Prepare a space for the result on our own stack.
        // • Prepare a wrapper future that'll do some bookkeeping around the user's future ‒ for
        //   example makes sure the wrapper future has the same signature and can be spawned onto
        //   the reactor.
        // • Switch to our parent context with the instruction to install the future for us into
        //   the reactor.
        //
        // Some time later, as the reactor runs, the future resolves. It'll do the following:
        // • Store the result into the prepared space on our stack.
        // • Switch the context back to us.
        // • This function resumes, picks ups the result from its stack and returns it.
        //
        // There are few unsafe blocks here, some of them looking a bit dangerous. So, some
        // rationale why this should be in fact safe.
        //
        // The handle.spawn() requires a 'static future. It is because the future will almost
        // certainly live longer than the stack frame that spawned it onto the reactor. Therefore,
        // the future must own anything it'll touch in some unknown later time.
        //
        // However, this is true in our case. The closure that runs in a coroutine is required to
        // be 'static. Therefore, anything non-'static must live on the coroutine's stack. And the
        // future has the only pointer to the stack of this coroutine, therefore effectively owns
        // the stack and everything on it.
        //
        // In other words, the stack is there for as long as the future waits idle in the reactor
        // and won't go away before the future either resolves or is dropped. There's a small trick
        // in the `drop` implementation and the future itself to ensure this is true even when
        // switching the contexts (it is true when we switch to this coroutine, but not after we
        // leave it, so the future's implementation must not touch the things afterwards.
        let my_context = CONTEXTS.with(|c| {
            c.borrow_mut().pop().expect("Can't wait outside of a coroutine")
        });
        let mut result: Option<Result<I, E>> = None;
        let (reply_instruction, context) = {
            // Shenanigans to make the closure pretend to be 'static to the compiler.
            let res_ref = &mut result as *mut _ as usize;
            let fut_ref = &mut fut as *mut _ as usize;

            let mut poll = move || {
                let fut = fut_ref as *mut Fut;
                let res = match unsafe { fut.as_mut() }.unwrap().poll() {
                    Ok(Async::NotReady) => return Ok(Async::NotReady),
                    Ok(Async::Ready(ok)) => Ok(ok),
                    Err(err) => Err(err),
                };
                let result = res_ref as *mut Option<Result<I, E>>;
                // Inject the result inside the place on the stack.
                unsafe { *result = Some(res) };
                Ok(Async::Ready(()))
            };
            let mut task = WaitTask {
                poll: &mut poll,
                context: None,
                cleanup_strategy: my_context.cleanup_strategy,
                stack: Some(my_context.stack),
            };
            let instruction = Switch::WaitFuture { task };
            instruction.exchange(my_context.parent_context)
        };
        let (result, stack) = match reply_instruction {
            Switch::Resume { stack } => (Ok(Ok(result.unwrap())), stack),
            Switch::Cleanup { stack } => (Ok(Err(Dropped)), stack),
            Switch::PropagateFuturePanic { stack, panic } => (Err(panic), stack),
            _ => unreachable!("Invalid instruction on wakeup"),
        };
        // Reconstruct our context anew after we switched back.
        let new_context = CoroutineContext {
            parent_context: context,
            stack: stack,
            cleanup_strategy: my_context.cleanup_strategy,
        };
        CONTEXTS.with(|c| c.borrow_mut().push(new_context));
        match result {
            Ok(result) => result,
            Err(panic) => panic::resume_unwind(panic),
        }
    }

    /// Checks if the current configuration is able spawn coroutines.
    ///
    /// The ability of a configuration is deterministic on the given system. Therefore, it is
    /// possible to check if further coroutines spawned by this builder would succeed. This does
    /// the check.
    pub fn verify(&self) -> Result<(), StackError> {
        // Try to spawn empty coroutine. That'd create the stack, etc, but there are no suspension
        // points in it, so it's safe even outside of the executor.
        self.spawn(|| ()).map(|_| ())
    }

    /// Puts the builder into a thread-local storage.
    ///
    /// This first verifies (see [`verify`](#method.verify)) the configuration in the builder is
    /// usable and then sets it into a thread-local storage.
    ///
    /// The thread-local storage is then used by the [`spawn`](fn.spawn.html) stand-alone function
    /// (in contrast to the [`spawn`](#method.spawn) method).
    ///
    /// If the verification fails, the original value is preserved. If not called, the thread-local
    /// storage contains a default configuration created by [`Coroutine::new`](#method.new).
    pub fn set_thread_local(&self) -> Result<(), StackError> {
        self.verify()?;
        BUILDER.with(|builder| builder.replace(self.clone()));
        Ok(())
    }

    /// Gets a copy of the builder in thread-local storage.
    ///
    /// This may help if you want to use the same builder as [`spawn`](fn.spawn.html) does, but you
    /// want to do something more fancy, like [`spawn_catch_panic`](#method.spawn_catch_panic).
    pub fn from_thread_local() -> Self {
        BUILDER.with(|builder| builder.borrow().clone())
    }

    /// Starts a whole runtime and waits for a main coroutine.
    ///
    /// While it is possible to create the `tokio::runtime::current_thread::Runtime` manually, feed
    /// it with a lazy future and then run a coroutine inside it (or reuse the runtime when
    /// something else creates it), this method is provided to take care of all these things,
    /// making it more convenient.
    ///
    /// In addition to starting the main coroutine passed to it, it sets the coroutine builder into
    /// a thread-local storage (see [`set_thread_local`](#method.set_thread_local).
    ///
    /// ```rust
    /// extern crate corona;
    /// extern crate tokio;
    ///
    /// use corona::prelude::*;
    /// use corona::spawn;
    /// use tokio::prelude::*;
    ///
    /// let result = Coroutine::new()
    ///     // 40kB of stack size for all the coroutines.
    ///     .stack_size(40960)
    ///     .run(|| {
    ///         // Everything (builder in thread local storage, coroutine, tokio runtime) is set up
    ///         // in here.
    ///         future::ok::<(), ()>(()).coro_wait();
    ///         let sub_coroutine = spawn(|| {
    ///             42
    ///         });
    ///         sub_coroutine.coro_wait().unwrap()
    ///     }).unwrap();
    /// assert_eq!(42, result);
    /// ```
    #[cfg(feature = "convenient-run")]
    pub fn run<R, Task>(&self, task: Task) -> Result<R, StackError>
    where
        R: 'static,
        Task: FnOnce() -> R + 'static,
    {
        self.set_thread_local()?;
        let result = ::tokio::runtime::current_thread::block_on_all(::futures::future::lazy(|| {
            spawn(task)
        })).expect("Lost a coroutine when waiting for all of them");
        Ok(result)
    }
}

impl Default for Coroutine {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawns a new coroutine.
///
/// This is very similar to [`Coroutine::spawn`](struct.Coroutine.html#method.spawn), but it uses
/// the coroutine builder in thread local storage (see
/// [`Coroutine::set_thread_local`](struct.Coroutine.html#method.set_thread_local)). This exists
/// merely as a convenience.
pub fn spawn<R, Task>(task: Task) -> CoroutineResult<R>
where
    R: 'static,
    Task: FnOnce() -> R + 'static,
{
    BUILDER.with(|builder| builder.borrow().spawn(task))
        .expect("Unverified builder in thread local storage")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::rc::Rc;
    use std::time::Duration;

    use futures::future;
    use tokio::clock;
    use tokio::prelude::*;
    use tokio::runtime::current_thread::{self, Runtime};
    use tokio::timer::Delay;

    use super::*;

    /// Test spawning and execution of tasks.
    #[test]
    fn spawn_some() {
        let s1 = Rc::new(AtomicBool::new(false));
        let s2 = Rc::new(AtomicBool::new(false));
        let s1c = s1.clone();
        let s2c = s2.clone();

        let mut builder = Coroutine::new();
        builder.stack_size(40960);
        let builder_inner = builder.clone();

        let result = builder.spawn(move || {
                let result = builder_inner.spawn(move || {
                        s2c.store(true, Ordering::Relaxed);
                        42
                    })
                    .unwrap();
                s1c.store(true, Ordering::Relaxed);
                result
            })
            .unwrap();

        // Both coroutines run to finish
        assert!(s1.load(Ordering::Relaxed), "The outer closure didn't run");
        assert!(s2.load(Ordering::Relaxed), "The inner closure didn't run");
        // The result gets propagated through.
        let extract = result.and_then(|r| r);
        assert_eq!(42, current_thread::block_on_all(extract).unwrap());
    }

    #[test]
    fn coroutine_run() {
        let result = Coroutine::new().run(|| {
            Coroutine::wait(future::ok::<(), ()>(())).unwrap().unwrap();
            42
        }).unwrap();
        assert_eq!(42, result);
    }

    /// Wait for a future to complete.
    #[test]
    fn future_wait() {
        let result = Coroutine::new().run(|| {
            let (sender, receiver) = oneshot::channel();
            let all_done = Coroutine::with_defaults(move || {
                let msg = Coroutine::wait(receiver).unwrap().unwrap();
                msg
            });
            Coroutine::with_defaults(move || {
                let timeout = Delay::new(clock::now() + Duration::from_millis(50));
                Coroutine::wait(timeout).unwrap().unwrap();
                sender.send(42).unwrap();
            });
            Coroutine::wait(all_done).unwrap().unwrap()
        });
        assert_eq!(42, result.unwrap());
    }

    /// The panic doesn't kill the main thread, but is reported.
    #[test]
    fn panics_catch() {
        let mut rt = Runtime::new().unwrap();
        let catch_panic = future::lazy(|| {
            Coroutine::new().spawn_catch_panic(|| panic!("Test")).unwrap()
        });
        match rt.block_on(catch_panic) {
            Err(TaskFailed::Panicked(_)) => (),
            _ => panic!("Panic not reported properly"),
        }
        assert_eq!(42, rt.block_on(future::lazy(|| Coroutine::with_defaults(|| 42))).unwrap());
    }

    /// However, normal coroutines do panic.
    #[test]
    #[should_panic]
    fn panics_spawn() {
        let _ = Coroutine::new().run(|| {
            spawn(|| panic!("Test"))
        });
    }

    /// This one panics and the panic is propagated, but after suspension point it is out of run.
    #[test]
    fn panics_run() {
        panic::catch_unwind(|| {
            current_thread::block_on_all(future::lazy(|| {
                Coroutine::with_defaults(|| {
                    let _ = Coroutine::wait(future::ok::<(), ()>(()));
                    panic!("Test");
                })
            }))
        }).unwrap_err();
    }

    /// It's impossible to wait on a future outside of a coroutine
    #[test]
    #[should_panic]
    fn panic_without_coroutine() {
        drop(Coroutine::wait(future::ok::<_, ()>(42)));
    }

    /// Tests leaking instead of double-panicking. This is tested simply by the tests not crashing
    /// hard.
    #[test]
    fn panic_leak() {
        panic::catch_unwind(|| current_thread::block_on_all(future::lazy(|| -> Result<(), ()> {
            let _coroutine = Coroutine::new()
                .cleanup_strategy(CleanupStrategy::LeakOnPanic)
                .spawn(|| {
                    let _ = Coroutine::wait(future::empty::<(), ()>());
                    panic!("Should never get here!");
                })
                .unwrap();
            panic!("Test");
        }))).unwrap_err();
        /*
         * FIXME: This doesn't work as intended. The sender gets leaked too, so it is never closed
         * and we don't get the Lost case. Any way to make sure we get it?
        if let Err(TaskFailed::Lost) = coroutine.wait() {
            // OK, correct error
        } else {
            panic!("Coroutine didn't get lost correctly");
        }
        */
    }

    /// Tests unconditional leaking on dropping the core. We panic in the destructor in there, so
    /// that checks it is not called.
    #[test]
    fn leak_always() {
        let mut rt = Runtime::new().unwrap();
        // This leaves the coroutine in there
        let _ = rt.block_on(future::lazy(|| {
            Coroutine::new()
                .cleanup_strategy(CleanupStrategy::LeakAlways)
                .spawn(|| {
                    struct Destroyer;
                    impl Drop for Destroyer {
                        fn drop(&mut self) {
                            panic!("Destructor called");
                        }
                    }
                    let _d = Destroyer;
                    let _ = Coroutine::wait(future::empty::<(), ()>());
                })
                .unwrap();
            Ok::<(), ()>(())
        }));
        drop(rt);
    }

    /// A panic from inside the future doesn't kill the core, but falls out of the wait into the
    /// responsible coroutine.
    ///
    /// We test this separately because the future is „exported“ to the main coroutine to be
    /// polled. So we check it doesn't kill the core.
    #[test]
    fn panic_in_future() {
        current_thread::block_on_all(future::lazy(|| {
            Coroutine::with_defaults(|| {
                struct PanicFuture;
                impl Future for PanicFuture {
                    type Item = ();
                    type Error = ();
                    fn poll(&mut self) -> Poll<(), ()> {
                        panic!("Test");
                    }
                }

                if let Ok(_) = panic::catch_unwind(|| Coroutine::wait(PanicFuture)) {
                    panic!("A panic should fall out of wait");
                }
            })
        })).unwrap();
    }
}
