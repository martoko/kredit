use std::collections::HashMap;
use std::convert::TryFrom;
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::io;

use crate::blockchain::{Block, DeserializeBlockError, Error, Result};
use crate::difficulty;

// #[derive(Debug, Copy, Clone)]
// pub struct Address {
//     value: u64
// }

#[derive(Debug)]
pub struct Blockchain {
    file: File,
    genesis: Block,
    blocks_addresses: HashMap<[u8; 32], u64>,
    top: Block,
}

impl Blockchain {
    pub fn new(genesis: Block, path: &str) -> Result<Blockchain> {
        let mut blocks_addresses = HashMap::new();
        let mut file = OpenOptions::new().read(true).write(true).create(true).open(path)?;
        file.seek(SeekFrom::Start(0))?;
        let genesis = match Block::read(&mut file) {
            Ok(saved_genesis) if saved_genesis.hash() == genesis.hash() => {
                file.seek(SeekFrom::Start(u64::try_from(Block::SERIALIZED_LEN)?))?;
                genesis
            },
            _ => {
                eprintln!("Failed to read genesis from existing db");
                file.set_len(0)?;
                file.seek(SeekFrom::Start(0))?;
                genesis.write(&mut file)?;
                file.flush()?;
                genesis
            }
        };
        blocks_addresses.insert(*genesis.hash(), 0);
        let mut blockchain = Blockchain { file, blocks_addresses, genesis, top: genesis };

        loop {
            let address = blockchain.file.stream_position()?;
            match Block::read(&mut blockchain.file) {
                Ok(block) => {
                    blockchain.blocks_addresses.insert(*block.hash(), address);
                    let top = blockchain.top;
                    if blockchain.height(block.hash())? > blockchain.height(top.hash())? {
                        blockchain.top = block;
                    }
                    blockchain.file.seek(SeekFrom::Start(address + u64::try_from(Block::SERIALIZED_LEN)?))?;
                }
                Err(DeserializeBlockError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    break;
                }
                e => {
                    e?;
                }
            }
        }
        Ok(blockchain)
    }

    pub fn add(&mut self, block: Block) -> Result<()> {
        match self.blocks_addresses.get(&block.parent_hash()) {
            Some(parent_address) => {
                eprintln!("Reading parent {}", parent_address);
                self.file.seek(SeekFrom::Start(*parent_address))?;
                let parent = Block::read(&mut self.file)?;

                if difficulty(block.hash()) != parent.difficulty() {
                    Err(Error::InvalidHash)
                } else {
                    self.file.seek(SeekFrom::End(0))?;
                    let address = self.file.stream_position()?;
                    block.write(&mut self.file)?;
                    self.file.flush()?;
                    let top = self.top;
                    if self.height(parent.hash())? + 1 > self.height(top.hash())? {
                        self.top = block;
                    }
                    self.blocks_addresses.insert(*block.hash(), address);
                    Ok(())
                }
            }
            None => Err(Error::MissingParent),
        }
    }

    pub fn get(&mut self, hash: &[u8; 32]) -> Result<Block> {
        match self.blocks_addresses.get(hash) {
            Some(address) => {
                self.file.seek(SeekFrom::Start(*address))?;
                Ok(Block::read(&mut self.file)?)
            }
            None => Err(Error::NotFound),
        }
    }

    // Returns the child of the main chain
    pub fn get_child(&mut self, parent_hash: &[u8; 32]) -> Result<Block> {
        let mut block = self.top;
        while block.parent_hash() != *parent_hash {
            block = self.get(&block.parent_hash())?
        }
        Ok(block)
    }

    #[allow(dead_code)]
    pub fn genesis(&self) -> Block {
        self.genesis
    }

    pub fn top(&self) -> Block {
        self.top
    }

    pub fn height(&mut self, hash: &[u8; 32]) -> Result<u64> {
        let mut block = self.get(hash)?;
        let mut height = 0;
        while block.hash() != self.genesis.hash() {
            block = self.get(&block.parent_hash())?;
            height += 1;
        }
        Ok(height)
    }

    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.blocks_addresses.contains_key(hash)
    }

    pub fn print_blocks(&mut self) -> Result<()> {
        loop {
            let address = self.file.stream_position()?;
            match Block::read(&mut self.file) {
                Ok(block) => {
                    let height = self.height(block.hash())?;
                    let top = block.hash() == self.top.hash();
                    eprintln!("{}, height: {}, top: {}", block, height, top);
                    self.file.seek(SeekFrom::Start(address + u64::try_from(Block::SERIALIZED_LEN)?))?;
                }
                Err(DeserializeBlockError::Io(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
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