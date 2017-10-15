use std::any::Any;

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
