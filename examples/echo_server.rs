//! A show-case of an echo server using coroutines.
//!
//! It listens on port 1234 and sends each line back. It handles multiple clients concurrently.

extern crate corona;
extern crate futures;
extern crate tokio_core;
extern crate tokio_io;

use std::io::BufReader;

use corona::Coroutine;
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_io::{io as aio, AsyncRead};

fn handle_connection(handle: &Handle, connection: TcpStream) {
    let (input, mut output) = connection.split();
    let input = BufReader::new(input);
    Coroutine::with_defaults(handle.clone(), move |await| {
        for line in await.stream(aio::lines(input)) {
            // If there's an error, kill the current coroutine. That one is not waited on and the
            // panic won't propagate. Logging it might be cleaner, but this demonstrates how the
            // coroutines act.
            let mut line = line.unwrap();
            line += "\n";
            // Send it back (the coroutine will yield until the data is written).
            let (o_tmp, _) = await.future(aio::write_all(output, line)).unwrap();
            output = o_tmp;
        }
        println!("A connection terminated");
    });
}

fn main() {
    // Set up of the listening socket
    let mut core = Core::new().unwrap();
    let listener = TcpListener::bind(&"[::]:1234".parse().unwrap(), &core.handle()).unwrap();
    let incoming = listener.incoming();

    let acceptor = Coroutine::with_defaults(core.handle(), move |await| {
        // This will accept the connections, but will allow other coroutines to run when there are
        // none ready.
        for attempt in await.stream(incoming) {
            match attempt {
                Ok((connection, address)) => {
                    println!("Received a connection from {}", address);
                    handle_connection(await.handle(), connection);
                },
                // FIXME: Are all the errors recoverable?
                Err(e) => eprintln!("An error accepting a connection: {}", e),
            }
        }
    });
    // Let the acceptor and everything else run.
    // Propagate all panics from the coroutine to the main thread with the unwrap
    core.run(acceptor).unwrap();
}
