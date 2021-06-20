use std::fmt;

use sha2::{Digest, Sha256};

#[derive(Debug, Copy, Clone)]
pub struct Block {
    parent_hash: [u8; 32],
    miner_address: [u8; 32],
    nonce: u64,
    difficulty: u8,
    time: u64,
    hash: [u8; 32],
}

pub fn hash(
    parent_hash: [u8; 32],
    miner_address: [u8; 32],
    nonce: u64,
    difficulty: u8,
    time: u64,
) -> [u8; 32] {
    let mut sha256 = Sha256::new();
    sha256.update(parent_hash);
    sha256.update(miner_address);
    sha256.update(nonce.to_le_bytes());
    sha256.update(difficulty.to_le_bytes());
    sha256.update(time.to_le_bytes());
    sha256.finalize().into()
}

impl Block {
    pub fn new(
        parent_hash: [u8; 32],
        miner_address: [u8; 32],
        nonce: u64,
        difficulty: u8,
        time: u64,
    ) -> Block {
        Block {
            parent_hash,
            miner_address,
            nonce,
            difficulty,
            time,
            hash: hash(parent_hash, miner_address, nonce, difficulty, time),
        }
    }

    pub fn parent_hash(&self) -> [u8; 32] { self.parent_hash }
    pub fn miner_address(&self) -> [u8; 32] { self.miner_address }
    pub fn nonce(&self) -> u64 { self.nonce }
    pub fn difficulty(&self) -> u8 { self.difficulty }
    pub fn time(&self) -> u64 { self.time }
    pub fn hash(&self) -> &[u8; 32] { &self.hash }
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