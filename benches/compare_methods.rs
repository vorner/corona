#![feature(conservative_impl_trait, generators, proc_macro, test)]

//! Minimal benchmarks and comparison of some IO manipulation.
//!
//! This tries to compare speed of different methods how to implement a networked server. The
//! servers differ, while the client is always the same.
//!
//! The client opens `PARALLEL` connections to the server, then `EXCHANGES` times sends a buffer of
//! data through each of the connection and expects an answer back.
//!
//! Note that we leave the server threads running after the benchmark terminates, to avoid the need
//! to synchronize shut down. As they just sit there inactive, this should have no real effect on
//! the performance.
//!
//! The `*_many` varianst run the listener in multiple independent threads.

extern crate corona;
extern crate futures_await as futures;
extern crate futures_cpupool;
#[macro_use]
extern crate lazy_static;
extern crate net2;
extern crate test;
extern crate tokio_core;
extern crate tokio_io;

use std::env;
use std::io::{Read, Write};
use std::net::{TcpStream, TcpListener, SocketAddr};
use std::sync::mpsc;
use std::thread;

use corona::prelude::*;
use futures::{stream, Future, Stream};
use futures::prelude::*;
use futures_cpupool::CpuPool;
use net2::TcpBuilder;
use tokio_core::net::TcpListener as TokioTcpListener;
use tokio_core::reactor::Core;
use tokio_io::io;
use test::Bencher;

const BUF_SIZE: usize = 1024;

fn get_var(name: &str, default: usize) -> usize {
    env::var(name)
        .map_err(|_| ())
        .and_then(|s| s.parse().map_err(|_| ()))
        .unwrap_or(default)
}

lazy_static! {
    static ref POOL: CpuPool = CpuPool::new_num_cpus();
    static ref PARALLEL: usize = get_var("PARALLEL", 512);
    static ref EXCHANGES: usize = get_var("EXCHANGES", 6);
    static ref WARMUP: usize = get_var("WARMUP", 10);
    static ref BATCH: usize = get_var("BATCH", 20);
    static ref CLIENT_THREADS: usize = get_var("CLIENT_THREADS", 32);
    static ref SERVER_THREADS: usize = get_var("SERVER_THREADS", 4);
}

/// The client side
fn batter(addr: SocketAddr) {
    let mut streams = (0..*PARALLEL / *CLIENT_THREADS).map(|_| {
            TcpStream::connect(&addr)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let input = [1u8; BUF_SIZE];
    let mut output = [0u8; BUF_SIZE];
    for _ in 0..*EXCHANGES {
        for stream in &mut streams {
            stream.write_all(&input[..]).unwrap();
        }
        for stream in &mut streams {
            stream.read_exact(&mut output[..]).unwrap();
        }
    }
}

/// Performs one benchmark, with the body as the server implementation
///
/// There's a short warm-up before the actual benchmark starts ‒ both to initialize whatever
/// buffers or caches the library uses and to make sure the server already started after the
/// barrier.
///
/// We run the clients in multiple threads (so the server is kept busy). To not start and stop a
/// lot of client threads, we report the progress through a sync channel.
fn bench(b: &mut Bencher, paral: usize, body: fn(TcpListener)) {
    let listener = TcpBuilder::new_v4()
        .unwrap()
        .reuse_address(true)
        .unwrap()
        .bind("127.0.0.1:0")
        .unwrap()
        .listen(4096)
        .unwrap();
    let addr = listener.local_addr().unwrap();
    for _ in 0..paral {
        let listener = listener.try_clone().unwrap();
        thread::spawn(move || body(listener));
    }
    let (sender, receiver) = mpsc::sync_channel(*CLIENT_THREADS * 10);
    for _ in 0..*CLIENT_THREADS {
        let sender = sender.clone();
        let addr = addr.clone();
        thread::spawn(move || {
            while let Ok(_) = sender.send(()) {
                for _ in 0..*BATCH {
                    batter(addr);
                }
            }
        });
    }
    for _ in 0..*WARMUP * *CLIENT_THREADS {
        receiver.recv().unwrap();
    }
    b.iter(move || receiver.recv().unwrap());
}

fn run_corona(listener: TcpListener) {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let main = Coroutine::with_defaults(core.handle(), move || {
        let addr = listener.local_addr().unwrap();
        let incoming = TokioTcpListener::from_listener(listener, &addr, &handle)
            .unwrap()
            .incoming()
            .iter_ok();
        for (mut connection, _address) in incoming {
            Coroutine::with_defaults(handle.clone(), move || {
                let mut buf = [0u8; BUF_SIZE];
                for _ in 0..*EXCHANGES {
                    io::read_exact(&mut connection, &mut buf[..])
                        .coro_wait()
                        .unwrap();
                    io::write_all(&mut connection, &buf[..])
                        .coro_wait()
                        .unwrap();
                }
            });
        }
    });
    core.run(main).unwrap();
}

/// Our own corona.
#[bench]
fn corona(b: &mut Bencher) {
    bench(b, 1, run_corona);
}

#[bench]
fn corona_many(b: &mut Bencher) {
    bench(b, *SERVER_THREADS, run_corona);
}

fn run_threads(listener: TcpListener) {
    while let Ok((mut connection, _address)) = listener.accept() {
        thread::spawn(move || {
            let mut buf = [0u8; BUF_SIZE];
            for _ in 0..*EXCHANGES {
                connection.read_exact(&mut buf[..]).unwrap();
                connection.write_all(&buf[..]).unwrap();
            }
        });
    }
}

/// Runs a fresh thread for each connection
///
/// This might happen to be slightly faster because it may use more CPU parallelism. Or maybe
/// there's just less overhead due to the event loop ‒ but there's the price to switch threads.
#[bench]
fn threads(b: &mut Bencher) {
    bench(b, 1, run_threads);
}

#[bench]
fn threads_many(b: &mut Bencher) {
    bench(b, *SERVER_THREADS, run_threads);
}

fn run_futures(listener: TcpListener) {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let addr = listener.local_addr().unwrap();
    let main = TokioTcpListener::from_listener(listener, &addr, &handle)
        .unwrap()
        .incoming()
        .for_each(move |(connection, _addr)| {
            let buf = vec![0u8; BUF_SIZE];
            let perform = stream::iter_ok(0..*EXCHANGES)
                .fold((connection, buf), |(connection, buf), _i| {
                    io::read_exact(connection, buf)
                        .and_then(|(connection, buf)| io::write_all(connection, buf))
                })
            .map(|_| ())
                .map_err(|e: std::io::Error| panic!("{}", e));
            handle.spawn(perform);
            Ok(())
        });
    core.run(main).unwrap();
}

/// Just plain futures.
///
/// While faster than corona, it is probably harder to read and write.
#[bench]
fn futures(b: &mut Bencher) {
    bench(b, 1, run_futures);
}

#[bench]
fn futures_many(b: &mut Bencher) {
    bench(b, *SERVER_THREADS, run_futures);
}

fn run_futures_cpupool(listener: TcpListener) {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let addr = listener.local_addr().unwrap();
    let main = TokioTcpListener::from_listener(listener, &addr, &handle)
        .unwrap()
        .incoming()
        .for_each(move |(connection, _addr)| {
            let buf = vec![0u8; BUF_SIZE];
            let perform = stream::iter_ok(0..*EXCHANGES)
                .fold((connection, buf), |(connection, buf), _i| {
                    io::read_exact(connection, buf)
                        .and_then(|(connection, buf)| io::write_all(connection, buf))
                })
            .map(|_| ());
            let offloaded = POOL.spawn(perform)
                .map(|_| ())
                .map_err(|e: std::io::Error| panic!("{}", e));
            handle.spawn(offloaded);
            Ok(())
        });
    core.run(main).unwrap();
}

/// Like `futures`, but uses cpu pool.
#[bench]
fn futures_cpupool(b: &mut Bencher) {
    bench(b, 1, run_futures_cpupool);
}

#[bench]
fn futures_cpupool_many(b: &mut Bencher) {
    bench(b, *SERVER_THREADS, run_futures_cpupool);
}

fn run_async(listener: TcpListener) {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let addr = listener.local_addr().unwrap();
    let incoming = TokioTcpListener::from_listener(listener, &addr, &handle)
        .unwrap()
        .incoming();
    let main = async_block! {
        #[async]
        for (mut connection, _addr) in incoming {
            let client = async_block! {
                    let mut buf = vec![0u8; BUF_SIZE];
                    for _ in 0..*EXCHANGES {
                        let (c, b) = await!(io::read_exact(connection, buf))?;
                        let (c, b) = await!(io::write_all(c, b))?;
                        connection = c;
                        buf = b;
                    }
                    Ok(())
                }
                .map(|_| ())
                .map_err(|e: std::io::Error| panic!(e));
            handle.spawn(client);
        }
        Ok::<_, std::io::Error>(())
    };
    core.run(main).unwrap();
}

/// With the futures-async magic
#[bench]
fn async(b: &mut Bencher) {
    bench(b, 1, run_async);
}

#[bench]
fn async_many(b: &mut Bencher) {
    bench(b, *SERVER_THREADS, run_async);
}
