use std::{array, io};
use std::convert::TryInto;
use std::env::args;
use std::io::{BufWriter, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::exit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blockchain::Block;

use crate::blockchain::Blockchain;

mod blockchain;

#[derive(Debug, Copy, Clone)]
enum ToMiner {
    Quit,
    Start(Block),
    Pause,
}

#[derive(Debug, Copy, Clone)]
enum ToPeer {
    Quit,
    Send(NetworkedMessage),
}

#[derive(Debug, Clone)]
enum ToNode {
    Quit,
    Received(NetworkedMessage, SocketAddr),
    Mined(Block),
    ConnectionEstablished(SocketAddr),
}

#[derive(Debug)]
enum DeserializeError {
    Io(io::Error),
    TryFromSlice(array::TryFromSliceError),
}

impl From<io::Error> for DeserializeError {
    fn from(e: io::Error) -> Self { DeserializeError::Io(e) }
}

impl From<array::TryFromSliceError> for DeserializeError {
    fn from(e: array::TryFromSliceError) -> Self { DeserializeError::TryFromSlice(e) }
}

#[derive(Debug, Copy, Clone)]
enum NetworkedMessage {
    Block(Block),
    Ping,
    Pong,
    TopBlockHash([u8; 32]),
}

impl NetworkedMessage {
    fn receive(stream: &TcpStream, r#type: u8) -> Result<NetworkedMessage, DeserializeError> {
        match r#type {
            0 => Ok(NetworkedMessage::Ping),
            1 => Ok(NetworkedMessage::Pong),
            2 => {
                let mut buffer = [0; 32 + 32 + 8 + 1 + 8];
                stream.set_nonblocking(false)?;
                // stream.read_exact(&mut buffer)?
                (&mut (&*stream)).read(&mut buffer)?;
                stream.set_nonblocking(true)?;

                Ok(NetworkedMessage::Block(Block {
                    parent_hash: buffer[0..32].try_into()?,
                    miner_address: buffer[32..64].try_into()?,
                    nonce: u64::from_le_bytes(buffer[64..72].try_into()?),
                    difficulty: buffer[72],
                    time: u64::from_le_bytes(buffer[73..81].try_into()?),
                }))
            }
            3 => {
                let mut buffer = [0; 32];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read(&mut buffer)?;
                stream.set_nonblocking(true)?;
                Ok(NetworkedMessage::TopBlockHash(buffer))
            }
            _ => Err(DeserializeError::from(io::Error::new(
                ErrorKind::InvalidData, format!("Invalid message type: {}", r#type), // TODO: Bad error style
            )))
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

// TODO: Introduce phases
// Phase 1, seed peers
//   For now just rely on the user specifying exact IP's
//   In the future the nodes should exchange peers with each other
// Phase 2, synchronize blockchain
//   Maybe just choose a node and ask it to send a full history
// Phase 3, maintain the blockchain & mine
//
// Another interesting task: Persisting the blockchain on shutdown, maybe also some peers?

// TODO: Exchange peers
// TODO: DAG block chain, to resolve consensus problems
// TODO: File-backed blockchain?

fn accept_connection(stream: TcpStream, node_sender: Sender<ToNode>)
                     -> (SocketAddr, JoinHandle<io::Result<()>>, Sender<ToPeer>) {
    stream.set_nonblocking(true).unwrap();
    let peer_addr = stream.peer_addr().unwrap();
    let local_addr = stream.local_addr().unwrap();
    eprintln!("connection: them {}, us {}", peer_addr, local_addr);
    let (connection_sender, connection_receiver) = channel();
    let connection_thread = spawn(move || -> io::Result<()> {
        let mut writer = BufWriter::new(&stream);

        node_sender.send(ToNode::ConnectionEstablished(peer_addr)).unwrap();

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
                        node_sender.send(ToNode::Received(message, peer_addr)).unwrap();
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
                    Ok(ToPeer::Quit) => {
                        eprintln!("Quitting connection thread {}->{}", local_addr, peer_addr);
                        break 'main;
                    }
                    Ok(ToPeer::Send(NetworkedMessage::Ping)) => {
                        writer.write(&[0]).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
                        }
                    }
                    Ok(ToPeer::Send(NetworkedMessage::Pong)) => {
                        writer.write(&[1]).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
                        }
                    }
                    Ok(ToPeer::Send(NetworkedMessage::Block(block))) => {
                        writer.write(&[2]).unwrap();
                        let mut buffer = vec![0; 0];
                        for byte in block.parent_hash { buffer.push(byte); }
                        for byte in block.miner_address { buffer.push(byte); }
                        for byte in block.nonce.to_le_bytes() { buffer.push(byte); }
                        for byte in block.difficulty.to_le_bytes() { buffer.push(byte); }
                        for byte in block.time.to_le_bytes() { buffer.push(byte); }
                        writer.write_all(&buffer).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
                        }
                    }
                    Ok(ToPeer::Send(NetworkedMessage::TopBlockHash(block_hash))) => {
                        writer.write(&[3]).unwrap();
                        writer.write(&block_hash).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
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

    (peer_addr, connection_thread, connection_sender)
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = args().collect();
    let mut should_mine = args.contains(&"--mine".to_string()) || args.contains(&"-m".to_string());
    let listen_addr_str = args.get(1).expect("You must supply a listen addr as arg1");
    let listener = TcpListener::bind(listen_addr_str).unwrap();
    listener.set_nonblocking(true).unwrap();

    let (node_sender, node_receiver) = channel();

    let quit_requested = Arc::new(AtomicBool::new(false));
    {
        let node_sender = node_sender.clone();
        let quit_requested = quit_requested.clone();
        ctrlc::set_handler(move || {
            if quit_requested.load(Ordering::SeqCst) {
                eprintln!("\rExiting forcefully");
                exit(1);
            } else {
                eprintln!("\rShutdown requested");
                quit_requested.store(true, Ordering::SeqCst);
                node_sender.send(ToNode::Quit).unwrap();
            }
        }).unwrap();
    }

    let (miner_sender, miner_receiver) = channel();
    let node_sender_for_miner = node_sender.clone();
    let miner_thread = spawn(move || -> io::Result<()> {
        let mut start_time = SystemTime::now();
        let mut job_block = None;

        loop {
            match miner_receiver.try_recv() {
                Ok(ToMiner::Start(parent_block)) => {
                    start_time = SystemTime::now();
                    job_block = Some(Block {
                        miner_address: [0; 32],
                        parent_hash: parent_block.hash(),
                        nonce: 0,
                        time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
                        difficulty: parent_block.difficulty,
                    });
                }
                Ok(ToMiner::Pause) => job_block = None,
                Ok(ToMiner::Quit) => break,
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

                            node_sender_for_miner.send(ToNode::Mined(block)).unwrap();
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

    let mut blockchain = Blockchain::new(Block {
        miner_address: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        parent_hash: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        nonce: 0,
        time: 1622999578, // SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
        difficulty: 2, // 2 gives results in 0-5 seconds, 3 gives results in 3-10 minutes
    });

    let mut connections = Vec::new();

    // Make outbound connections

    args.get(2).map(|arg| {
        eprintln!("Connecting to {}", arg);
        let node_sender = node_sender.clone();
        match TcpStream::connect(arg) {
            Ok(stream) => connections.push(
                accept_connection(stream, node_sender)
            ),
            Err(error) => eprintln!("Failed to connect to {}: {}", arg, error)
        };
    });

    if should_mine {
        miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
    }

    let mut terminal_input = true;
    let mut command = String::new();
    'outer: loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let main_sender = node_sender.clone();
                connections.push(accept_connection(stream, main_sender));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                // TODO: Replace with something smarter and OS dependant
                sleep(Duration::from_millis(10));
            }
            Err(error) => eprintln!("Error when accepting listeners: {}", error),
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
                                    connections.retain(|(_, _, connection_sender)| {
                                        if let Err(_) = connection_sender.send(ToPeer::Send(NetworkedMessage::Ping)) {
                                            false
                                        } else {
                                            true
                                        }
                                    });
                                } else if command == "quit" || command == "q" {
                                    quit_requested.store(true, Ordering::SeqCst);
                                } else if command == "mine" || command == "m" {
                                    miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                                    should_mine = true;
                                } else if command == "pause" || command == "p" {
                                    miner_sender.send(ToMiner::Pause).unwrap();
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
            match node_receiver.try_recv() {
                Ok(ToNode::Received(NetworkedMessage::Ping, from)) => {
                    eprintln!("Ping!");
                    for (addr, _thread, connection_sender) in &connections {
                        if *addr == from {
                            connection_sender.send(ToPeer::Send(NetworkedMessage::Pong)).unwrap();
                        }
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::Pong, _)) => {
                    eprintln!("Pong!");
                }
                Ok(ToNode::Received(NetworkedMessage::Block(block), from)) => {
                    eprintln!("Got block {} from {}", hex::encode(block.hash()), from);
                    if !blockchain.contains(block.hash()) {
                        match blockchain.add(block) {
                            Ok(()) => {
                                eprintln!("Inserting new block {}\nBlock height: {}", block,
                                          blockchain.block_height(block.hash()).unwrap());
                                connections.retain(|(addr, _, connection_sender)| {
                                    if *addr == from { return true; }

                                    if let Err(_) = connection_sender.send(ToPeer::Send(NetworkedMessage::Block(block))) {
                                        false
                                    } else {
                                        true
                                    }
                                });

                                for (addr, _, connection_sender) in &connections {
                                    if *addr == from {
                                        connection_sender.send(ToPeer::Send(
                                            NetworkedMessage::TopBlockHash(blockchain.top().hash())
                                        )).unwrap();
                                    }
                                }

                                if should_mine {
                                    miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                                }
                            }
                            Err(_) => eprintln!("Discarding invalid block {}", block)
                        }
                    } else {
                        eprintln!("Ignoring duplicate block {}", hex::encode(block.hash()))
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::TopBlockHash(block_hash), from)) => {
                    eprintln!("peer has block top: {}", hex::encode(block_hash));
                    for (addr, _, connection_sender) in &connections {
                        if *addr == from {
                            blockchain.get_child(block_hash).map(|block| {
                                eprintln!("Sending child block back");
                                connection_sender.send(ToPeer::Send(
                                    NetworkedMessage::Block(block)
                                )).unwrap();
                            });
                        }
                    }
                }
                Ok(ToNode::Mined(block)) => {
                    match blockchain.add(block) {
                        Ok(()) => {
                            eprintln!("Inserting new block {}\nBlock height: {}", block,
                                      blockchain.block_height(block.hash()).unwrap());
                            connections.retain(|(_, _, connection_sender)| {
                                if let Err(_) = connection_sender.send(ToPeer::Send(NetworkedMessage::Block(block))) {
                                    false
                                } else {
                                    true
                                }
                            });

                            if should_mine {
                                miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                            }
                        }
                        Err(_) => eprintln!("Discarding invalid block {}", block)
                    }
                }
                Ok(ToNode::ConnectionEstablished(connected_addr)) => {
                    for (addr, _, connection_sender) in &connections {
                        if *addr == connected_addr {
                            connection_sender.send(ToPeer::Send(NetworkedMessage::TopBlockHash(
                                blockchain.top().hash()
                            ))).unwrap();
                        }
                    }
                }
                Ok(ToNode::Quit) => {
                    miner_sender.send(ToMiner::Quit).unwrap();
                    miner_thread.join().unwrap().unwrap();

                    connections.retain(|(_, _, connection_sender)| {
                        if let Err(_) = connection_sender.send(ToPeer::Quit) {
                            false
                        } else {
                            true
                        }
                    });

                    while let Some((_, connection_thread, _)) = connections.pop() {
                        connection_thread.join().unwrap().unwrap();
                    }
                    break 'outer;
                }
                Err(_) => break,
            }
        }

        // TODO: Replace with something smarter and OS dependant
        sleep(Duration::from_millis(10));
    }

    Ok(())
}
