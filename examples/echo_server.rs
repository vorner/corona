//! A show-case of an echo server using coroutines.
//!
//! It listens on port 1234 and sends each line back. It handles multiple clients concurrently.

extern crate corona;
extern crate tokio;

use std::io::BufReader;

use corona::prelude::*;
use tokio::net::{TcpListener, TcpStream};
use tokio::io as aio;
use tokio::io::AsyncRead;

fn handle_connection(connection: TcpStream) {
    let (input, mut output) = connection.split();
    let input = BufReader::new(input);
    corona::spawn(move || {
        for line in aio::lines(input).iter_result() {
            // If there's an error, kill the current coroutine. That one is not waited on and the
            // panic won't propagate. Logging it might be cleaner, but this demonstrates how the
            // coroutines act.
            let mut line = line.unwrap();
            line += "\n";
            // Send it back (the coroutine will yield until the data is written).
            let (o_tmp, _) = aio::write_all(output, line).coro_wait().unwrap();
            output = o_tmp;
        }
        println!("A connection terminated");
    });
}

fn main() {
    Coroutine::new().run(|| {
        // Set up of the listening socket
        let listener = TcpListener::bind(&"[::]:1234".parse().unwrap()).unwrap();
        for attempt in listener.incoming().iter_result() {
            match attempt {
                Ok(connection) => {
                    println!("Received a connection");
                    handle_connection(connection);
                },
                Err(e) => println!("An error accepting a connection: {}", e),
            }
        }
    }).unwrap();
}
