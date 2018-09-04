//! This tests the pretention of blocking IO (Read/Write).

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;

use corona::prelude::*;
use corona::io::BlockingWrapper;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;

fn server(handle: Handle) -> SocketAddr {
    let listener = TcpListener::bind(&"127.0.0.1:0".parse().unwrap(), &handle).unwrap();
    let addr = listener.local_addr().unwrap();
    Coroutine::with_defaults(move || {
        for (connection, _address) in listener.incoming().iter_ok() {
            Coroutine::with_defaults(move || {
                let (read, write) = connection.split();
                let read = BufReader::new(BlockingWrapper::new(read));
                let mut write = BlockingWrapper::new(write);
                for line in read.lines() {
                    let mut line = line.unwrap();
                    line.push('\n');
                    write.write_all(line.as_bytes()).unwrap();
                    write.flush().unwrap();
                }
            });
        }
    });
    addr
}

fn client(addr: &SocketAddr, handle: &Handle) {
    let connection = TcpStream::connect(addr, handle)
        .coro_wait()
        .unwrap();
    let mut connection = BlockingWrapper::new(connection);
    connection.write_all(b"hello\n").unwrap();
    let mut answer = String::new();
    BufReader::new(connection).read_line(&mut answer).unwrap();
    assert_eq!("hello\n", answer);
}

/// Runs both the client and server on the same thread, so we can be sure they can switch.
///
/// Both use the „normal“ blocking API, but switch coroutines under the hood.
///
/// The test runs a listener, makes a single connection and exchanges a line there and back.
#[test]
fn line_req_resp() {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let addr = server(handle.clone());
    let done = Coroutine::with_defaults(move || {
            client(&addr, &handle);
        });
    core.run(done).unwrap();
}
