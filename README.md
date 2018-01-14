# Corona

[![Travis Build Status](https://api.travis-ci.org/vorner/corona.png?branch=master)](https://travis-ci.org/vorner/corona)
[![AppVeyor Build status](https://ci.appveyor.com/api/projects/status/ygytb97bion810ru/branch/master?svg=true)](https://ci.appveyor.com/project/vorner/corona/branch/master)

When you need to get the asynchronous out of the way.

Corona is a library providing stackful coroutines for Rust. They integrate well
with futures ‒ it is possible to switch between the abstractions as needed, each
coroutine is also a future and a coroutine can wait for a future to complete.
Furthermore, the futures don't have to be `'static`.

On the other hand, there's a runtime cost to the library. The performance does
not necessarily suffer (that yet needs to be measured, but the fact a future
doesn't need to own its data may actually make it *faster*). But each coroutine
has its own stack, which takes memory.

You want to read the [docs](https://docs.rs/corona) and examine the
[examples](https://github.com/vorner/corona/tree/master/examples).

# Status

I hope to stabilize the API soon. But I want to write some more examples and
experiments first.

If you use it for something, I want to hear about it.

## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms
or conditions.
