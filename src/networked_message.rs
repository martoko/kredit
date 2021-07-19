use std::{array, io};
use std::io::{BufWriter, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, TcpStream};

use crate::block;
use crate::block::Block;

#[derive(Debug, Clone)]
pub enum NetworkedMessage {
    Block(Block),
    Ping,
    Pong,
    Request([u8; 32]),
    RequestChild([u8; 32]),
    PeerAddresses(Vec<SocketAddr>),
}

#[derive(Debug)]
pub enum DeserializeError {
    Io(io::Error),
    ArrayTryFromSlice(array::TryFromSliceError),
    BlockDeserialize(block::DeserializeError),
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

                    let peer = match ip {
                        IpAddr::V4(ip) => SocketAddr::from(SocketAddrV4::new(ip, port)),
                        IpAddr::V6(ip) => SocketAddr::from(SocketAddrV6::new(ip, port, 0, 0))
                    };

                    peers.push(peer);
                }
                Ok(NetworkedMessage::PeerAddresses(peers))
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
            NetworkedMessage::PeerAddresses(peers) => {
                writer.write_all(&[5])?;
                writer.write_all(&(peers.len() as u64).to_le_bytes())?;
                for peer in peers {
                    match peer {
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
                }
                writer.flush()?;
                Ok(())
            }
        }
    }
}