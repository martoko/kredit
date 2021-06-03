#![allow(dead_code, unused_imports, unused_variables)]

use std::net::{TcpListener, TcpStream, SocketAddr};
use std::str::FromStr;
use std::sync::mpsc::{Sender, Receiver, channel, TryRecvError};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::{spawn, sleep};
use std::io::{Write, BufRead, BufReader, BufWriter, Read, stdin, stdout, ErrorKind};
use std::error::Error;
use sha2::{Sha512, Digest, Sha256};
use hex::ToHex;
use std::env::args;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{time, fmt};
use sha2::digest::generic_array::GenericArray;
use sha2::digest::consts::{U64};
use std::convert::TryInto;
use std::fmt::{Display, Formatter};

fn hashing() {
    let a = Block {
        miner_address: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        parent_hash: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        nonce: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        parent: None,
    };
    sleep(Duration::from_secs(1));
    let b = Block {
        miner_address: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        parent_hash: a.hash(),
        nonce: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        parent: Some(&a),
    };
    eprintln!("{}", hex::encode(a.hash()));
    eprintln!("{}", a);
    eprintln!("{}", b);
}

struct Block<'a> {
    parent_hash: [u8; 32],
    miner_address: [u8; 32],
    nonce: [u8; 32],
    time: u64,

    parent: Option<&'a Block<'a>>,
}

impl<'a> Block<'a> {
    fn hash(&self) -> [u8; 32] {
        let mut sha256 = Sha256::new();
        sha256.update(self.parent_hash);
        sha256.update(self.miner_address);
        sha256.update(self.nonce);

        sha256.finalize().into()
    }
}

impl<'a> fmt::Display for Block<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {}, {}, {})",
                hex::encode(self.hash()),
               hex::encode(self.parent_hash),
               hex::encode(self.miner_address),
               hex::encode(self.nonce),
               self.time)
    }
}

enum MessageType {
    Quit,
    SendChat(String),
}


fn main() -> std::io::Result<()> {
    hashing();

    let args: Vec<String> = args().collect();
    let listen_addr_str = args.get(1).expect("You must supply a listen addr as arg1");
    let listener = TcpListener::bind(
        SocketAddr::from_str(listen_addr_str).expect("Invalid address")
    ).unwrap();
    listener.set_nonblocking(true).unwrap();

    // let (incoming_messages_sender, incoming_messages_receiver) = channel();

    let (server_sender, server_receiver) = channel();
    let server_thread = spawn(move || {
        let mut connection_threads = Vec::new();

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    stream.set_nonblocking(true).unwrap();
                    println!(
                        "connection from {} to {}",
                        stream.peer_addr().unwrap(),
                        stream.local_addr().unwrap()
                    );
                    let (connection_sender, connection_receiver) = channel();
                    let connection_thread = spawn(move || {
                        let mut reader = BufReader::new(&stream);
                        let mut writer = BufWriter::new(&stream);

                        writer.write_fmt(format_args!("Hello, {}!\n", stream.peer_addr().unwrap()))
                            .unwrap();
                        writer.flush().unwrap();

                        loop {
                            match connection_receiver.try_recv() {
                                Ok(MessageType::Quit) => {
                                    println!("Quitting connection thread");
                                    break;
                                }
                                Ok(MessageType::SendChat(text)) => {
                                    writer.write_fmt(format_args!(
                                        "[{}] {}\n",
                                        stream.peer_addr().unwrap(),
                                        text
                                    )).unwrap();
                                    writer.flush().unwrap();
                                }
                                Err(e) if e == TryRecvError::Empty => {
                                    // TODO: Replace with something smarter
                                    sleep(Duration::from_millis(10));
                                }
                                Err(e) => panic!("{}", e),
                            }
                        }
                    });
                    connection_threads.push((connection_thread, connection_sender));
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                    match server_receiver.try_recv() {
                        Ok(MessageType::Quit) => {
                            for (_, connection_sender) in connection_threads.iter() {
                                connection_sender.send(MessageType::Quit).unwrap();
                            }

                            while let Some((connection_thread, _)) = connection_threads.pop() {
                                connection_thread.join().unwrap();
                            }
                            println!("Quitting server thread");
                            break;
                        }
                        Ok(MessageType::SendChat(text)) => {
                            for (_, connection_sender) in connection_threads.iter() {
                                connection_sender.send(MessageType::SendChat(text.clone())).unwrap();
                            }
                        }
                        Err(_) => {
                            // TODO: Replace with something smarter
                            sleep(Duration::from_millis(10));
                        }
                    }
                }
                Err(error) => eprintln!("Error when accepting listeners: {}", error),
            }
        }
    });

    let (client_sender, client_receiver): (Sender<MessageType>, Receiver<MessageType>) = channel();
    let connect_addr_str = &args.get(2);
    match connect_addr_str {
        Some(connect_addr_str) => {
            println!("Connecting to {}", connect_addr_str);
        }
        None => {
            println!("Quitting client");
        }
    }
    let client_thread = spawn(move || {});

    loop {
        print!("> ");
        stdout().flush().unwrap();
        let mut input = String::new();
        stdin().read_line(&mut input).unwrap();
        let input = input.trim();
        if input == "q" {
            server_sender.send(MessageType::SendChat("Bye".to_string())).unwrap();
            println!("Quitting...");
            server_sender.send(MessageType::Quit).unwrap();
            break;
        } else {
            println!("[you] {}", input);
            server_sender.send(MessageType::SendChat(input.to_string())).unwrap();
        }
    }

    println!("Waiting for client");
    client_thread.join().unwrap();
    println!("Waiting for server");
    server_thread.join().unwrap();

    Ok(())
}
