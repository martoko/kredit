use std::fmt;

use sha2::{Digest, Sha256};

use crate::difficulty;

#[derive(Debug, Clone)]
pub enum AddBlockError {
    MissingParent,
    InvalidHash,
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
                if difficulty(block.hash()) != parent.difficulty {
                    Err(AddBlockError::InvalidHash)
                } else {
                    self.blocks.push(block);
                    Ok(())
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

    // Returns the child of the main chain
    pub fn get_child(&self, parent_hash: [u8; 32]) -> Option<Block> {
        let mut block = Some(self.top());
        while let Some(value) = block {
            if value.parent_hash == parent_hash {
                return Some(value);
            }
            block = self.get(value.parent_hash);
        }

        None
    }

    pub fn genesis(&self) -> Block {
        *self.blocks.first().unwrap()
    }

    pub fn top(&self) -> Block {
        let mut best_block = self.genesis();
        let mut best_height = self.block_height(best_block.hash()).unwrap();

        for block in &self.blocks {
            let height = self.block_height(block.hash()).unwrap();
            if height > best_height {
                best_height = height;
                best_block = *block;
            }
        }

        best_block
    }

    pub fn block_height(&self, hash: [u8; 32]) -> Option<u64> {
        if let Some(block) = self.get(hash) {
            let mut height = 0 as u64;
            let mut block = self.get(block.parent_hash);
            while let Some(value) = block {
                height += 1;
                block = self.get(value.parent_hash);
            }

            Some(height)
        } else {
            None
        }
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