//! Module for the low-level switching of coroutines

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::thread;

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::{Async, Future, Poll};
use tokio_core::reactor::Handle;

use errors::StackError;
use stack_cache;

/// A workaround befause Box<FnOnce> is currently very unusable in rust :-(.
pub(crate) trait BoxableTask {
    fn perform(&mut self, Context, ProtectedFixedSizeStack) ->
        (Context, ProtectedFixedSizeStack, Option<Box<Any + Send>>);
}

impl<F> BoxableTask for Option<F>
where
    F: FnOnce(Context, ProtectedFixedSizeStack) ->
        (Context, ProtectedFixedSizeStack, Option<Box<Any + Send>>),
{
    fn perform(&mut self, context: Context, stack: ProtectedFixedSizeStack) ->
        (Context, ProtectedFixedSizeStack, Option<Box<Any + Send>>)
    {
        self.take().unwrap()(context, stack)
    }
}

/// A fake Box<FnOnce(Context) -> Context>.
type BoxedTask = Box<BoxableTask>;

pub(crate) struct WaitTask {
    pub(crate) poll: *mut FnMut() -> Poll<(), ()>,
    pub(crate) context: Option<Context>,
    pub(crate) stack: Option<ProtectedFixedSizeStack>,
    pub(crate) handle: Handle,
    pub(crate) leak_on_panic: bool,
}

impl Future for WaitTask {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Poll<(), ()> {
        assert!(self.context.is_some());
        // The catch unwind is fine ‒ we don't swallow the panic, only move it to the correct place
        // ‒ so likely everything relevant will be dropped like with any other normal panic.
        match panic::catch_unwind(AssertUnwindSafe(unsafe {
            // The future is still not dangling pointer ‒ we never resumed the stack
            self.poll
                .as_mut()
                .unwrap()
        })) {
            Ok(Ok(Async::NotReady)) => Ok(Async::NotReady),
            Ok(result) => {
                Switch::Resume {
                        stack: self.stack.take().unwrap(),
                    }
                    .run_child(self.context.take().unwrap());
                result
            },
            Err(panic) => {
                Switch::PropagateFuturePanic {
                        stack: self.stack.take().unwrap(),
                        panic
                    }
                    .run_child(self.context.take().unwrap());
                Err(())
            },
        }
    }
}

impl Drop for WaitTask {
    fn drop(&mut self) {
        if let Some(context) = self.context.take() {
            if self.leak_on_panic && thread::panicking() {
                // We leak all the things on the stack, but we get rid of the stack itself at least
                // (by letting it disappear with us)
                return;
            }
            Switch::Cleanup {
                    stack: self.stack.take().expect("Taken stack, but not context?")
                }
                .run_child(context);
        }
    }
}


/// Execution of a coroutine.
///
/// This holds the extracted logic, so once we leave the coroutine, all locals that may possibly
/// have any kind of destructor are gone.
fn coroutine_internal(transfer: Transfer) -> (Switch, Context) {
    let mut context = transfer.context;
    let switch = Switch::extract(transfer.data);
    let result = match switch {
        Switch::StartTask { stack, mut task } => {
            let (ctx, stack, panic) = task.perform(context, stack);
            context = ctx;
            Switch::Destroy {
                stack,
                panic,
            }
        },
        _ => panic!("Invalid switch instruction on coroutine entry"),
    };
    (result, context)
}

/// Wrapper for the execution of a coroutine.
///
/// This is just a minimal wrapper that runs the `coroutine_internal` and then switches back to the
/// parent context. This contains very minimal amount of local variables and only the ones from the
/// `context` crate, so we don't have anything with destructor here. The problem is, this function
/// never finishes and therefore such destructors wouldn't be run.
extern "C" fn coroutine(transfer: Transfer) -> ! {
    let (result, context) = coroutine_internal(transfer);
    result.exchange(context);
    unreachable!("Woken up after termination!");
}

/// An instruction carried across the coroutine boundary.
///
/// This describes what the receiving coroutine should do next (and contains parameters for that).
/// It also holds the methods to do the actual switching.
///
/// Note that not all instructions are valid at all contexts, but as this is not an API visible
/// outside of the crate, that's likely OK with checking not thing invalid gets received.
pub(crate) enum Switch {
    /// Start a new task in the coroutine.
    StartTask {
        stack: ProtectedFixedSizeStack,
        task: BoxedTask,
    },
    /// Wait on a future to finish
    WaitFuture {
        task: WaitTask,
    },
    /// A future panicked, propagate it into the coroutine.
    PropagateFuturePanic {
        stack: ProtectedFixedSizeStack,
        panic: Box<Any + Send>,
    },
    /// Continue operation, the future is resolved.
    Resume {
        stack: ProtectedFixedSizeStack,
    },
    /// Abort the coroutine and clean up the resources.
    Cleanup {
        stack: ProtectedFixedSizeStack,
    },
    /// Get rid of the sending coroutine, it terminated.
    Destroy {
        stack: ProtectedFixedSizeStack,
        /// In case the coroutine panicked and the panic should continue.
        panic: Option<Box<Any + Send>>,
    },
}

impl Switch {
    /// Extracts the instruction passed through the coroutine transfer data.
    fn extract(transfer_data: usize) -> Switch {
        let ptr = transfer_data as *mut Option<Self>;
        // The extract is called only in two cases. When switching into a newly born coroutine and
        // during the exchange of two coroutines. In both cases, the caller is in this module, it
        // places data onto its stack and passes the pointer as the usize parameter (which is
        // safe). The stack is still alive at the time we are called and it hasn't moved (since our
        // stack got the control), so the pointer is not dangling. We just extract the data from
        // there right away and leave None on the stack, which doesn't need any special handling
        // during destruction, etc.
        let optref = unsafe { ptr.as_mut() }
            .expect("NULL pointer passed through a coroutine switch");
        optref.take().expect("Switch instruction already extracted")
    }
    /// Switches to a coroutine and back.
    ///
    /// Switches to the given context (coroutine) and sends it the current instruction. Returns the
    /// context that resumed us (after we are resumed) and provides the instruction it send us.
    ///
    /// # Internals
    ///
    /// There are two stacks in the play, the current one and one we want to transition into
    /// (described by the passed `context`). We also pass a `Switch` *instruction* along the
    /// transition.
    ///
    /// To pass the instruction, we abuse the usize `data` field in the underlying library for
    /// switching stacks (also called `context`). To do that, we place the instruction into a
    /// `Option<Switch>` on the current stack. We pass a pointer to that `Option` through that
    /// `usize`. The receiving coroutine takes the instruction out of the `Option`, stealing it
    /// from the originating stack. The originating stack doesn't change until we pass back here.
    ///
    /// Some future exchange from that (or possibly other) stack into this will do the reverse ‒
    /// activate this stack and it'll extract the instruction from that stack.
    ///
    /// As the exchange leaves just an empty `Option` behind, destroying the stack (once it asks
    /// for so through the instruction) is safe, we don't need to run any destructor on that.
    pub(crate) fn exchange(self, context: Context) -> (Self, Context) {
        let mut sw = Some(self);
        let swp: *mut Option<Self> = &mut sw;
        // We store the switch instruction onto the current stack and pass a pointer for it to the
        // other coroutine. It will get extracted just as the first thing the other coroutine does,
        // therefore at the time this stack frame is still active.
        //
        // Also, switching to the other coroutine is OK, because each coroutine owns its own stack
        // (it has it passed to it and it keeps it on its own stack until it decides to terminate
        // and passes it back through the instruction on switching out). So the stack can't get
        // destroyed prematurely.
        let transfer = unsafe { context.resume(swp as usize) };
        (Self::extract(transfer.data), transfer.context)
    }
    /// Runs a child coroutine (one that does the work, is not a control coroutine) and once it
    /// returns, handles its return instruction.
    pub(crate) fn run_child(self, context: Context) {
        let (reply, context) = self.exchange(context);
        use self::Switch::*;
        match reply {
            Destroy { stack, panic } => {
                drop(context);
                stack_cache::put(stack);
                if let Some(panic) = panic {
                    panic::resume_unwind(panic);
                }
            },
            WaitFuture { mut task } => {
                task.context = Some(context);
                let handle = task.handle.clone();
                handle.spawn(task);
            },
            _ => unreachable!("Invalid switch instruction when switching out"),
        }
    }
    /// Creates a new coroutine and runs it.
    pub(crate) fn run_new_coroutine(stack_size: usize, task: BoxedTask) -> Result<(), StackError> {
        let stack = stack_cache::get(stack_size)?;
        assert_eq!(stack.len(), stack_size);
        // The `Context::new` is unsafe only because we have to promise not to delete the stack
        // prematurely, while the coroutine is still alive. We ensure that by giving the ownership
        // of the stack to the coroutine and it gives it up only once it is ready to terminate.
        let context = unsafe { Context::new(&stack, coroutine) };
        Switch::StartTask { stack, task }.run_child(context);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn switch_coroutine() {
        let called = Rc::new(Cell::new(false));
        let called_cp = called.clone();
        let task = move |context, stack| {
            called_cp.set(true);
            (context, stack, None)
        };
        Switch::run_new_coroutine(40960, Box::new(Some(task))).unwrap();
        assert!(called.get());
        assert_eq!(1, Rc::strong_count(&called));
    }
}
