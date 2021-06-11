use std::fmt;

use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub enum AddBlockError {
    MissingParent
}

#[derive(Debug)]
pub struct Blockchain {
    blocks: Vec<Block>,
}

impl Blockchain {
    pub fn new(seed: Block) -> Blockchain {
        Blockchain {
            blocks: vec![seed]
        }
    }

    pub fn add(&mut self, block: Block) -> Result<(), AddBlockError> {
        match self.get(block.parent_hash) {
            Some(parent) => {
                if parent.hash() == self.blocks.last().unwrap().hash() {
                    self.blocks.push(block);
                    Ok(())
                } else {
                    todo!()
                }
            }
            None => Err(AddBlockError::MissingParent),
        }
    }

    pub fn get(&self, hash: [u8; 32]) -> Option<Block> {
        for &block in &self.blocks {
            if block.hash() == hash {
                return Some(block);
            }
        }

        None
    }

    pub fn get_child(&self, parent_hash: [u8; 32]) -> Option<Block> {
        for &block in &self.blocks {
            if block.parent_hash == parent_hash {
                return Some(block);
            }
        }

        None
    }

    pub fn genesis(&self) -> Block {
        *self.blocks.first().unwrap()
    }

    pub fn top(&self) -> Block {
        *self.blocks.last().unwrap()
    }

    pub fn block_height(&self, hash: [u8; 32]) -> Option<u64> {
        for i in 0..self.blocks.len() {
            if self.blocks[i].hash() == hash {
                return Some(i as u64);
            }
        }

        None
    }

    pub fn contains(&self, hash: [u8; 32]) -> bool {
        for &block in &self.blocks {
            if block.hash() == hash {
                return true;
            }
        }

        false
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Block {
    pub parent_hash: [u8; 32],
    pub miner_address: [u8; 32],
    pub nonce: u64,
    pub difficulty: u8,
    pub time: u64,
}

impl Block {
    pub fn hash(&self) -> [u8; 32] {
        let mut sha256 = Sha256::new();
        sha256.update(self.parent_hash);
        sha256.update(self.miner_address);
        sha256.update(self.nonce.to_le_bytes());
        sha256.update(self.difficulty.to_le_bytes());
        sha256.update(self.time.to_le_bytes());
        sha256.finalize().into()
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