#![feature(test)]

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

extern crate corona;
extern crate futures;
extern crate test;
extern crate tokio_core;
extern crate tokio_io;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpStream, TcpListener, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Barrier};
use std::thread;

use corona::prelude::*;
use futures::{stream, Future, Stream};
use tokio_core::net::TcpListener as TokioTcpListener;
use tokio_core::reactor::Core;
use tokio_io::io;
use test::Bencher;

const EXCHANGES: usize = 5;
const BUF_SIZE: usize = 64;
const PARALLEL: usize = 128;
const WARMUP: usize = 10;

/// Generates a socket address with the given port
fn addr(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port))
}

/// The client side
fn batter(port: u16) {
    let mut streams = (0..PARALLEL).map(|_| {
            TcpStream::connect(&addr(port))
                .unwrap()
        })
        .collect::<Vec<_>>();
    let input = vec![1u8; BUF_SIZE];
    let mut output = vec![0u8; BUF_SIZE];
    for _ in 0..EXCHANGES {
        for stream in &mut streams {
            stream.write_all(&input[..]).unwrap();
        }
        for stream in &mut streams {
            stream.read_exact(&mut output[..]).unwrap();
            assert_eq!(input, output);
        }
    }
}

/// Performs one benchmark, with the body as the server implementation
///
/// The server should create its listening socket on the given port and then synchronize through
/// the provided barrier ‒ so the client doesn't start connecting before the listener is ready.
///
/// There's a short warm-up before the actual benchmark starts ‒ both to initialize whatever
/// buffers or caches the library uses and to make sure the server already started after the
/// barrier.
fn bench<Body: FnOnce(Arc<Barrier>, u16) + Send + 'static>(b: &mut Bencher, port: u16, body: Body) {
    let barrier = Arc::new(Barrier::new(2));
    let barrier_copy = Arc::clone(&barrier);
    thread::spawn(move || body(barrier_copy, port));
    barrier.wait();
    for _ in 0..WARMUP {
        batter(port);
    }
    b.iter(|| batter(port));
}

/// Our own corona.
#[bench]
fn corona(b: &mut Bencher) {
    bench(b, 1234, |started, port| {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        let main = Coroutine::with_defaults(core.handle(), move || {
            let incoming = TokioTcpListener::bind(&addr(port), &handle)
                .unwrap()
                .incoming()
                .iter_ok();
            started.wait();
            for (mut connection, _address) in incoming {
                Coroutine::with_defaults(handle.clone(), move || {
                    let mut buf = [0u8; BUF_SIZE];
                    for _ in 0..EXCHANGES {
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
    });
}

/// Runs a fresh thread for each connection
///
/// This might happen to be slightly faster because it may use more CPU parallelism. Or maybe
/// there's just less overhead due to the event loop ‒ but there's the price to switch threads.
#[bench]
fn threads(b: &mut Bencher) {
    bench(b, 1235, |started, port| {
        let started = started; // Avoid clippy warning
        let listener = TcpListener::bind(&addr(port))
            .unwrap();
        started.wait();
        while let Ok((mut connection, _address)) = listener.accept() {
            thread::spawn(move || {
                let mut buf = [0u8; BUF_SIZE];
                for _ in 0..EXCHANGES {
                    connection.read_exact(&mut buf[..]).unwrap();
                    connection.write_all(&buf[..]).unwrap();
                }
            });
        }
    });
}

/// Just plain futures.
///
/// While faster than corona, it is probably harder to read and write.
#[bench]
fn futures(b: &mut Bencher) {
    bench(b, 1236, |started, port| {
        let mut core = Core::new().unwrap();
        let handle = core.handle();
        let main = TokioTcpListener::bind(&addr(port), &handle)
            .unwrap()
            .incoming()
            .for_each(move |(connection, _addr)| {
                let buf = vec![0u8; BUF_SIZE];
                let perform = stream::iter_ok(0..EXCHANGES)
                    .fold((connection, buf), |(connection, buf), _i| {
                        io::read_exact(connection, buf)
                            .and_then(|(connection, buf)| io::write_all(connection, buf))
                    })
                    .map(|_| ())
                    .map_err(|e: std::io::Error| panic!("{}", e));
                handle.spawn(perform);
                Ok(())
            });
        started.wait();
        core.run(main).unwrap();
    });
}
