# The Corona benchmarks

These benchmarks try to check the overhead of Corona isn't simply insane. They
run series of tests, using different libraries and paradigms.

What each test does:

* Starts a server. Each server is different (using the currently measured
  library).
* Starts bunch of client threads. Each thread creates several connections to the
  server.
* Each client thread then repeatedly writes and reads over all its connections.
* The server echoes all the messages back.

The benchmark is highly IO-heavy ‒ neither the clients nor the server do any
computations, they just read and write data.

## Naming

Each library has three different tests. One without a suffix, one with `_many`
and another with `_cpus`. This signifies the parallelism of the server.

With no prefix, single server is started. Depending on the library or technique
used, this may mean that only one thread is used or that the load is spread by
the master thread to some other, slave threads. The number of slave threads is
auto-configured to the amount of CPUs available.

The `_many` version runs configurable number of servers in parallel. In case the
technique spreads the load between further work threads, the pool of work
threads is the same size as with no suffix, but there are more master threads.
In case of the single-threaded approaches, it is made multi-threaded by running
parallel (independent) servers on the same port.

The `_cpus` version is like `_many`, but the number of instances is configured
automatically to the number of available CPUs.

## Disadvantages

* It doesn't measure a „real“ workload, only the switching overhead.
* It uses a huge number of connections to localhost. Sometimes, the benchmark
  fails due to that. It may be possible to work around by allowing more ports
  for local endpoint of the connections, by
  `echo 1024 65535 > /proc/sys/net/ipv4/ip_local_port_range`.
* Sometimes the measurements are bit unstable or the harness over-estimates the
  number of iterations and runs for a very long time.
* The client threads run on the same machine. Therefore, they also consume CPU
  power, fighting for it with the server instances. Still, slower servers will
  produce slower benchmarks.

Therefore, this needs to be taken with a grain of salt.

## Configuration

There are several environment variables (look into the source code) that
configure aspects of the benchmarks ‒ how many threads are started, how many
messages are exchanged on each connection, etc.
