use std::{array, io};
use std::convert::TryInto;
use std::io::{BufWriter, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream};
use std::process::exit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use structopt::StructOpt;

use blockchain::Block;

use crate::blockchain::Blockchain;

mod blockchain;
mod terminal_input;

#[derive(Debug, Copy, Clone)]
enum ToMiner {
    Quit,
    Start(Block),
    Pause,
}

#[derive(Debug, Clone)]
enum ToPeer {
    Quit,
    Send(NetworkedMessage),
}

#[derive(Debug, Clone)]
pub enum ToNode {
    Quit,
    Received(NetworkedMessage, SocketAddr),
    Mined(Block),
    ConnectionEstablished(SocketAddr),
    Connect(SocketAddr),
    SendPing,
    StartMining,
    PauseMining,
    ShowTopBlock,
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

#[derive(Debug, Clone)]
pub enum NetworkedMessage {
    Block(Block),
    Ping,
    Pong,
    Request([u8; 32]),
    RequestChild([u8; 32]),
    Peers(Vec<SocketAddr>),
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
                Ok(NetworkedMessage::Request(buffer))
            }
            4 => {
                let mut buffer = [0; 32];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read(&mut buffer)?;
                stream.set_nonblocking(true)?;
                Ok(NetworkedMessage::RequestChild(buffer))
            }
            5 => {
                // Read the size of the peers list
                let mut buffer = [0; 8];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read(&mut buffer)?;
                stream.set_nonblocking(true)?;
                let size = u64::from_le_bytes(buffer);

                // Read the list of peers
                let mut peers = Vec::new();
                for _ in 0..size {
                    // Read a SocketAddr
                    let mut buffer = [0; 1];
                    stream.set_nonblocking(false)?;
                    (&mut (&*stream)).read(&mut buffer)?;
                    stream.set_nonblocking(true)?;
                    let ip: IpAddr = match buffer[0] {
                        // ipv4
                        0 => {
                            // Read ipv4
                            let mut buffer = [0; 4];
                            stream.set_nonblocking(false)?;
                            (&mut (&*stream)).read(&mut buffer)?;
                            stream.set_nonblocking(true)?;
                            IpAddr::V4(Ipv4Addr::from(buffer))
                        }
                        // ipv6
                        1 => {
                            // Read ipv6
                            let mut buffer = [0; 16];
                            stream.set_nonblocking(false)?;
                            (&mut (&*stream)).read(&mut buffer)?;
                            stream.set_nonblocking(true)?;
                            IpAddr::V6(Ipv6Addr::from(buffer))
                        }
                        _ => return Err(DeserializeError::from(io::Error::new(
                            ErrorKind::InvalidData, format!("Invalid socket addr type: {}", buffer[0]), // TODO: Bad error style
                        )))
                    };

                    // Read port
                    let mut buffer = [0; 2];
                    stream.set_nonblocking(false)?;
                    (&mut (&*stream)).read(&mut buffer)?;
                    stream.set_nonblocking(true)?;
                    let port = u16::from_le_bytes(buffer);

                    let peer = match ip {
                        IpAddr::V4(ip) => SocketAddr::from(SocketAddrV4::new(ip, port)),
                        IpAddr::V6(ip) => SocketAddr::from(SocketAddrV6::new(ip, port, 0, 0))
                    };

                    peers.push(peer);
                }
                Ok(NetworkedMessage::Peers(peers))
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

// TODO: Do not exchange inbound connections
// TODO: File-backed blockchain datastrcture?
// TODO: Smarter blockchain datastructure
// TODO: Proper command prompt by making cross-platform cbreak-mode crate
// TODO: Clean up all the unwraps, handling closed connections better
// TODO: "peers" command

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
                    Ok(ToPeer::Send(NetworkedMessage::Request(block_hash))) => {
                        writer.write(&[3]).unwrap();
                        writer.write(&block_hash).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
                        }
                    }
                    Ok(ToPeer::Send(NetworkedMessage::RequestChild(block_hash))) => {
                        writer.write(&[4]).unwrap();
                        writer.write(&block_hash).unwrap();
                        if let Err(err) = writer.flush() {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, err);
                            break 'main;
                        }
                    }
                    Ok(ToPeer::Send(NetworkedMessage::Peers(peers))) => {
                        writer.write(&[5]).unwrap();
                        writer.write(&(peers.len() as u64).to_le_bytes()).unwrap();
                        for peer in peers {
                            match peer {
                                SocketAddr::V4(addr) => {
                                    writer.write(&[0]).unwrap();
                                    writer.write(&addr.ip().octets()).unwrap();
                                    writer.write(&addr.port().to_le_bytes()).unwrap();
                                }
                                SocketAddr::V6(addr) => {
                                    writer.write(&[1]).unwrap();
                                    writer.write(&addr.ip().octets()).unwrap();
                                    writer.write(&addr.port().to_le_bytes()).unwrap();
                                }
                            }
                        }
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

#[derive(StructOpt, Debug)]
#[structopt()]
struct Args {
    /// Bind node to this address
    #[structopt()]
    address: SocketAddr,

    /// Connect to this address
    #[structopt(short, long)]
    connect: Vec<SocketAddr>,

    /// Start mining right away
    #[structopt(short, long)]
    mine: bool,
}

fn main() -> std::io::Result<()> {
    let args = Args::from_args();
    let mut should_mine = args.mine;
    let listener = TcpListener::bind(args.address).unwrap();
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
    for addr in args.connect {
        node_sender.send(ToNode::Connect(addr)).unwrap()
    }

    if should_mine {
        miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
    }
    let mut terminal_input_enabled = true;
    let mut command_reader = terminal_input::CommandReader::new();
    'outer: loop {
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    let main_sender = node_sender.clone();
                    connections.push(accept_connection(stream, main_sender));
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(error) => eprintln!("Error when accepting listeners: {}", error),
            }
        }

        if terminal_input_enabled {
            loop {
                match command_reader.poll() {
                    Ok(to_node) => node_sender.send(to_node).unwrap(),
                    Err(terminal_input::PollError::WouldBlock) => break,
                    Err(error) => {
                        eprintln!("Failed to read from terminal, disabling terminal input: {:?}", error);
                        terminal_input_enabled = false;
                        break;
                    }
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
                                    // if in sync mode
                                    if *addr == from {
                                        connection_sender.send(ToPeer::Send(
                                            NetworkedMessage::RequestChild(block.hash())
                                        )).unwrap();
                                    }
                                    // it not in sync mode
                                    if *addr != from {
                                        connection_sender.send(ToPeer::Send(
                                            NetworkedMessage::Block(block)
                                        )).unwrap();
                                    }
                                }

                                if should_mine {
                                    miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                                }
                            }
                            Err(blockchain::AddBlockError::MissingParent) => {
                                for (addr, _, connection_sender) in &connections {
                                    if *addr == from {
                                        connection_sender.send(ToPeer::Send(NetworkedMessage::Request(block.parent_hash))).unwrap()
                                    }
                                }
                            }
                            Err(_) => eprintln!("Discarding invalid block {}", block)
                        }
                    } else {
                        eprintln!("Ignoring duplicate block {}", hex::encode(block.hash()))
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::Request(block_hash), from)) => {
                    eprintln!("peer requests: {}", hex::encode(block_hash));
                    for (addr, _, connection_sender) in &connections {
                        if *addr == from {
                            blockchain.get(block_hash).map(|block| {
                                connection_sender.send(ToPeer::Send(
                                    NetworkedMessage::Block(block)
                                )).unwrap();
                            });
                        }
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::RequestChild(block_hash), from)) => {
                    eprintln!("peer requests child: {}", hex::encode(block_hash));
                    for (addr, _, connection_sender) in &connections {
                        if *addr == from {
                            blockchain.get_child(block_hash).map(|block| {
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
                            connection_sender.send(ToPeer::Send(NetworkedMessage::Block(
                                blockchain.top()
                            ))).unwrap();
                            let peers = connections.iter().map(|(addr, _, _)| *addr).collect();
                            connection_sender.send(ToPeer::Send(NetworkedMessage::Peers(peers)))
                                .unwrap();
                        }
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::Peers(peers), _)) => {
                    eprintln!("Got {} peers", peers.len());
                    for peer in peers {
                        let mut found = false;
                        for (addr, _, _) in &connections {
                            if *addr == peer {
                                found = true;
                            }
                        }

                        if !found {
                            eprintln!("Got new peer {}", peer);
                            node_sender.send(ToNode::Connect(peer)).unwrap();
                        } else {
                            eprintln!("Got existing peer {}", peer);
                        }
                    }
                }
                Ok(ToNode::Quit) => {
                    quit_requested.store(true, Ordering::SeqCst);
                    // The \r is because we might be called from Ctrl+c
                    eprintln!("\rShutdown requested");
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
                Ok(ToNode::Connect(addr)) => {
                    eprintln!("Connecting to {}", addr);
                    let node_sender = node_sender.clone();
                    match TcpStream::connect(addr) {
                        Ok(stream) => connections.push(
                            accept_connection(stream, node_sender)
                        ),
                        Err(error) => eprintln!("Failed to connect to {}: {}", addr, error)
                    };
                }
                Ok(ToNode::StartMining) => {
                    miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                    should_mine = true;
                }
                Ok(ToNode::PauseMining) => {
                    miner_sender.send(ToMiner::Pause).unwrap();
                    should_mine = false;
                }
                Ok(ToNode::ShowTopBlock) => {
                    eprintln!("{}, blockheight: {}", blockchain.top(), blockchain.block_height(blockchain.top().hash()).unwrap())
                }
                Ok(ToNode::SendPing) => {
                    // TODO: Move duplicated sending logic somewhere
                    connections.retain(|(_, _, connection_sender)| {
                        if let Err(_) = connection_sender.send(ToPeer::Send(NetworkedMessage::Ping)) {
                            false
                        } else {
                            true
                        }
                    });
                }
                Err(_) => break,
            }
        }

        // TODO: Replace with something smarter and OS dependant (i.e. sleep until
        //       terminal/socket/miner event received)
        sleep(Duration::from_millis(10));
    }

    Ok(())
}
