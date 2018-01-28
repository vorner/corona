//! This tests the pretention of blocking IO (Read/Write).

use std::io::{BufRead, BufReader, Write};

use corona::prelude::*;
use corona::io::IoWrapper;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;

const ADDR: &str = "127.0.0.1:2345";

fn server(handle: Handle) {
    let listener = TcpListener::bind(&ADDR.parse().unwrap(), &handle).unwrap();
    Coroutine::with_defaults(handle.clone(), move || {
        for (connection, _address) in listener.incoming().iter_ok() {
            Coroutine::with_defaults(handle.clone(), move || {
                let (read, write) = connection.split();
                let read = BufReader::new(IoWrapper::new(read));
                let mut write = IoWrapper::new(write);
                for line in read.lines() {
                    let mut line = line.unwrap();
                    line.push('\n');
                    write.write_all(line.as_bytes()).unwrap();
                }
            });
        }
    });
}

fn client(handle: &Handle) {
    let connection = TcpStream::connect(&ADDR.parse().unwrap(), handle)
        .coro_wait()
        .unwrap();
    let mut connection = IoWrapper::new(connection);
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
    server(handle.clone());
    let done = Coroutine::with_defaults(handle.clone(), move || {
        client(&handle);
    });
    core.run(done).unwrap();
}
