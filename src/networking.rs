use std::{
    fmt::{Display, Formatter},
    io::{BufWriter, ErrorKind, Read},
    io,
    net::{SocketAddr, TcpListener, TcpStream},
    sync::mpsc::{channel, Receiver, Sender, SendError, TryRecvError},
    thread::{JoinHandle, sleep},
    time::Duration,
};

use crate::{
    Direction::{Inbound, Outbound},
    Direction,
    networked_message::NetworkedMessage,
};
use crate::node::{Incoming, Outgoing};
use crate::spawn_with_name;

#[derive(Debug)]
pub struct Connection {
    addr: SocketAddr,
    thread: JoinHandle<io::Result<()>>,
    sender: Sender<NetworkedMessage>,
    direction: Direction,
}

impl Connection {
    fn send(&self, message: NetworkedMessage) -> Result<(), SendError<NetworkedMessage>> {
        self.sender.send(message)
    }
}

impl PartialEq for Connection {
    fn eq(&self, other: &Self) -> bool {
        self.addr == other.addr
    }
}

impl Display for Connection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.addr, self.direction)
    }
}

/// Handles sending and receiving messages, but leaves the business logic to the `node`.
pub struct Networking {
    connections: Vec<Connection>,
    sender: Sender<Incoming>,
    receiver: Receiver<Outgoing>,
    listener: TcpListener,
}

impl Networking {
    pub fn new(
        addr: SocketAddr,
        sender: Sender<Incoming>,
        receiver: Receiver<Outgoing>,
    ) -> Networking {
        let listener = TcpListener::bind(addr).unwrap();
        listener.set_nonblocking(true).unwrap();
        Networking { connections: vec![], sender, receiver, listener }
    }

    pub fn process(&mut self) {
        // Accept new connections
        loop {
            match self.accept() {
                Ok(addr) => self.sender.send(Incoming::ConnectionEstablished(addr)).unwrap(),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("Error when accepting listeners: {}", error),
            }
        }

        // Send queued messages
        loop {
            match self.receiver.try_recv() {
                Ok(Outgoing::Connect(addr)) => {
                    match self.connect(addr) {
                        Ok(addr) => self.sender.send(Incoming::ConnectionEstablished(addr)).unwrap(),
                        Err(error) => eprintln!("Failed to connect to {}: {}", addr, error)
                    }
                }
                Ok(Outgoing::Send(addr, message)) => self.send(addr, message),
                Ok(Outgoing::Forward(from, message)) => self.forward(from, message),
                Ok(Outgoing::Broadcast(message)) => self.broadcast(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    pub fn accept(&mut self) -> Result<SocketAddr, io::Error> {
        let (stream, _) = self.listener.accept()?;
        let incoming_message_sender = self.sender.clone();
        self.accept_connection(stream, incoming_message_sender, Inbound)
    }

    pub fn connect(&mut self, addr: SocketAddr) -> Result<SocketAddr, io::Error> {
        if self.is_connected(addr) { todo!() }

        eprintln!("Connecting to {}", addr);
        let sender = self.sender.clone();
        let stream = TcpStream::connect(addr)?;
        self.accept_connection(stream, sender, Outbound)
    }

    fn accept_connection(&mut self, stream: TcpStream, node_sender: Sender<Incoming>, direction: Direction)
                         -> Result<SocketAddr, io::Error> {
        stream.set_nonblocking(true)?;
        let remote_addr = stream.peer_addr()?;
        let local_addr = stream.local_addr()?;
        eprintln!("connection: them {}, us {}", remote_addr, local_addr);
        let (connection_sender, connection_receiver) = channel::<NetworkedMessage>();
        let connection_thread = spawn_with_name("connection", move || -> io::Result<()> {
            let mut writer = BufWriter::new(&stream);

            'main: loop {
                let mut buffer = vec![0; 1];
                'stream_events: loop {
                    match Read::read(&mut (&stream), &mut buffer) {
                        Ok(0) => {
                            // connection closed
                            break 'stream_events;
                        }
                        Ok(1) => {
                            let message = NetworkedMessage::receive(&stream, buffer[0]).unwrap();
                            node_sender.send(Incoming::Message(remote_addr, message)).unwrap();
                        }
                        Ok(count) => panic!("Read more bytes than the size of the buffer {}", count),
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            break 'stream_events;
                        }
                        Err(error) => panic!("error reading {:?}", error),
                    }
                }

                'channel_events: loop {
                    match connection_receiver.try_recv() {
                        Ok(message) => {
                            if let Err(error) = message.send(&mut writer) {
                                eprintln!("Connection closed {}->{}: {}",
                                          local_addr, remote_addr, error);
                                break 'main;
                            }
                        }
                        Err(TryRecvError::Empty) => break 'channel_events,
                        Err(TryRecvError::Disconnected) => {
                            eprintln!("Quitting connection thread {}->{}", local_addr, remote_addr);
                            break 'main;
                        }
                    }
                }

                // TODO: Replace with something smarter and OS dependant
                sleep(Duration::from_millis(10));
            }

            Ok(())
        });

        self.connections.push(Connection {
            addr: remote_addr,
            thread: connection_thread,
            sender: connection_sender,
            direction,
        });
        Ok(remote_addr)
    }

    pub fn is_connected(&self, addr: SocketAddr) -> bool {
        return self.connections.iter().any(|c| c.addr == addr);
    }

    /// Sends a message to a single connection
    pub fn send(&mut self, to: SocketAddr, message: NetworkedMessage) {
        let sender = self.sender.clone();
        self.connections.retain(|connection| {
            if connection.addr != to { return true; }

            if let Err(error) = connection.send(message.clone()) {
                eprintln!("Connection closed {}: {}", connection.addr, error);
                sender.send(Incoming::ConnectionClosed(connection.addr)).unwrap();
                false
            } else {
                true
            }
        });
    }

    /// Sends a message to all connections, except the original sender
    pub fn forward(&mut self, except: SocketAddr, message: NetworkedMessage) {
        let sender = self.sender.clone();
        self.connections.retain(|connection| {
            if connection.addr == except { return true; }

            if let Err(error) = connection.send(message.clone()) {
                eprintln!("Connection closed {}: {}", connection.addr, error);
                sender.send(Incoming::ConnectionClosed(connection.addr)).unwrap();
                false
            } else {
                true
            }
        });
    }

    /// Sends a message to all connections
    pub fn broadcast(&mut self, message: NetworkedMessage) {
        let sender = self.sender.clone();
        self.connections.retain(|connection| {
            if let Err(error) = connection.send(message.clone()) {
                eprintln!("Connection closed {}: {}", connection.addr, error);
                sender.send(Incoming::ConnectionClosed(connection.addr)).unwrap();
                false
            } else {
                true
            }
        });
    }

    pub fn close(&mut self) {
        // todo!()
        // let threads: Vec<JoinHandle<io::Result<()>>> = self.connections.into_iter().map(|c| c.thread).collect();
        // self.connections.clear();
        // for thread in threads { thread.join().unwrap(); }
    }
}



