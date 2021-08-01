use std::io;
use std::sync::mpsc::{Receiver, RecvError, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{block, spawn_with_name};
use crate::block::Block;
use crate::node::Incoming;

#[derive(Debug, Copy, Clone)]
pub enum ToMiner {
    Start(Block),
    Reset(Block),
    Stop,
}

pub fn spawn_miner(
    miner_address: [u8; 32],
    receiver: Receiver<ToMiner>,
    sender: Sender<Incoming>,
) -> JoinHandle<io::Result<()>> {
    let miner_thread = spawn_with_name("miner", move || -> io::Result<()> {
        loop {
            match receiver.recv() {
                Ok(ToMiner::Start(parent_block)) => {
                    let mut start_time = SystemTime::now();
                    let mut parent_hash = *parent_block.hash();
                    let mut nonce: u64 = 0;
                    let difficulty_target: u8 = 1;
                    let mut time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

                    loop {
                        match receiver.try_recv() {
                            // Cancel the current mining in favor of this parent block
                            Ok(ToMiner::Reset(parent_block)) |
                            Ok(ToMiner::Start(parent_block)) => {
                                start_time = SystemTime::now();
                                parent_hash = *parent_block.hash();
                                nonce = 0;
                                // difficulty_target = parent_block.difficulty();
                                time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                            }
                            Err(TryRecvError::Empty) => {
                                if let Some(new_nonce) = nonce.checked_add(1) {
                                    nonce = new_nonce;
                                } else {
                                    // We ran out of nonce, bump the time and restart
                                    time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                                }

                                let hash = block::hash(parent_hash, miner_address, nonce, time);
                                if block::difficulty(&hash) >= difficulty_target {
                                    let duration = start_time.elapsed().unwrap();
                                    // Artificially make the minimum mining time 2 seconds for debug
                                    // if let Some(duration) = std::time::Duration::from_secs(2)
                                    //     .checked_sub(duration) {
                                    //     std::thread::sleep(duration);
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

                                    if let Err(_) = sender.send(Incoming::Mined(
                                        Block::new(parent_hash, miner_address, nonce, time)
                                    )) {
                                        break;
                                    }
                                    match receiver.recv() {
                                        Ok(ToMiner::Reset(parent_block)) |
                                        Ok(ToMiner::Start(parent_block)) => {
                                            start_time = SystemTime::now();
                                            parent_hash = *parent_block.hash();
                                            nonce = 0;
                                            // difficulty_target = parent_block.difficulty();
                                            time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                                        }
                                        Ok(ToMiner::Stop) => break,
                                        Err(RecvError {}) => break,
                                    }
                                }
                            }
                            // Pause the mining
                            Ok(ToMiner::Stop) => break,
                            // Channel has been closed, and so we do the same for the thread
                            Err(TryRecvError::Disconnected) => break,
                        }
                    }
                }
                Ok(ToMiner::Stop) |
                Ok(ToMiner::Reset(_)) => { /* Ignore stop requests when not mining. */ }
                // Channel has been closed, and so we do the same for the thread
                Err(RecvError {}) => break
            }
        }

        Ok(())
    });

    miner_thread
}