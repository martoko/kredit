use std::{array, io, result};

pub use block::Block;
pub use block::hash;
pub use file_blockchain::Blockchain as Blockchain;
pub use crate::blockchain::block::DeserializeBlockError;
use std::num::TryFromIntError;

mod block;
mod file_blockchain;
mod memory_blockchain;

pub type Result<T> = result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    MissingParent,
    InvalidHash,
    NotFound,
    Io(io::Error),
    TryFromSlice(array::TryFromSliceError),
    TryFromInt(TryFromIntError)
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self { Error::Io(e) }
}

impl From<array::TryFromSliceError> for Error {
    fn from(e: array::TryFromSliceError) -> Self { Error::TryFromSlice(e) }
}

impl From<block::DeserializeBlockError> for Error {
    fn from(e: block::DeserializeBlockError) -> Self {
        match e  {
            DeserializeBlockError::Io(e) => Error::Io(e),
            DeserializeBlockError::TryFromSlice(e) => Error::TryFromSlice(e)
        }
    }
}

impl From<TryFromIntError> for Error {
    fn from(e: TryFromIntError) -> Self { Error::TryFromInt(e) }
}