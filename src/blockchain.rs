use std::{array, io};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};

use crate::block;
use crate::block::{Block};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    TryFromSlice(array::TryFromSliceError),
    DeserializeBlock(block::DeserializeError),
    MissingParent,
    InvalidHash,
    NotFound,
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self { Error::Io(e) }
}

impl From<array::TryFromSliceError> for Error {
    fn from(e: array::TryFromSliceError) -> Self { Error::TryFromSlice(e) }
}

impl From<block::DeserializeError> for Error {
    fn from(e: block::DeserializeError) -> Self { Error::DeserializeBlock(e) }
}

#[derive(Debug)]
pub struct Blockchain {
    file: File,
    genesis: Block,
    block_entries: HashMap<[u8; 32], BlockEntry>,
    top: BlockEntry,
}

#[derive(Debug, Copy, Clone)]
pub struct BlockEntry {
    address: u64,
    height: u64,
}

impl Blockchain {
    pub fn new(genesis: Block, path: &str) -> Result<Blockchain, Error> {
        let mut block_entries = HashMap::new();
        let mut file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
        file.seek(SeekFrom::Start(0))?;
        let genesis = match Block::read(&mut file) {
            Ok(saved_genesis) if saved_genesis.hash() == genesis.hash() => {
                file.seek(SeekFrom::Start(u64::from(Block::SERIALIZED_LEN)))?;
                genesis
            }
            result => {
                if let Ok(saved_genesis) = result {
                    eprintln!("Discarding invalid blockchain DB: genesis mismatch\n\n\
                     Expected:\n{}\nActual:\n{}", genesis, saved_genesis);
                }
                if let Err(error) = result {
                    eprintln!("Discarding invalid blockchain DB: {:?}", error);
                }
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                genesis.write(&mut file)?;
                file.flush()?;
                genesis
            }
        };
        let genesis_entry = BlockEntry { address: 0, height: 0 };
        block_entries.insert(*genesis.hash(), genesis_entry.clone());
        let mut blockchain = Blockchain { file, block_entries, genesis, top: genesis_entry };

        loop {
            let address = blockchain.file.stream_position()?;
            match Block::read(&mut blockchain.file) {
                Ok(block) => {
                    let height = blockchain.height(&block.parent_hash()).unwrap() + 1;
                    let block_entry = BlockEntry { address, height };
                    blockchain.block_entries.insert(*block.hash(), block_entry.clone());
                    let top = blockchain.top;
                    if height > top.height {
                        blockchain.top = block_entry;
                    }
                    blockchain.file.seek(SeekFrom::Start(address + u64::from(Block::SERIALIZED_LEN)))?;
                }
                Err(block::DeserializeError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                e => {
                    e?;
                }
            }
        }
        Ok(blockchain)
    }

    pub fn add(&mut self, block: Block) -> Result<(), Error> {
        match self.block_entries.get(&block.parent_hash()) {
            Some(BlockEntry { address: parent_address, height: parent_height }) => {
                eprintln!("Reading parent {}", parent_address);
                self.file.seek(SeekFrom::Start(*parent_address))?;
                let parent = Block::read(&mut self.file)?;
                let height = parent_height + 1;

                if block.difficulty() < self.difficulty_target(&parent)? {
                    Err(Error::InvalidHash)
                } else {
                    self.file.seek(SeekFrom::End(0))?;
                    let address = self.file.stream_position()?;
                    block.write(&mut self.file)?;
                    self.file.flush()?;
                    let block_entry = BlockEntry { address, height };
                    if height > self.top.height {
                        self.top = block_entry.clone();
                    }
                    self.block_entries.insert(*block.hash(), block_entry);
                    Ok(())
                }
            }
            None => Err(Error::MissingParent),
        }
    }

    pub fn get(&mut self, hash: &[u8; 32]) -> Result<Block, Error> {
        match self.block_entries.get(hash) {
            Some(BlockEntry { address, .. }) => {
                self.file.seek(SeekFrom::Start(*address))?;
                Ok(Block::read(&mut self.file)?)
            }
            None => Err(Error::NotFound),
        }
    }

    // Returns the child of the main chain
    pub fn get_child(&mut self, parent_hash: &[u8; 32]) -> Result<Block, Error> {
        let mut block = self.top()?;
        while block.parent_hash() != *parent_hash {
            block = self.get(&block.parent_hash())?
        }
        Ok(block)
    }

    #[allow(dead_code)]
    pub fn genesis(&self) -> Block {
        self.genesis
    }

    pub fn top(&mut self) -> Result<Block, Error> {
        self.file.seek(SeekFrom::Start(self.top.address))?;
        Ok(Block::read(&mut self.file)?)
    }

    pub fn height(&self, hash: &[u8; 32]) -> Result<u64, Error> {
        self.block_entries.get(hash).map(|x| x.height).ok_or(Error::NotFound)
    }

    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.block_entries.contains_key(hash)
    }

    pub fn difficulty_target(&self, block: &Block) -> Result<u8, Error> {
        let height = self.height(block.hash())?;
        if height > 1010 {
            Ok(3)
        } else if height > 1000 {
            Ok(2)
        } else {
            Ok(1)
        }
    }

    pub fn print_blocks(&mut self) -> Result<(), Error> {
        self.file.seek(SeekFrom::Start(0))?;
        loop {
            let address = self.file.stream_position()?;
            match Block::read(&mut self.file) {
                Ok(block) => {
                    let height = self.height(block.hash())?;
                    let top = block.hash() == self.top()?.hash();
                    eprintln!("{}, height: {}, top: {}", block, height, top);
                    self.file.seek(SeekFrom::Start(address + u64::from(Block::SERIALIZED_LEN)))?;
                }
                Err(block::DeserializeError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                e => {
                    e?;
                }
            }
        }
        Ok(())
    }
}