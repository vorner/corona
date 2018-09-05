//! This tests the pretention of blocking IO (Read/Write).

use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;

use corona::prelude::*;
use corona::io::BlockingWrapper;
use tokio::runtime::current_thread::Runtime;
use tokio::net::{TcpListener, TcpStream};
use tokio::prelude::*;

fn server() -> SocketAddr {
    let listener = TcpListener::bind(&"127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();
    Coroutine::with_defaults(move || {
        for connection in listener.incoming().iter_ok() {
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

fn client(addr: &SocketAddr) {
    let connection = TcpStream::connect(addr)
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
    let mut rt = Runtime::new().unwrap();
    rt.block_on(future::lazy(|| {
        let addr = server();
        Coroutine::with_defaults(move || {
            client(&addr);
        })
    })).unwrap();
}
