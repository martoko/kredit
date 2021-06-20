mod block;
mod file_blockchain;
mod memory_blockchain;

pub use block::Block;
pub use block::hash;
pub use memory_blockchain::MemoryBlockchain as Blockchain;
// pub use memory_blockchain::FileBlockchain as Blockchain;

#[derive(Debug, Clone)]
pub enum AddBlockError {
    MissingParent,
    InvalidHash,
}