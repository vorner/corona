* Added some benchmarks to measure the overhead and compare with others.
* Introduced the BlockingWrapper to wrap AsyncRead/AsyncWrite things and turn
  them into blocking (blocking only the coroutine). This allows them to be used
  in futures-unaware APIs expecting Read/Write.
* A panic inside a future propagates to the owning coroutine, doesn't kill the
  whole core.
* The `spawn` method no longer catches panic by default. The
  `spawn_catch_panic`.

# 0.3.1

* Made the `Coroutine::new()` builder more ergonomic to use.
* Documentation hint about stack sizes.

# 0.3.0

Redesign of the API. The async context is implicit in thread local storage. The
interface is easier to work with and looks cleaner, at the cost of checking some
misuses at runtime.

Old code can be adapted mostly by removing the parameter of the closure passed
to `Coroutine::new()`.
