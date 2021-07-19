use std::{
    net::SocketAddr,
    str::FromStr,
    time::Duration,
};

use crossterm::{
    ErrorKind,
    event::Event,
};

use Command::*;

#[derive(Debug)]
pub enum Command {
    Quit,
    Connect(SocketAddr),
    SendPing,
    StartMiner,
    StopMiner,
    ShowTopBlock,
    Peers,
    Blocks,
}

#[derive(Debug)]
pub enum PollError {
    WouldBlock,
    Crossterm(crossterm::ErrorKind),
}

impl From<crossterm::ErrorKind> for PollError {
    fn from(crossterm: ErrorKind) -> Self {
        PollError::Crossterm(crossterm)
    }
}

pub struct CommandReader {
    buffer: String,
}

impl CommandReader {
    pub fn new() -> CommandReader {
        CommandReader { buffer: String::new() }
    }

    pub fn poll(&mut self) -> Result<Command, PollError> {
        // Keep draining events as long as there are new events available
        loop {
            match crossterm::event::poll(Duration::from_secs(0))? {
                true => {
                    // It's guaranteed that read() wont block if `poll` returns `Ok(true)`
                    match crossterm::event::read()? {
                        Event::Key(crossterm::event::KeyEvent { code, .. }) => {
                            match code {
                                crossterm::event::KeyCode::Enter => {
                                    let buffer_clone = self.buffer.clone();
                                    self.buffer.clear();

                                    let mut iterator = buffer_clone.split_whitespace();
                                    match iterator.next() {
                                        Some("ping") => return Ok(SendPing),
                                        Some("quit") | Some("q") => return Ok(Quit),
                                        Some("mine") | Some("m") => return Ok(StartMiner),
                                        Some("pause") | Some("p") => return Ok(StopMiner),
                                        Some("top") | Some("t") => return Ok(ShowTopBlock),
                                        Some("peers") | Some("e") => return Ok(Peers),
                                        Some("blocks") | Some("b") => return Ok(Blocks),
                                        Some("connect") | Some("c") => {
                                            match iterator.next() {
                                                Some(address) => {
                                                    match SocketAddr::from_str(address) {
                                                        Ok(address) => return Ok(Connect(address)),
                                                        Err(error) => eprintln!("Unable to parse \
                                                        address \
                                                        {}: {}", address, error)
                                                    }
                                                }
                                                None => eprintln!("You must supply an address to \
                                                connect, usage: connect ADDRESS")
                                            }
                                        }
                                        Some(command) => {
                                            eprintln!("Unknown command {}", command)
                                        }
                                        None => ()
                                    }
                                }
                                crossterm::event::KeyCode::Char(c) => {
                                    self.buffer.push(c);
                                }
                                _ => () // Ignore keys we don't care about
                            }
                        }
                        _ => () // Ignore event we don't care about
                    }
                }
                false => return Err(PollError::WouldBlock),
            }
        }
    }
}