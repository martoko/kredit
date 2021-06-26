use std::{fmt, io, array};
use std::convert::TryInto;
use std::io::{Read, Write};

use sha2::{Digest, Sha256};

#[derive(Debug)]
pub enum DeserializeBlockError {
    Io(io::Error),
    TryFromSlice(array::TryFromSliceError),
}

impl From<io::Error> for DeserializeBlockError {
    fn from(e: io::Error) -> Self { DeserializeBlockError::Io(e) }
}

impl From<array::TryFromSliceError> for DeserializeBlockError {
    fn from(e: array::TryFromSliceError) -> Self { DeserializeBlockError::TryFromSlice(e) }
}

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

    /// Writes the block using Write::write_all
    pub fn write(&self, writer: &mut dyn Write) -> Result<(), io::Error> {
        writer.write_all(&self.parent_hash())?;
        writer.write_all(&self.miner_address())?;
        writer.write_all(&self.nonce().to_le_bytes())?;
        writer.write_all(&self.difficulty().to_le_bytes())?;
        writer.write_all(&self.time().to_le_bytes())?;
        Ok(())
    }

    /// Reads a block using Read::read_exact
    pub fn read(reader: &mut dyn Read) -> Result<Block, DeserializeBlockError> {
        let mut buffer = [0; Block::SERIALIZED_LEN];
        reader.read_exact(&mut buffer)?;
        Ok(Block::new(
            buffer[0..32].try_into()?,
            buffer[32..64].try_into()?,
            u64::from_le_bytes(buffer[64..72].try_into()?),
            buffer[72],
            u64::from_le_bytes(buffer[73..81].try_into()?),
        ))
    }

    pub const SERIALIZED_LEN: usize = 32 + 32 + 8 + 1 + 8;
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