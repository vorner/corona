//! Module for the low-level switching of coroutines

use std::cell::RefCell;

use context::{Context, Transfer};
use context::stack::ProtectedFixedSizeStack;
use futures::{Async, Future, Poll};
use tokio_core::reactor::Handle;

use stack_cache;

/// A workaround befause Box<FnOnce> is currently very unusable in rust :-(.
pub trait BoxableTask {
    fn perform(&mut self, Context, ProtectedFixedSizeStack) -> (Context, ProtectedFixedSizeStack);
}

impl<F: FnOnce(Context, ProtectedFixedSizeStack) -> (Context, ProtectedFixedSizeStack)> BoxableTask for Option<F> {
    fn perform(&mut self, context: Context, stack: ProtectedFixedSizeStack) -> (Context, ProtectedFixedSizeStack) {
        self.take().unwrap()(context, stack)
    }
}

/// A fake Box<FnOnce(Context) -> Context>.
type BoxedTask = Box<BoxableTask>;

/// A future that a coroutine waits on.
type WakeupAfter = Box<Future<Item = (), Error = ()>>;

struct Wakeup {
    after: WakeupAfter,
    context: Option<Context>,
}

impl Future for Wakeup {
    type Item = ();
    type Error = ();
    fn poll(&mut self) -> Poll<(), ()> {
        assert!(self.context.is_some());
        match self.after.poll() {
            Ok(Async::Ready(())) => {
                Switch::Resume.run_child(self.context.take().unwrap());
                Ok(Async::Ready(()))
            }
            other => other,
        }
    }
}

impl Drop for Wakeup {
    fn drop(&mut self) {
        if let Some(context) = self.context.take() {
            // In case the future hasn't fired, ask the coroutine to clean up
            Switch::Cleanup.run_child(context);
        }
    }
}

/// Execution of a coroutine.
///
/// This holds the extracted logic, so once we leave the coroutine, all locals that may possibly
/// have any kind of destructor are gone.
fn coroutine_internal(transfer: Transfer) -> Context {
    let mut context = transfer.context;
    let switch = Switch::get();
    let result = match switch {
        Switch::StartTask { stack, mut task } => {
            let (ctx, stack) = task.perform(context, stack);
            context = ctx;
            Switch::Destroy { stack }
        },
        _ => panic!("Invalid switch instruction on coroutine entry"),
    };
    result.put();
    context
}

/// Wrapper for the execution of a coroutine.
///
/// This is just a minimal wrapper that runs the `coroutine_internal` and then switches back to the
/// parent context. This contains very minimal amount of local variables and only the ones from the
/// `context` crate, so we don't have anything with destructor here. The problem is, this function
/// never finishes and therefore such destructors wouldn't be run.
extern "C" fn coroutine(transfer: Transfer) -> ! {
    let context = coroutine_internal(transfer);
    unsafe { context.resume(0) };
    unreachable!("Woken up after termination!");
}

// TODO: We could actually pass this through the data field of the transfer
/// An instruction carried across the coroutine boundary.
///
/// This describes what the receiving coroutine should do next (and contains parameters for that).
/// It also holds the methods to do the actual switching.
///
/// Note that not all instructions are valid at all contexts, but as this is not an API visible
/// outside of the crate, that's likely OK with checking not thing invalid gets received.
pub enum Switch {
    /// Start a new task in the coroutine.
    StartTask {
        stack: ProtectedFixedSizeStack,
        task: BoxedTask,
    },
    /// Please wake the sending coroutine once the `after` future resolves.
    ScheduleWakeup {
        after: WakeupAfter,
        handle: Handle,
    },
    /// Continue operation, the future is resolved.
    Resume,
    /// Abort the coroutine and clean up the resources.
    Cleanup,
    /// Get rid of the sending coroutine, it terminated.
    Destroy {
        stack: ProtectedFixedSizeStack,
    },
}

thread_local! {
    static SWITCH: RefCell<Option<Switch>> = RefCell::new(None);
}

impl Switch {
    /// Stores the switch instruction in a thread-local variable, so it can be taken out in another
    /// coroutine.
    fn put(self) {
        SWITCH.with(|s| {
            let mut s = s.borrow_mut();
            assert!(s.is_none(), "Leftover switch instruction");
            *s = Some(self);
        });
    }
    /// Extracts the instruction stored in a thread-local variable.
    fn get() -> Self {
        SWITCH.with(|s| s.borrow_mut().take().expect("Missing switch instruction"))
    }
    /// Switches to a coroutine and back.
    ///
    /// Switches to the given context (coroutine) and sends it the current instruction. Returns the
    /// context that resumed us (after we are resumed) and provides the instruction it send us.
    pub fn exchange(self, context: Context) -> (Self, Context) {
        self.put();
        let transfer = unsafe { context.resume(0) };
        let switch = Self::get();
        (switch, transfer.context)
    }
    /// Runs a child coroutine (one that does the work, is not a control coroutine) and once it
    /// returns, handles its return instruction.
    fn run_child(self, context: Context) {
        let (reply, context) = self.exchange(context);
        use Switch::*;
        match reply {
            Destroy { stack } => {
                drop(context);
                stack_cache::put(stack);
            },
            ScheduleWakeup { after, handle } => {
                let wakeup = Wakeup {
                    context: Some(context),
                    after,
                };
                handle.spawn(wakeup);
            },
            _ => unreachable!("Invalid switch instruction when switching out"),
        }
    }
    /// Creates a new coroutine and runs it.
    pub fn run_new_coroutine(stack_size: usize, task: BoxedTask) {
        let stack = stack_cache::get(stack_size);
        assert_eq!(stack.len(), stack_size);
        let context = unsafe { Context::new(&stack, coroutine) };
        Switch::StartTask { stack, task }.run_child(context);
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
            (context, stack)
        };
        Switch::run_new_coroutine(40960, Box::new(Some(task)));
        assert!(called.get());
        assert_eq!(1, Rc::strong_count(&called));
    }
}
