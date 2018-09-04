//! A show-case of a chat server.
//!
//! The server listens on the port 1234 and accepts connections. Whenever a line of text comes, it
//! is broadcasted to all the clients (including back).
//!
//! There are two long-running coroutines. One accepts the new connections and spawns a receiving
//! coroutine for each. The other is on a receiving end of a channel and broadcasts each message to
//! all the currently available clients. If some of them errors during the send, it is removed.
//!
//! Each receiving coroutine simply reads the lines from the client and stuffs them into the
//! channel.
//!
//! There's a shared `Vec` of new writing halves of the connections. Before every message, the
//! broadcasting coroutine extracts the new ones and appends them to its local storage (so it
//! doesn't have to keep the shared state borrowed).
//!
//! Currently, there's very little error handling ‒ the relevant connections are simply dropped.

extern crate bytes;
extern crate corona;
extern crate futures;
extern crate tokio_core;
extern crate tokio_io;

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Error as IoError};
use std::iter;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use bytes::BytesMut;
use corona::Coroutine;
use corona::io::BlockingWrapper;
use corona::prelude::*;
use corona::wrappers::SinkSender;
use futures::{future, Future};
use futures::unsync::mpsc::{self, Sender, Receiver};
use tokio_core::net::{TcpListener, TcpStream};
use tokio_core::reactor::{Core, Handle};
use tokio_io::AsyncRead;
use tokio_io::io::WriteHalf;
use tokio_io::codec::{Encoder, FramedWrite};

/// Encoder turning strings into lines.
///
/// Doesn't do much, simply passes the strings as lines. For a convenient use of `SinkSender` (the
/// thing that is usually behind `coro_send`, but doesn't wait on the send, is only the future) on
/// the senders.
struct LineEncoder;

impl Encoder for LineEncoder {
    type Item = Rc<String>;
    type Error = IoError;
    fn encode(&mut self, item: Rc<String>, dst: &mut BytesMut) -> Result<(), IoError> {
        dst.extend_from_slice(item.as_bytes());
        dst.extend_from_slice(b"\n");
        Ok(())
    }
}

type Client = FramedWrite<WriteHalf<TcpStream>, LineEncoder>;
type Clients = Rc<RefCell<Vec<Client>>>;

fn handle_connection(connection: TcpStream,
                     clients: &Clients,
                     mut msgs: Sender<String>)
{
    let (input, output) = connection.split();
    let writer = FramedWrite::new(output, LineEncoder);
    clients.borrow_mut().push(writer);
    let input = BufReader::new(BlockingWrapper::new(input));
    Coroutine::new().stack_size(32_768).spawn_catch_panic(AssertUnwindSafe(move || {
        // If there's an error, kill the current coroutine. That one is not waited on and the
        // panic won't propagate. Logging it might be cleaner, but this demonstrates how the
        // coroutines act.
        for line in input.lines() {
            let line = line.expect("Broken line on input");
            // Pass each line to the broadcaster so it sends it to everyone.
            // Send it back (the coroutine will yield until the data is written). May block on
            // being full for a while, then we don't accept more messages.
            msgs.coro_send(line).expect("The broadcaster suddenly disappeared");
        }
        eprintln!("A connection terminated");
    })).expect("Wrong stack size");
}

fn broadcaster(msgs: Receiver<String>, clients: &Clients) {
    // We have to steal the clients. We can't keep a mut borrow into the clients for the time of
    // the future, since someone else might try to add more at the same time, which would panic.
    let mut extracted = Vec::new();
    for msg in msgs.iter_ok() {
        { // Steal the clients and return the borrow
            let mut borrowed = clients.borrow_mut();
            extracted.extend(borrowed.drain(..));
        }
        let broken_idxs = {
            let msg = Rc::new(msg);
            // Schedule sending of the message to everyone in parallel
            let all_sent = extracted.iter_mut()
                .map(|client| SinkSender::new(client, iter::once(Rc::clone(&msg))))
                // Turn failures into falses, so it plays nice with collect below.
                .map(|send_future| send_future.then(|res| Ok::<_, IoError>(res.is_ok())));
            future::join_all(all_sent) // Create a mega-future of everything
                .coro_wait() // Wait for them
                .unwrap() // Impossible to fail
                // Take only the indices of things that failed to send.
                .into_iter()
                .enumerate()
                .filter_map(|(idx, success)| if success {
                        None
                    } else {
                        Some(idx)
                    })
                .collect::<Vec<_>>()
        };
        // Remove the failing ones. We go from the back, since swap_remove reorders the tail.
        for idx in broken_idxs.into_iter().rev() {
            extracted.swap_remove(idx);
        }
    }
}

fn acceptor(handle: &Handle, clients: &Clients, sender: &Sender<String>) {
    let listener = TcpListener::bind(&"[::]:1234".parse().unwrap(), handle).unwrap();
    let incoming = listener.incoming();
    // This will accept the connections, but will allow other coroutines to run when there are
    // none ready.
    for attempt in incoming.iter_result() {
        match attempt {
            Ok((connection, address)) => {
                eprintln!("Received a connection from {}", address);
                handle_connection(connection, clients, sender.clone());
            },
            // FIXME: Are all the errors recoverable?
            Err(e) => eprintln!("An error accepting a connection: {}", e),
        }
    }
}

fn main() {
    // Set up of the listening socket
    let mut core = Core::new().unwrap();
    let handle = core.handle();

    let (sender, receiver) = mpsc::channel(100);
    let clients = Clients::default();
    let clients_rc = Rc::clone(&clients);
    let broadcaster = Coroutine::with_defaults(move || {
        broadcaster(receiver, &clients_rc)
    });
    let acceptor = Coroutine::with_defaults(move || {
        acceptor(&handle, &clients, &sender)
    });
    // Let the acceptor and everything else run.
    // Propagate all panics from the coroutine to the main thread with the unwrap
    core.run(broadcaster.join(acceptor)).unwrap();
}
