use std::io;
use std::io::{BufWriter, ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::exit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::thread::{JoinHandle, sleep, spawn};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use structopt::StructOpt;

use crate::blockchain::Block;
use crate::blockchain::Blockchain;
use crate::Direction::{Inbound, Outbound};
use crate::networked_message::NetworkedMessage;

mod blockchain;
mod terminal_input;
mod networked_message;

#[derive(Debug, Copy, Clone)]
enum ToMiner {
    Quit,
    Start(Block),
    Stop,
}

#[derive(Debug, Clone)]
enum ToPeer {
    Quit,
    Send(NetworkedMessage),
}

#[derive(Debug)]
pub struct Connection {
    addr: SocketAddr,
    thread: JoinHandle<io::Result<()>>,
    sender: Sender<ToPeer>,
    direction: Direction,
}

#[derive(Debug, PartialEq, Copy, Clone)]
enum Direction {
    Inbound,
    Outbound,
}

#[derive(Debug)]
pub enum ToNode {
    Quit,
    Received(NetworkedMessage, SocketAddr),
    Mined(Block),
    ConnectionEstablished(Connection),
    Connect(SocketAddr),
    SendPing,
    StartMiner,
    StopMiner,
    ShowTopBlock,
    Peers,
}

fn difficulty(hash: &[u8; 32]) -> u8 {
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

// TODO: File-backed blockchain datastrcture?
// TODO: Smarter blockchain datastructure
// TODO: Better command prompt by making cross-platform cbreak-mode crate
// TODO: Clean up all the unwraps
// TODO: Simulate poor network conditions
// TODO: Non-blocking read_exact
// TODO: Do not open connection twice between nodes (one outbound and one inbound)

fn accept_connection(stream: TcpStream, node_sender: Sender<ToNode>, direction: Direction)
                     -> Result<Connection, io::Error> {
    stream.set_nonblocking(true)?;
    let peer_addr = stream.peer_addr()?;
    let local_addr = stream.local_addr()?;
    eprintln!("connection: them {}, us {}", peer_addr, local_addr);
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
                        let message = NetworkedMessage::receive(&stream, buffer[0]).unwrap();
                        node_sender.send(ToNode::Received(message, peer_addr)).unwrap();
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
                    Ok(ToPeer::Quit) => {
                        eprintln!("Quitting connection thread {}->{}", local_addr, peer_addr);
                        break 'main;
                    }
                    Ok(ToPeer::Send(message)) => {
                        if let Err(error) = message.send(&mut writer) {
                            eprintln!("Connection closed {}->{}: {}",
                                      local_addr, peer_addr, error);
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

    Ok(Connection {
        addr: peer_addr,
        thread: connection_thread,
        sender: connection_sender,
        direction,
    })
}

/// Sends a message to given peers, dropping any peers where sending fails
fn send_to_peers(connections: &mut Vec<Connection>, to_peer: ToPeer) {
    connections.retain(|Connection { sender, .. }| {
        // A send operation can only fail if the receiving end of a channel is disconnected,
        // implying that the data could never be received. The error contains the data being sent
        // as a payload so it can be recovered.
        sender.send(to_peer.clone()).is_ok()
    });
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

fn main() -> io::Result<()> {
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
        let mut parent_hash: [u8; 32] = [0; 32];
        let mut miner_address: [u8; 32] = [0; 32];
        let mut nonce: u64 = 0;
        let mut difficulty: u8 = 0;
        let mut time: u64 = 0;
        let mut is_mining = false;

        loop {
            match miner_receiver.try_recv() {
                Ok(ToMiner::Start(parent_block)) => {
                    start_time = SystemTime::now();

                    parent_hash = *parent_block.hash();
                    miner_address = [0; 32];
                    nonce = 0;
                    difficulty = parent_block.difficulty();
                    time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

                    is_mining = true;
                }
                Ok(ToMiner::Stop) => is_mining = false,
                Ok(ToMiner::Quit) => break,
                Err(TryRecvError::Empty) => {
                    if is_mining {
                        if let Some(new_nonce) = nonce.checked_add(1) {
                            nonce = new_nonce;
                        } else {
                            // We ran out of nonce, bump the time and restart
                            time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                        }

                        let hash = blockchain::hash(parent_hash, miner_address, nonce, difficulty, time);
                        if crate::difficulty(&hash) >= difficulty {
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

                            node_sender_for_miner.send(ToNode::Mined(
                                Block::new(parent_hash, miner_address, nonce, difficulty, time)
                            )).unwrap();
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

    let mut blockchain = Blockchain::new(Block::new(
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        0,
        2, // 2 gives results in 0-5 seconds, 3 gives results in 3-10 minutes
        1622999578, // SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
    ));

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
                    let node_sender_for_connection = node_sender.clone();
                    match accept_connection(stream, node_sender_for_connection, Inbound) {
                        Ok(c) => node_sender.send(ToNode::ConnectionEstablished(c)).unwrap(),
                        Err(error) => eprintln!("Error when accepting listeners: {}", error),
                    }
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
                    for Connection { addr, sender, .. } in &connections {
                        if *addr == from {
                            sender.send(ToPeer::Send(NetworkedMessage::Pong)).unwrap();
                        }
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::Pong, _)) => {
                    eprintln!("Pong!");
                }
                Ok(ToNode::Received(NetworkedMessage::Block(block), from)) => {
                    eprintln!("Got block {} from {}", hex::encode(block.hash()), from);
                    if !blockchain.contains(&block.hash()) {
                        match blockchain.add(block) {
                            Ok(()) => {
                                eprintln!("Inserting new block {}\nBlock height: {}", block,
                                          blockchain.height(&block.hash()).unwrap());
                                connections.retain(|Connection { addr, sender, .. }| {
                                    if *addr == from { return true; }

                                    if let Err(_) = sender.send(ToPeer::Send(NetworkedMessage::Block
                                        (block))) {
                                        false
                                    } else {
                                        true
                                    }
                                });

                                for Connection { addr, sender, .. } in &connections {
                                    // if in sync mode
                                    if *addr == from {
                                        sender.send(ToPeer::Send(
                                            NetworkedMessage::RequestChild(*block.hash())
                                        )).unwrap();
                                    }
                                    // it not in sync mode
                                    if *addr != from {
                                        sender.send(ToPeer::Send(
                                            NetworkedMessage::Block(block)
                                        )).unwrap();
                                    }
                                }

                                if should_mine {
                                    miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                                }
                            }
                            Err(blockchain::AddBlockError::MissingParent) => {
                                for Connection { addr, sender, .. } in &connections {
                                    if *addr == from {
                                        sender.send(ToPeer::Send(NetworkedMessage::Request(block
                                            .parent_hash()))).unwrap()
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
                    for Connection { addr, sender, .. } in &connections {
                        if *addr == from {
                            blockchain.get(&block_hash).map(|block| {
                                sender.send(ToPeer::Send(
                                    NetworkedMessage::Block(block)
                                )).unwrap();
                            });
                        }
                    }
                }
                Ok(ToNode::Received(NetworkedMessage::RequestChild(block_hash), from)) => {
                    eprintln!("peer requests child: {}", hex::encode(block_hash));
                    for Connection { addr, sender, .. } in &connections {
                        if *addr == from {
                            blockchain.get_child(&block_hash).map(|block| {
                                sender.send(ToPeer::Send(
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
                                      blockchain.height(&block.hash()).unwrap());
                            send_to_peers(&mut connections, ToPeer::Send(NetworkedMessage::Block(block)));

                            if should_mine {
                                miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                            }
                        }
                        Err(_) => eprintln!("Discarding invalid block {}", block)
                    }
                }
                Ok(ToNode::ConnectionEstablished(connection)) => {
                    let peers = connections.iter()
                        .filter(|Connection { direction, .. }| *direction == Outbound)
                        .map(|Connection { addr, .. }| *addr).collect();
                    let connection_addr = connection.addr;

                    connection.sender.send(ToPeer::Send(NetworkedMessage::Block(
                        blockchain.top()
                    ))).and_then(|()| {
                        connection.sender.send(ToPeer::Send(NetworkedMessage::Peers(peers)))
                    }).and_then(|()| {
                        connections.push(connection);
                        Ok(())
                    }).unwrap_or_else(|error| {
                        eprintln!("Connection closed {}: {}", connection_addr, error)
                    });
                }
                Ok(ToNode::Received(NetworkedMessage::Peers(peers), _)) => {
                    eprintln!("Got {} peers", peers.len());
                    for peer in peers {
                        let mut found = false;
                        for Connection { addr, .. } in &connections {
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

                    send_to_peers(&mut connections, ToPeer::Quit);

                    while let Some(Connection { thread, .. }) = connections.pop() {
                        thread.join().unwrap().unwrap();
                    }
                    break 'outer;
                }
                Ok(ToNode::Connect(addr)) => {
                    eprintln!("Connecting to {}", addr);
                    let node_sender_for_connection = node_sender.clone();
                    match TcpStream::connect(addr) {
                        Ok(stream) => match accept_connection(stream, node_sender_for_connection, Outbound) {
                            Ok(c) => node_sender.send(ToNode::ConnectionEstablished(c)).unwrap(),
                            Err(error) => eprintln!("Failed to connect to {}: {}", addr, error),
                        }
                        Err(error) => eprintln!("Failed to connect to {}: {}", addr, error)
                    };
                }
                Ok(ToNode::StartMiner) => {
                    miner_sender.send(ToMiner::Start(blockchain.top().clone())).unwrap();
                    should_mine = true;
                    eprintln!("Mining: {}", should_mine);
                }
                Ok(ToNode::StopMiner) => {
                    miner_sender.send(ToMiner::Stop).unwrap();
                    should_mine = false;
                    eprintln!("Mining: {}", should_mine);
                }
                Ok(ToNode::ShowTopBlock) => {
                    eprintln!("{}, blockheight: {}", blockchain.top(), blockchain.height
                    (&blockchain.top().hash()).unwrap())
                }
                Ok(ToNode::SendPing) => {
                    eprintln!("Sending ping");
                    send_to_peers(&mut connections, ToPeer::Send(NetworkedMessage::Ping));
                }
                Ok(ToNode::Peers) => {
                    eprintln!("{} peers", connections.len());
                    for connection in connections.iter() {
                        eprintln!("{:#?}", connection);
                    }
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
