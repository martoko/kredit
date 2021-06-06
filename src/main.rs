#![allow(unused_imports)]

use std::net::{TcpListener, TcpStream, SocketAddr};
use std::str::FromStr;
use std::sync::mpsc::{Sender, Receiver, channel, TryRecvError, SendError};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::{spawn, sleep, JoinHandle};
use std::io::{Write, BufRead, BufReader, BufWriter, Read, stdin, stdout, ErrorKind};
use std::error::Error;
use sha2::{Sha512, Digest, Sha256};
use hex::ToHex;
use std::env::args;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{time, fmt, io};
use sha2::digest::generic_array::GenericArray;
use sha2::digest::consts::{U64};
use std::convert::{TryInto, TryFrom};
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, Ordering};
use std::process::exit;
use std::collections::HashMap;

#[derive(Debug, Copy, Clone)]
struct Block {
    parent_hash: [u8; 32],
    miner_address: [u8; 32],
    nonce: u64,
    difficulty: u8,
    time: u64,
}

impl Block {
    fn hash(&self) -> [u8; 32] {
        let mut sha256 = Sha256::new();
        sha256.update(self.parent_hash);
        sha256.update(self.miner_address);
        sha256.update(self.nonce.to_le_bytes());
        sha256.update(self.difficulty.to_le_bytes());
        sha256.update(self.time.to_le_bytes());
        sha256.finalize().into()
    }

    fn deserialize(reader: &mut dyn Read) -> io::Result<Block> {
        let mut buffer = [0; 32 + 32 + 8 + 1 + 8];
        let mut bytes_read = 0;
        while bytes_read < buffer.len() {
            match reader.read(&mut buffer[bytes_read..]) {
                Ok(x) => bytes_read += x,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    eprintln!("Have read this so far {:?}", buffer);
                    eprintln!("Would block, waiting...");
                    // TODO: Replace with something smarter and OS dependant
                    sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e),
            };
        }

        Ok(Block {
            parent_hash: buffer[0..32].try_into().unwrap(),
            miner_address: buffer[32..64].try_into().unwrap(),
            nonce: u64::from_le_bytes(buffer[64..72].try_into().unwrap()),
            difficulty: buffer[72].try_into().unwrap(),
            time: u64::from_le_bytes(buffer[73..81].try_into().unwrap()),
        })
    }

    fn serialize(&self, writer: &mut dyn Write) {
        let mut buffer = vec![0; 0];
        for byte in self.parent_hash { buffer.push(byte); }
        for byte in self.miner_address { buffer.push(byte); }
        for byte in self.nonce.to_le_bytes() { buffer.push(byte); }
        for byte in self.difficulty.to_le_bytes() { buffer.push(byte); }
        for byte in self.time.to_le_bytes() { buffer.push(byte); }
        writer.write_all(&buffer).unwrap();
    }
}

impl fmt::Display for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{\n\
        hash: {},\n\
        parent_hash: {},\n\
        miner_address: {},\n\
        nonce: {},\n\
        time: {},\n\
        difficulty: {}\n\
        }}",
               hex::encode(self.hash()),
               hex::encode(self.parent_hash),
               hex::encode(self.miner_address),
               hex::encode(self.nonce.to_le_bytes()),
               self.time,
               self.difficulty)
    }
}

#[derive(Debug, Copy, Clone)]
enum MinerMessage {
    Start(Block),
    Pause,
    Quit,
}

#[derive(Debug, Copy, Clone)]
struct NewBlock {
    block: Block,
    from: Option<SocketAddr>,
}

#[derive(Debug, Copy, Clone)]
enum ConnectionMessage {
    Quit,
    SendPing,
    NewBlock(NewBlock),
}

#[derive(Debug, Copy, Clone)]
enum NetworkedMessageType {
    Ping,
    Pong,
    Block,
}

impl TryFrom<u8> for NetworkedMessageType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            x if x == NetworkedMessageType::Ping as u8 => Ok(NetworkedMessageType::Ping),
            x if x == NetworkedMessageType::Pong as u8 => Ok(NetworkedMessageType::Pong),
            x if x == NetworkedMessageType::Block as u8 => Ok(NetworkedMessageType::Block),
            _ => Err(()),
        }
    }
}

fn difficulty(hash: [u8; 32]) -> u8 {
    let mut trailing_zeros = 0;
    for i in (0..32).rev() {
        if hash[i] == 0 {
            trailing_zeros += 1;
        } else {
            break;
        }
    }

    trailing_zeros
}

fn accept_connection(stream: TcpStream, main_sender: Sender<NewBlock>)
                     -> (JoinHandle<io::Result<()>>, Sender<ConnectionMessage>) {
    stream.set_nonblocking(true).unwrap();
    let peer_addr = stream.peer_addr().unwrap();
    let local_addr = stream.local_addr().unwrap();
    println!("connection: them {}, us {}", peer_addr, local_addr);
    let (connection_sender, connection_receiver) = channel();
    let connection_thread = spawn(move || -> io::Result<()> {
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
                        match buffer[0].try_into() {
                            Ok(NetworkedMessageType::Ping) => {
                                eprintln!("Ping!");
                                writer.write(&[NetworkedMessageType::Pong as u8]).unwrap();
                                writer.flush().unwrap();
                            }
                            Ok(NetworkedMessageType::Pong) => {
                                eprintln!("Pong!");
                            }
                            Ok(NetworkedMessageType::Block) => {
                                eprintln!("Parsing block...");
                                let block = Block::deserialize(&mut (&stream)).unwrap();
                                main_sender.send(NewBlock { block, from: Some(peer_addr) }).unwrap();
                            }
                            Err(()) => {
                                eprintln!("Received invalid message type {}", buffer[0]);
                                break 'main;
                            }
                        }
                    }
                    Ok(count) => panic!("Impossible amount of bytes read {}", count),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        break 'stream_events;
                    }
                    Err(error) => panic!("error reading {:?}", error),
                }
            }

            'channel_events: loop {
                match connection_receiver.try_recv() {
                    Ok(ConnectionMessage::Quit) => {
                        eprintln!("Quitting connection thread {}->{}", local_addr, peer_addr);
                        break 'main;
                    }
                    Ok(ConnectionMessage::SendPing) => {
                        writer.write(&[NetworkedMessageType::Ping as u8]).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
                        }
                    }
                    Ok(ConnectionMessage::NewBlock(NewBlock { block, from })) => {
                        let should_skip = from.map(|f| f == peer_addr).unwrap_or(false);
                        if !should_skip {
                            writer.write(&[NetworkedMessageType::Block as u8]).unwrap();
                            block.serialize(&mut writer);
                            if let Err(err) = writer.flush() {
                                eprintln!("Connection closed {}->{}: {}",
                                          local_addr, peer_addr, err);
                                break 'main;
                            }
                        }
                    }
                    Err(e) if e == TryRecvError::Empty => {
                        break 'channel_events;
                    }
                    Err(e) => panic!("{}", e),
                }
            }

            // TODO: Replace with something smarter and OS dependant
            sleep(Duration::from_millis(10));
        }

        Ok(())
    });

    (connection_thread, connection_sender)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = args().collect();
    let mut should_mine = args.contains(&"--mine".to_string()) || args.contains(&"-m".to_string());
    let listen_addr_str = args.get(1).expect("You must supply a listen addr as arg1");
    let listener = TcpListener::bind(listen_addr_str).unwrap();
    listener.set_nonblocking(true).unwrap();

    let (main_sender, main_receiver) = channel();

    let (miner_jobs_sender, miner_jobs_receiver) = channel();
    let miner_main_sender = main_sender.clone();
    let miner_thread = spawn(move || -> io::Result<()> {
        let mut start_time = SystemTime::now();
        let mut job_block = None;

        loop {
            match miner_jobs_receiver.try_recv() {
                Ok(MinerMessage::Start(parent_block)) => {
                    start_time = SystemTime::now();
                    job_block = Some(Block {
                        miner_address: [0; 32],
                        parent_hash: parent_block.hash(),
                        nonce: 0,
                        time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                        difficulty: parent_block.difficulty,
                    });
                }
                Ok(MinerMessage::Pause) => job_block = None,
                Ok(MinerMessage::Quit) => break,
                Err(TryRecvError::Empty) => {
                    if let Some(mut block) = job_block {
                        if let Some(nonce) = block.nonce.checked_add(1) {
                            block.nonce = nonce;
                        } else {
                            // We ran out of nonce, bump the time and restart
                            block.time = SystemTime::now().duration_since(UNIX_EPOCH)
                                .unwrap().as_secs();
                        }
                        job_block = Some(block);

                        if difficulty(block.hash()) >= block.difficulty {
                            let duration = start_time.elapsed().unwrap();
                            // Artificially make the minimum mining time 5 seconds
                            // if let Some(duration) = Duration::from_secs(2).checked_sub(duration) {
                            //     sleep(duration);
                            // }
                            let seconds = duration.as_secs() % 60;
                            let minutes = (duration.as_secs() / 60) % 60;
                            let hours = (duration.as_secs() / 60) / 60;
                            if hours > 0 {
                                eprintln!("Mining took {} hour(s) {} minute(s) {} second(s)",
                                          hours, minutes, seconds);
                            } else if minutes > 0 {
                                eprintln!("Mining took {} minute(s) {} second(s)", minutes, seconds);
                            } else {
                                eprintln!("Mining took {} second(s)", seconds);
                            }

                            miner_main_sender.send(NewBlock { block, from: None }).unwrap();
                            job_block = None;
                        }
                    } else {
                        // TODO: Replace with something smarter and OS dependant
                        sleep(Duration::from_millis(10));
                    }
                }
                Err(e) => panic!("{:?}", e),
            }
        }

        Ok(())
    });

    let (server_sender, server_receiver) = channel();
    let server_main_sender = main_sender.clone();
    let server_thread = spawn(move || -> io::Result<()> {
        let mut connection_threads = Vec::new();

        // Make outbound connections
        &args.get(2).map(|connect_addr_str| {
            eprintln!("Connecting to {}", connect_addr_str);
            let main_sender = server_main_sender.clone();
            match TcpStream::connect(connect_addr_str) {
                Ok(stream) => connection_threads.push(accept_connection(stream, main_sender)),
                Err(error) => eprintln!("Failed to connect to {}: {}", connect_addr_str, error)
            };
        });

        // Accept new inbound connections
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let main_sender = server_main_sender.clone();
                    connection_threads.push(accept_connection(stream, main_sender));
                }
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {
                    match server_receiver.try_recv() {
                        Ok(ConnectionMessage::Quit) => {
                            connection_threads.retain(|(_, connection_sender)| {
                                if let Err(_) = connection_sender.send(ConnectionMessage::Quit) {
                                    false
                                } else {
                                    true
                                }
                            });

                            while let Some((connection_thread, _)) = connection_threads.pop() {
                                connection_thread.join().unwrap().unwrap();
                            }
                            eprintln!("Quitting server thread {}", listener.local_addr().unwrap());
                            break;
                        }
                        Ok(ConnectionMessage::NewBlock(new_block)) => {
                            connection_threads.retain(|(_, connection_sender)| {
                                if let Err(_) = connection_sender.send(ConnectionMessage::NewBlock(new_block)) {
                                    false
                                } else {
                                    true
                                }
                            });
                        }
                        Ok(ConnectionMessage::SendPing) => {
                            connection_threads.retain(|(_, connection_sender)| {
                                if let Err(_) = connection_sender.send(ConnectionMessage::SendPing) {
                                    false
                                } else {
                                    true
                                }
                            });
                        }
                        Err(_) => {
                            // TODO: Replace with something smarter and OS dependant
                            sleep(Duration::from_millis(10));
                        }
                    }
                }
                Err(error) => eprintln!("Error when accepting listeners: {}", error),
            }
        }

        Ok(())
    });

    let quit_requested = Arc::new(AtomicBool::new(false));
    {
        let quit_requested = quit_requested.clone();
        ctrlc::set_handler(move || {
            if quit_requested.load(Ordering::SeqCst) {
                eprintln!("\rExiting forcefully");
                exit(1);
            } else {
                eprintln!("\rShutdown requested");
                quit_requested.store(true, Ordering::SeqCst);
            }
        }).unwrap();
    }

    let seed_block = Block {
        miner_address: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        parent_hash: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        nonce: 0,
        time: 1622999578, // SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        difficulty: 2, // 2 gives results in 0-5 seconds, 3 gives results in 3-10 minutes
    };


    let mut blocks = Vec::new();
    blocks.push(seed_block);

    eprintln!("Seed block {}", blocks.last().unwrap());

    if should_mine {
        miner_jobs_sender.send(MinerMessage::Start(blocks.last().unwrap().clone())).unwrap();
    }

    let mut terminal_input = true;
    let mut command = String::new();
    loop {
        if quit_requested.load(Ordering::SeqCst) {
            server_sender.send(ConnectionMessage::Quit).unwrap();
            miner_jobs_sender.send(MinerMessage::Quit).unwrap();
            server_thread.join().unwrap().unwrap();
            miner_thread.join().unwrap().unwrap();
            break;
        }

        if terminal_input {
            match crossterm::event::poll(Duration::from_secs(0)) {
                Ok(true) => {
                    // It's guaranteed that read() wont block if `poll` returns `Ok(true)`
                    if let crossterm::event::Event::Key(crossterm::event::KeyEvent { code, .. })
                    = crossterm::event::read().unwrap() {
                        match code {
                            crossterm::event::KeyCode::Enter => {
                                if command == "ping" || command == "p" {
                                    server_sender.send(ConnectionMessage::SendPing).unwrap();
                                } else if command == "quit" || command == "q" {
                                    quit_requested.store(true, Ordering::SeqCst);
                                } else if command == "mine" || command == "m" {
                                    miner_jobs_sender.send(MinerMessage::Start(blocks.last().unwrap().clone())).unwrap();
                                } else if command == "pause" || command == "p" {
                                    miner_jobs_sender.send(MinerMessage::Pause).unwrap();
                                    should_mine = false;
                                }
                                command.clear();
                            }
                            crossterm::event::KeyCode::Char(c) => {
                                command.push(c);
                            }
                            _ => {}
                        }
                    }
                }
                Ok(false) => {
                    // TODO: Replace with something smarter and OS dependant
                    sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    eprintln!("Failed to read from terminal, commands disabled: {:?}", error);
                    terminal_input = false;
                }
            }
        }

        loop {
            match main_receiver.try_recv() {
                Ok(NewBlock { block, from }) => {
                    let parent = blocks.last().unwrap();
                    if block.parent_hash == parent.hash() &&
                        block.difficulty == parent.difficulty {
                        blocks.push(block);
                        eprintln!("Inserting new block {}\nBlock height: {}", block, blocks.len());
                        server_sender.send(ConnectionMessage::NewBlock(NewBlock { block, from })).unwrap();

                        if should_mine {
                            miner_jobs_sender.send(MinerMessage::Start(blocks.last().unwrap().clone())).unwrap();
                        }
                    } else {
                        eprintln!("Discarding invalid block {}", block);
                    }
                }
                Err(_) => break,
            }
        }

        // TODO: Replace with something smarter and OS dependant
        sleep(Duration::from_millis(10));
    }

    Ok(())
}
