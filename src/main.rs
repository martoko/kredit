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

use blockchain::Block;

use crate::blockchain::Blockchain;
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
struct Connection {
    addr: SocketAddr,
    thread: JoinHandle<io::Result<()>>,
    sender: Sender<ToPeer>,
}

#[derive(Debug, Clone)]
pub enum ToNode {
    Quit,
    Received(NetworkedMessage, SocketAddr),
    Mined(Block),
    ConnectionEstablished(SocketAddr),
    Connect(SocketAddr),
    SendPing,
    StartMiner,
    StopMiner,
    ShowTopBlock,
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
// TODO: Clean up all the unwraps
// TODO: "peers" command
// TODO: Simulate poor network conditions

fn accept_connection(stream: TcpStream, node_sender: Sender<ToNode>)
                     -> Connection {
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

    Connection {
        addr: peer_addr,
        thread: connection_thread,
        sender: connection_sender,
    }
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
                Ok(ToMiner::Stop) => job_block = None,
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
                    if !blockchain.contains(block.hash()) {
                        match blockchain.add(block) {
                            Ok(()) => {
                                eprintln!("Inserting new block {}\nBlock height: {}", block,
                                          blockchain.block_height(block.hash()).unwrap());
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
                                            NetworkedMessage::RequestChild(block.hash())
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
                                            .parent_hash))).unwrap()
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
                            blockchain.get(block_hash).map(|block| {
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
                            blockchain.get_child(block_hash).map(|block| {
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
                                      blockchain.block_height(block.hash()).unwrap());
                            connections.retain(|Connection { sender, .. }| {
                                if let Err(_) = sender.send(ToPeer::Send(NetworkedMessage::Block(block))) {
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
                    for Connection { addr, sender, .. } in &connections {
                        if *addr == connected_addr {
                            sender.send(ToPeer::Send(NetworkedMessage::Block(
                                blockchain.top()
                            ))).unwrap();
                            let peers = connections.iter().map(|Connection { addr, .. }| *addr).collect();
                            sender.send(ToPeer::Send(NetworkedMessage::Peers(peers)))
                                .unwrap();
                        }
                    }
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

                    connections.retain(|Connection { sender, .. }| {
                        if let Err(_) = sender.send(ToPeer::Quit) {
                            false
                        } else {
                            true
                        }
                    });

                    while let Some(Connection { thread, .. }) = connections.pop() {
                        thread.join().unwrap().unwrap();
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
                    eprintln!("{}, blockheight: {}", blockchain.top(), blockchain.block_height(blockchain.top().hash()).unwrap())
                }
                Ok(ToNode::SendPing) => {
                    eprintln!("Sending ping");

                    // TODO: Move duplicated sending logic somewhere
                    connections.retain(|Connection { sender, .. }| {
                        if let Err(_) = sender.send(ToPeer::Send(NetworkedMessage::Ping)) {
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
