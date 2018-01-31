//! This tests the pretention of blocking IO (Read/Write).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::thread;

use corona::prelude::*;
use corona::io::BlockingWrapper;
use tokio_core::net::TcpStream;
use tokio_core::reactor::{Core, Handle};

fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        while let Ok((connection, _address)) = listener.accept() {
            let mut connection = BufReader::new(connection);
            let mut line = String::new();
            while let Ok(size) = connection.read_line(&mut line) {
                if size == 0 {
                    break;
                }
                connection.get_mut().write_all(line.as_bytes()).unwrap();
                line.clear();
            }
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
#[test]
fn line_req_resp() {
    let mut core = Core::new().unwrap();
    let handle = core.handle();
    let addr = server();
    let done = Coroutine::with_defaults(handle.clone(), move || {
            client(&addr, &handle);
        });
    core.run(done).unwrap();
}
