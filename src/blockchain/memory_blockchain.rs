use std::collections::HashMap;

use crate::blockchain::AddBlockError;
use crate::blockchain::block::Block;
use crate::difficulty;

#[derive(Debug, Copy, Clone)]
struct Node {
    block: Block,
    height: u64,
}

#[derive(Debug)]
pub struct MemoryBlockchain {
    genesis: Node,
    blocks: HashMap<[u8; 32], Node>,
    // TODO: Use set instead with the hash of the hash
    top: Node,
}

impl MemoryBlockchain {
    pub fn new(seed: Block) -> MemoryBlockchain {
        let mut blocks = HashMap::new();
        let node = Node { block: seed, height: 0 };
        blocks.insert(*seed.hash(), node);
        MemoryBlockchain { blocks, genesis: node, top: node }
    }

    pub fn add(&mut self, block: Block) -> Result<(), AddBlockError> {
        match self.blocks.get(&block.parent_hash()) {
            Some(parent) => {
                if difficulty(block.hash()) != parent.block.difficulty() {
                    Err(AddBlockError::InvalidHash)
                } else {
                    let node = Node { block, height: parent.height + 1 };
                    self.blocks.insert(*block.hash(), node);
                    if node.height > self.top.height {
                        self.top = node;
                    }
                    Ok(())
                }
            }
            None => Err(AddBlockError::MissingParent),
        }
    }

    pub fn get(&self, hash: &[u8; 32]) -> Option<Block> {
        self.blocks.get(hash).map(|x| x.block)
    }

    // Returns the child of the main chain
    pub fn get_child(&self, parent_hash: &[u8; 32]) -> Option<Block> {
        let mut block = Some(&self.top);
        while let Some(value) = block {
            if value.block.parent_hash() == *parent_hash {
                return Some(value.block);
            }
            block = self.blocks.get(&value.block.parent_hash());
        }

        None
    }

    pub fn genesis(&self) -> Block {
        self.genesis.block
    }

    pub fn top(&self) -> Block {
        self.top.block
    }

    pub fn height(&self, hash: &[u8; 32]) -> Option<u64> {
        self.blocks.get(hash).map(|x| x.height)
    }

    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.blocks.contains_key(hash)
    }
}