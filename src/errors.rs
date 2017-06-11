use std::any::Any;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub enum TaskResult<R> {
    Panicked(Box<Any + Send + 'static>),
    Finished(R),
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

