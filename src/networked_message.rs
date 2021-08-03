use std::{array, io};
use std::io::{BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};

use ed25519_dalek::*;

use crate::block;
use crate::block::Block;

#[derive(Debug, Clone)]
pub enum NetworkedMessage {
    Block(Block),
    Ping,
    Pong,
    // Each node has a unique id, their public key.
    // We can challenge a node on their public key to confirm their identity.
    // This is more less only useful for not connecting to the same node twice.
    Challenge([u8; 32]),
    ChallengeResponse { signature: Signature, public_key: PublicKey },
    Request([u8; 32]),
    RequestChild([u8; 32]),
    Peers(Vec<(SocketAddr, PublicKey)>),
}

#[derive(Debug)]
pub enum DeserializeError {
    Io(io::Error),
    ArrayTryFromSlice(array::TryFromSliceError),
    BlockDeserialize(block::DeserializeError),
    Ed25519Error(ed25519_dalek::ed25519::Error),
    InvalidSocketAddrType(u8),
    InvalidMessageType(u8),
}

impl From<io::Error> for DeserializeError {
    fn from(e: io::Error) -> Self { DeserializeError::Io(e) }
}

impl From<array::TryFromSliceError> for DeserializeError {
    fn from(e: array::TryFromSliceError) -> Self { DeserializeError::ArrayTryFromSlice(e) }
}

impl From<block::DeserializeError> for DeserializeError {
    fn from(e: block::DeserializeError) -> Self { DeserializeError::BlockDeserialize(e) }
}

impl From<ed25519_dalek::ed25519::Error> for DeserializeError {
    fn from(e: ed25519_dalek::ed25519::Error) -> Self { DeserializeError::Ed25519Error(e) }
}

impl NetworkedMessage {
    pub fn receive(stream: &TcpStream, r#type: u8) -> Result<NetworkedMessage, DeserializeError> {
        match r#type {
            0 => Ok(NetworkedMessage::Ping),
            1 => Ok(NetworkedMessage::Pong),
            2 => {
                stream.set_nonblocking(false)?;
                let block = Block::read(&mut (&*stream))?;
                stream.set_nonblocking(true)?;
                Ok(NetworkedMessage::Block(block))
            }
            3 => {
                let mut buffer = [0; 32];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read_exact(&mut buffer)?;
                stream.set_nonblocking(true)?;
                Ok(NetworkedMessage::Request(buffer))
            }
            4 => {
                let mut buffer = [0; 32];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read_exact(&mut buffer)?;
                stream.set_nonblocking(true)?;
                Ok(NetworkedMessage::RequestChild(buffer))
            }
            5 => {
                // Read the size of the peers list
                let mut buffer = [0; 8];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read_exact(&mut buffer)?;
                stream.set_nonblocking(true)?;
                let size = u64::from_le_bytes(buffer);

                // Read the list of peers
                let mut peers = Vec::new();
                for _ in 0..size {
                    // Read a SocketAddr
                    let mut buffer = [0; 1];
                    stream.set_nonblocking(false)?;
                    (&mut (&*stream)).read_exact(&mut buffer)?;
                    stream.set_nonblocking(true)?;
                    let ip: IpAddr = match buffer[0] {
                        // ipv4
                        0 => {
                            // Read ipv4
                            let mut buffer = [0; 4];
                            stream.set_nonblocking(false)?;
                            (&mut (&*stream)).read_exact(&mut buffer)?;
                            stream.set_nonblocking(true)?;
                            IpAddr::V4(Ipv4Addr::from(buffer))
                        }
                        // ipv6
                        1 => {
                            // Read ipv6
                            let mut buffer = [0; 16];
                            stream.set_nonblocking(false)?;
                            (&mut (&*stream)).read_exact(&mut buffer)?;
                            stream.set_nonblocking(true)?;
                            IpAddr::V6(Ipv6Addr::from(buffer))
                        }
                        r#type => return Err(DeserializeError::InvalidSocketAddrType(r#type))
                    };

                    // Read port
                    let mut buffer = [0; 2];
                    stream.set_nonblocking(false)?;
                    (&mut (&*stream)).read_exact(&mut buffer)?;
                    stream.set_nonblocking(true)?;
                    let port = u16::from_le_bytes(buffer);

                    let addr = match ip {
                        IpAddr::V4(ip) => SocketAddr::from(SocketAddrV4::new(ip, port)),
                        IpAddr::V6(ip) => SocketAddr::from(SocketAddrV6::new(ip, port, 0, 0))
                    };

                    let mut buffer = [0; PUBLIC_KEY_LENGTH];
                    stream.set_nonblocking(false)?;
                    (&mut (&*stream)).read_exact(&mut buffer)?;
                    stream.set_nonblocking(true)?;
                    let public_key = PublicKey::from_bytes(&buffer)?;

                    peers.push((addr, public_key));
                }
                Ok(NetworkedMessage::Peers(peers))
            }
            6 => {
                let mut buffer = [0; 32];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read_exact(&mut buffer)?;
                stream.set_nonblocking(true)?;
                Ok(NetworkedMessage::Challenge(buffer))
            }
            7 => {
                let mut buffer = [0; 64];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read_exact(&mut buffer)?;
                stream.set_nonblocking(true)?;
                let signature = buffer.into();

                let mut buffer = [0; 32];
                stream.set_nonblocking(false)?;
                (&mut (&*stream)).read_exact(&mut buffer)?;
                stream.set_nonblocking(true)?;
                let public_key = PublicKey::from_bytes(&buffer)?;

                Ok(NetworkedMessage::ChallengeResponse { signature, public_key })
            }
            r#type => Err(DeserializeError::InvalidMessageType(r#type))
        }
    }

    pub fn send(&self, writer: &mut BufWriter<&TcpStream>) -> Result<(), io::Error> {
        match self {
            NetworkedMessage::Ping => {
                writer.write_all(&[0])?;
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::Pong => {
                writer.write_all(&[1])?;
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::Block(block) => {
                writer.write_all(&[2])?;
                block.write(writer)?;
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::Request(block_hash) => {
                writer.write_all(&[3])?;
                writer.write_all(block_hash)?;
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::RequestChild(block_hash) => {
                writer.write_all(&[4])?;
                writer.write_all(block_hash)?;
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::Peers(peers) => {
                writer.write_all(&[5])?;
                writer.write_all(&(peers.len() as u64).to_le_bytes())?;
                for (addr, public_key) in peers {
                    match addr {
                        SocketAddr::V4(addr) => {
                            writer.write_all(&[0])?;
                            writer.write_all(&addr.ip().octets())?;
                            writer.write_all(&addr.port().to_le_bytes())?;
                        }
                        SocketAddr::V6(addr) => {
                            writer.write_all(&[1])?;
                            writer.write_all(&addr.ip().octets())?;
                            writer.write_all(&addr.port().to_le_bytes())?;
                        }
                    }
                    writer.write_all(&public_key.to_bytes())?;
                }
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::Challenge(challenge) => {
                writer.write_all(&[6])?;
                writer.write_all(challenge)?;
                writer.flush()?;
                Ok(())
            }
            NetworkedMessage::ChallengeResponse { signature: ciphertext, public_key } => {
                writer.write_all(&[7])?;
                writer.write_all(&ciphertext.to_bytes())?;
                writer.write_all(&public_key.to_bytes())?;
                writer.flush()?;
                Ok(())
            }
        }
    }
}