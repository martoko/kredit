use std::{io, net::SocketAddr, process::exit, sync::Arc, sync::atomic::{AtomicBool, Ordering}, sync::mpsc::{channel, Sender}, thread::{JoinHandle, sleep}, thread, time::Duration};
use std::fmt::{Display, Formatter};
use std::sync::atomic::Ordering::SeqCst;

use structopt::StructOpt;

use crate::{
    blockchain::Blockchain,
    Direction::{Inbound, Outbound},
    networked_message::NetworkedMessage,
};
use crate::block::Block;
use crate::miner::ToMiner;
use crate::networking::Networking;
use crate::node::{Node, Outgoing};
use crate::terminal_input::{Command, PollError};
use rand::Rng;

mod blockchain;
mod terminal_input;
mod networked_message;
mod node;
mod networking;
mod miner;
mod block;

#[derive(Debug)]
pub struct Connection {
    addr: SocketAddr,
    thread: JoinHandle<io::Result<()>>,
    sender: Sender<NetworkedMessage>,
    direction: Direction,
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

#[derive(Debug, PartialEq, Copy, Clone)]
enum Direction {
    Inbound,
    Outbound,
}

impl Display for Direction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Inbound => write!(f, "Inbound"),
            Outbound => write!(f, "Outbound"),
        }
    }
}

// TODO: # Introduce sync phase #
//       Phase 1, Sync:
//         seed peers
//            For now just rely on the user specifying exact IP's
//            In the future the nodes should exchange peers with each other
//         synchronize blockchain
//            Maybe just choose a node and ask it to send a full history
//       Phase 2, maintain the blockchain & mine
//
// TODO: In line with the syncing phase, do not spam the network as much/be smarter about when to
//       request
// TODO: Add transactions to blocks
// TODO: Handle blocks of different sizes, AKA don't assume all blocks have the same size, but DO
//       assume a max size for buffer purposes/DOS-protection
// TODO: Persist peers on shutdown
// TODO: Clean up all the unwraps
// TODO: Simulate poor network conditions

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

    /// Path to the blockchain database file, if none exists, it will be created
    #[structopt(short, long)]
    blockchain_path: String,
}

pub fn spawn_with_name<F, T>(name: &str, f: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T,
        F: Send + 'static,
        T: Send + 'static
{
    let builder = thread::Builder::new();
    builder.name(name.into()).spawn(f).expect("failed to spawn thread")
}

fn main() -> io::Result<()> {
    let args = Args::from_args();
    let should_mine_on_startup = args.mine;

    let quit_requested = Arc::new(AtomicBool::new(false));
    {
        let quit_requested = quit_requested.clone();
        ctrlc::set_handler(move || {
            if quit_requested.load(Ordering::SeqCst) {
                eprintln!("\rExiting forcefully");
                exit(1);
            } else {
                eprint!("\r"); // Let further printing override the '^C'
                quit_requested.store(true, Ordering::SeqCst);
            }
        }).unwrap();
    }

    let blockchain = Blockchain::new(Block::new(
        [0; 32],
        [0; 32],
        0,
        1622999578, // SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
    ), &args.blockchain_path).unwrap();

    let (node_sender, node_receiver) = channel();
    let (networking_sender, networking_receiver) = channel();
    let (miner_sender, miner_receiver) = channel();
    let mut networking = Networking::new(args.address, node_sender.clone(), networking_receiver);
    let mut node = Node::new(blockchain, node_receiver, networking_sender.clone(), miner_sender.clone());
    let miner_address = rand::thread_rng().gen::<[u8; 32]>();
    let miner_thread = miner::spawn_miner(miner_address, miner_receiver, node_sender.clone());

    for addr in args.connect {
        networking_sender.send(Outgoing::Connect(addr)).unwrap();
    }

    if should_mine_on_startup {
        let top = node.blockchain.top().unwrap();
        miner_sender.send(ToMiner::Start(
            node.blockchain.difficulty_target(&top).unwrap(), top.clone(),
        )).unwrap();
    }
    let mut terminal_input_enabled = true;
    let mut command_reader = terminal_input::CommandReader::new();
    loop {
        if quit_requested.load(SeqCst) {
            eprintln!("Shutting down...");
            std::mem::drop(miner_sender);
            std::mem::drop(node);
            networking.close();
            miner_thread.join().unwrap().unwrap();
            break;
        }
        if terminal_input_enabled {
            loop {
                match command_reader.poll() {
                    Ok(Command::Quit) => quit_requested.store(true, SeqCst),
                    Ok(Command::Connect(addr)) =>
                        networking_sender.send(Outgoing::Connect(addr)).unwrap(),
                    Ok(Command::SendPing) =>
                        networking_sender.send(Outgoing::Broadcast(NetworkedMessage::Ping)).unwrap(),
                    Ok(Command::StartMiner) => {
                        let top = node.blockchain.top().unwrap();
                        miner_sender.send(ToMiner::Start(
                            node.blockchain.difficulty_target(&top).unwrap(),
                            top.clone())
                        ).unwrap();
                    }
                    Ok(Command::StopMiner) => miner_sender.send(ToMiner::Stop).unwrap(),
                    Ok(Command::ShowTopBlock) => {
                        let top = node.blockchain.top().unwrap();
                        eprintln!(
                            "{}, height: {}",
                            top,
                            node.blockchain.height(top.hash()).unwrap())
                    }
                    Ok(Command::Peers) => eprintln!("{:?}", node.peers),
                    Ok(Command::Blocks) => node.blockchain.print_blocks().unwrap(),
                    Err(PollError::WouldBlock) => break,
                    Err(error) => {
                        eprintln!("Failed to read from terminal, disabling terminal input: {:?}", error);
                        terminal_input_enabled = false;
                        break;
                    }
                }
            }
        }
        networking.process(); // TODO: Any reason these are not thread blocking on the channel, handling their own sleep?
        node.process(); // TODO: Any reason these are not thread blocking on the channel, handling their own sleep?

        // TODO: Replace with something smarter and OS dependant (i.e. sleep until
        //       terminal/socket/miner event received)
        sleep(Duration::from_millis(10));
    }

    Ok(())
}
