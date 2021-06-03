use std::net::{TcpListener, TcpStream, SocketAddr};
use std::str::FromStr;
use std::sync::mpsc::{Sender, Receiver};
use std::sync::{mpsc, Arc, RwLock};
use std::thread::spawn;
use std::io::{Write, BufRead, BufReader, BufWriter, Read};
use std::error::Error;
use sha2::{Sha512, Digest};
use hex::ToHex;

fn hashing() {
    // same for Sha512
    let mut sha512 = Sha512::new();
    sha512.update(b"hello world\n");
    let digest = sha512.finalize();

    eprintln!("{}", hex::encode(digest));
}

fn main() -> std::io::Result<()> {
    let listener = TcpListener::bind(
        SocketAddr::from_str("127.0.0.1:44447").unwrap()
    ).unwrap();

    for stream in listener.incoming() {
        match stream {
            Err(error) => eprintln!("Error when accepting listeners: {}", error),
            Ok(stream) => {
                stream.set_nonblocking(true).unwrap();
                println!("connection from {} to {}", stream.peer_addr()?, stream.local_addr()?);
                spawn(move || {
                    let mut reader = BufReader::new(&stream);
                    let mut writer = BufWriter::new(&stream);

                    stream.read()

                    writer.write(b"Welcome to Simple Chat Server!\n").unwrap();
                    writer.write(b"Plz input yourname: ").unwrap();
                    writer.flush().unwrap();
                    let mut name = String::new();
                    reader.read_line(&mut name).unwrap();
                    let name = name.trim_end();
                    writer.write_fmt(format_args!("Hello, {}!\n", name)).unwrap();
                    writer.flush().unwrap();

                    let mut position = 0;
                    loop {
                        {
                            // let lines = arc.read().unwrap();
                            // println!("DEBUG arc.read() => {:?}", lines);
                            // for i in position..lines.len() {
                            //     writer.write_fmt(format_args!("{}", lines[i])).unwrap();
                            //     position = lines.len();
                            // };
                        }
                        writer.write(b" > ").unwrap();
                        writer.flush().unwrap();

                        // reader.
                        let mut reads = String::new();
                        reader.read_line(&mut reads).unwrap(); //TODO: non-blocking read
                        if reads.trim().len() != 0 {
                            println!("DEBUG: reads len =>>>>> {}", reads.len());
                            println!("DEBUG: msg [{}] said: {}", name, reads);
                            println!("DEBUG: got '{}' from {}", reads.trim(), name);
                        }
                    }
                });
            }
        }
    }

    Ok(())
}
