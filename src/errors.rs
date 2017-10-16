use std::any::Any;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// An error marker when a future is dropped before having change to get resolved.
///
/// If you wait on a future and the corresponding executor gets destroyed before the future has a
/// chance to run, this error is returned as there's no chance the future will ever get resolved.
/// It is up to the waiter to clean up the stack, or use methods that panic implicitly.
#[derive(Debug)]
pub struct Dropped;

impl Error for Dropped {
    fn description(&self) -> &str {
        "The future waited on has been dropped before resolving"
    }
}

impl Display for Dropped {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

/// The task (coroutine) has failed.
///
/// This is used as an error type and represents an unsuccessfull coroutine.
#[derive(Debug)]
pub enum TaskFailed {
    /// There was a panic inside the coroutine.
    Panicked(Box<Any + Send + 'static>),
    /// The coroutine was lost.
    ///
    /// This can happen in case the `tokio_core::reactor::Core` the coroutine was spawned onto was
    /// dropped before the coroutine completed.
    Lost,
}

impl Error for TaskFailed {
    fn description(&self) -> &str {
        match *self {
            TaskFailed::Panicked(_) => "The coroutine panicked",
            TaskFailed::Lost => "The coroutine was lost",
        }
    }
}

impl Display for TaskFailed {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}
