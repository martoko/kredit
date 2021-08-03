use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

use ed25519_dalek::{Keypair, PublicKey, Signer, Verifier};
use rand::Rng;

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
    // Maybe split into verified vec<(SocketAddr, PublicKey)> and unverified peers vec<(SocketAddr, challenge)>
    pub peers: HashMap<SocketAddr, ([u8; 32], Option<PublicKey>)>,
    node_receiver: Receiver<Incoming>,
    networking_sender: Sender<Outgoing>,
    miner_sender: Sender<ToMiner>,
    keypair: Keypair,
}

impl Node {
    pub fn new(
        blockchain: Blockchain,
        node_receiver: Receiver<Incoming>,
        networking_sender: Sender<Outgoing>,
        miner_sender: Sender<ToMiner>,
    ) -> Node {
        Node {
            blockchain,
            node_receiver,
            networking_sender,
            miner_sender,
            peers: HashMap::new(),
            keypair: Keypair::generate(&mut rand_0_7::rngs::OsRng {}),
        }
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
                Ok(Message(_peer, Peers(peers))) => {
                    eprintln!("Got {} peers", peers.len());
                    for (peer_addr, peer_public_key) in peers {
                        if self.peers.iter().any(|(addr, (_, public_key))| *addr == peer_addr || Some(peer_public_key) == *public_key) {
                            eprintln!("Got existing peer {:?}, {}", peer_public_key, peer_addr)
                        } else {
                            eprintln!("Got new peer {}", peer_addr);
                            self.networking_sender.send(Outgoing::Connect(peer_addr)).unwrap()
                        }
                    }
                }
                // A client is challenging our identity by asking us to sign a "challenge"
                Ok(Message(peer, Challenge(challenge))) => {
                    eprintln!("Got {} challenge from {}", hex::encode(challenge), peer);
                    self.networking_sender.send(Outgoing::Send(
                        peer,
                        NetworkedMessage::ChallengeResponse {
                            public_key: self.keypair.public,
                            signature: self.keypair.sign(&challenge),
                        },
                    )).unwrap()
                }
                Ok(Message(peer_address, ChallengeResponse { signature, public_key })) => {
                    match self.peers.get(&peer_address) {
                        Some((challenge, ..)) => {
                            let challenge = *challenge;
                            match public_key.verify(&challenge, &signature) {
                                Ok(()) => {
                                    eprintln!("Received valid challenge response from {}", peer_address);
                                    self.peers.insert(peer_address, (challenge, Some(public_key)));
                                }
                                Err(e) => eprintln!("Received invalid challenge response from {}: {}", peer_address, e)
                            }
                        }
                        None => eprintln!("Received unsolicited challenge response from {}", peer_address)
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
                Ok(ConnectionEstablished(peer_address)) => {
                    let challenge = rand::thread_rng().gen::<[u8; 32]>();
                    if let Some(_) = self.peers.insert(peer_address, (challenge, None)) {
                        eprintln!("Connection established twice for {}", peer_address);
                    }
                    self.networking_sender.send(Outgoing::Send(peer_address, NetworkedMessage::Challenge(challenge))).unwrap();
                    self.networking_sender.send(Outgoing::Send(peer_address, NetworkedMessage::Block(self.blockchain.top().unwrap()))).unwrap();
                    let peers = self.peers.iter().filter_map(|(addr, (_, public_key))| {
                        public_key.map(|public_key| (*addr, public_key))
                    }).collect();
                    self.networking_sender.send(Outgoing::Send(peer_address, NetworkedMessage::Peers(peers))).unwrap();
                }
                Ok(ConnectionClosed(peer_adress)) => {
                    self.peers.remove(&peer_adress);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}