use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use NetworkedMessage::*;

use crate::block::Block;
use crate::blockchain;
use crate::blockchain::Blockchain;
use crate::miner::ToMiner;
use crate::networked_message::NetworkedMessage;
use crate::node::Incoming::*;

pub enum Outgoing {
    Connect(SocketAddr),
    Send(SocketAddr, NetworkedMessage),
    Forward(SocketAddr, NetworkedMessage),
    Broadcast(NetworkedMessage),
}

pub enum Incoming {
    Message(SocketAddr, NetworkedMessage),
    Mined(Block),
    ConnectionEstablished(SocketAddr),
    ConnectionClosed(SocketAddr),
}

/// A node handles messages to and from peers, but is oblivious to the underlying networking.
pub struct Node {
    pub blockchain: Blockchain,
    pub peers: Vec<SocketAddr>,
    node_receiver: Receiver<Incoming>,
    networking_sender: Sender<Outgoing>,
    miner_sender: Sender<ToMiner>,
}

impl Node {
    pub fn new(
        blockchain: Blockchain,
        node_receiver: Receiver<Incoming>,
        networking_sender: Sender<Outgoing>,
        miner_sender: Sender<ToMiner>,
    ) -> Node {
        Node { blockchain, node_receiver, networking_sender, miner_sender, peers: vec![] }
    }

    pub fn process(&mut self) {
        loop {
            match self.node_receiver.try_recv() {
                Ok(Message(peer, Ping)) => {
                    eprintln!("{}: Ping!", peer);
                    self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::Pong)).unwrap()
                }
                Ok(Message(peer, Pong)) => {
                    eprintln!("{}: Pong!", peer);
                }
                Ok(Message(peer, Block(block))) => {
                    eprintln!("Got block {} from {}", hex::encode(block.hash()), peer);
                    if !self.blockchain.contains(&block.hash()) {
                        let top = self.blockchain.top().unwrap();
                        let old_top_height = self.blockchain.height(top.hash()).unwrap();
                        match self.blockchain.add(block) {
                            Ok(()) => {
                                eprintln!("Inserting new block {}\nBlock height: {}", block,
                                          self.blockchain.height(&block.hash()).unwrap());
                                self.networking_sender.send(Outgoing::Forward(peer, NetworkedMessage::Block(block))).unwrap();
                                if self.blockchain.height(&block.hash()).unwrap() > old_top_height {
                                    self.miner_sender.send(ToMiner::Reset(
                                        self.blockchain.difficulty_target(&block).unwrap(), block)
                                    ).unwrap();
                                }

                                self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::RequestChild(*block.hash()))).unwrap();
                            }
                            Err(blockchain::Error::MissingParent) => {
                                self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::Request(block.parent_hash()))).unwrap();
                            }
                            Err(e) => eprintln!("Discarding invalid block {:?} {}", e, block)
                        }
                    } else {
                        eprintln!("Ignoring duplicate block {}", hex::encode(block.hash()))
                    }
                }
                Ok(Message(peer, Request(block_hash))) => {
                    eprintln!("{} requests: {}", peer, hex::encode(block_hash));
                    match self.blockchain.get(&block_hash) {
                        Ok(block) => self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::Block(block))).unwrap(),
                        Err(blockchain::Error::NotFound) => eprintln!("Block {} not found", hex::encode(block_hash)),
                        Err(error) => eprintln!("{:?}", error),
                    }
                }
                Ok(Message(peer, RequestChild(block_hash))) => {
                    eprintln!("{} requests child: {}", peer, hex::encode(block_hash));
                    match self.blockchain.get_child(&block_hash) {
                        Ok(block) => self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::Block(block))).unwrap(),
                        Err(blockchain::Error::NotFound) => eprintln!("No child of {} found", hex::encode(block_hash)),
                        Err(error) => eprintln!("{:?}", error),
                    }
                }
                Ok(Message(_peer, PeerAddresses(peers))) => {
                    eprintln!("Got {} peers", peers.len());
                    for peer in peers {
                        if self.peers.contains(&peer) {
                            eprintln!("Got existing peer {}", peer)
                        } else {
                            eprintln!("Got new peer {}", peer);
                            self.networking_sender.send(Outgoing::Connect(peer)).unwrap()
                        }
                    }
                }
                Ok(Mined(block)) => {
                    match self.blockchain.add(block) {
                        Ok(()) => {
                            eprintln!("Mined new block {}\nBlock height: {}", block,
                                      self.blockchain.height(&block.hash()).unwrap());
                            self.networking_sender.send(Outgoing::Broadcast(Block(block))).unwrap();
                            self.miner_sender.send(ToMiner::Reset(
                                self.blockchain.difficulty_target(&block).unwrap(), block,
                            )).unwrap();
                        }
                        Err(e) => eprintln!("Failed to add mined block: {:?}", e)
                    }
                }
                Ok(ConnectionEstablished(peer)) => {
                    self.peers.push(peer);
                    self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::Block(self.blockchain.top().unwrap()))).unwrap();
                    self.networking_sender.send(Outgoing::Send(peer, NetworkedMessage::PeerAddresses(self.peers.clone()))).unwrap();
                }
                Ok(ConnectionClosed(peer)) => {
                    self.peers.retain(|p| *p != peer);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}