use std::collections::HashMap;
use std::sync::Arc;

use async_std::channel::{Receiver, Sender};
use async_std::io::prelude::BufReadExt;
use async_std::io::{BufReader, BufWriter, Lines, WriteExt};
use async_std::net::{TcpListener, TcpStream};
use async_std::stream::StreamExt;
use async_std::sync::Mutex;
use async_std::{channel, task};

fn main() {
    if let Err(err) = run() {
        eprintln!("{}", err);
    };
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let fut = init_listener("127.0.0.1:8080");
    task::block_on(fut)
}

struct UserChannel {
    receiver: Receiver<ChannelMessage>,
    sender: Sender<ChannelMessage>,
}

async fn init_listener(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(addr).await.unwrap();
    println!("Listening on {addr}");
    let user_receiver_channels: HashMap<String, UserChannel> = HashMap::new();
    let ptr_user_receiver_channels = Arc::new(Mutex::new(user_receiver_channels));
    while let Some(stream) = listener.incoming().next().await {
        let stream = stream?;
        println!("Accepting from:{}", stream.peer_addr()?);
        let ptr_clone_user_receiver_channels = ptr_user_receiver_channels.clone();
        let _handle = task::spawn(init_connection(stream, ptr_clone_user_receiver_channels));
    }
    Ok(())
}

async fn init_connection(
    stream: TcpStream,
    ptr_clone_user_receiver_channels: Arc<Mutex<HashMap<String, UserChannel>>>,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let ptr_stream = Arc::new(stream);
    let reader = BufReader::new(&*ptr_stream);
    let mut lines = reader.lines();
    let name = match lines.next().await {
        None => Err("No name")?,
        Some(line) => line?,
    };
    println!("\"{}\" logged in", name);
    let (sender, receiver) = channel::unbounded();
    ptr_clone_user_receiver_channels
        .lock()
        .await
        .entry(name.clone())
        .or_insert(UserChannel { receiver, sender });

    task::spawn(init_chat_receiver(
        ptr_stream.clone(),
        ptr_clone_user_receiver_channels.clone(),
        name.clone(),
    ));
    init_chat_sender(lines, ptr_clone_user_receiver_channels.clone(), name).await?;
    Ok(())
}

enum ChannelMessage {
    Message {
        from: String,
        to: String,
        msg: String,
    },
    Close {
        name: String,
    },
}

async fn init_chat_sender(
    mut lines: Lines<BufReader<&TcpStream>>,
    ptr_clone_user_receiver_channels: Arc<Mutex<HashMap<String, UserChannel>>>,
    name: String,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    while let Some(line) = lines.next().await {
        let line = line?;
        println!("user {name} requests {line}");
        let (dest, msg) = match line.find(':') {
            None => continue,
            Some(idx) => (&line[..idx], line[idx + 1..].trim()),
        };
        let dests: Vec<_> = dest.split(',').map(|d| d.trim()).collect();
        let to = dests.join(", ");
        for dest in dests {
            println!("sending message to {dest}");
            if let Some(channel) = ptr_clone_user_receiver_channels.lock().await.get(dest) {
                println!("sending message to {dest}, channel obtained");
                channel
                    .sender
                    .send(ChannelMessage::Message {
                        from: name.to_string(),
                        to: to.clone(),
                        msg: msg.to_string(),
                    })
                    .await?;
            }
        }
        println!("sending channel finished");
    }
    if let Some(channel) = ptr_clone_user_receiver_channels.lock().await.get(&name) {
        channel
            .sender
            .send(ChannelMessage::Close {
                name: name.to_string(),
            })
            .await?;
    }
    Ok(())
}

async fn init_chat_receiver(
    stream: Arc<TcpStream>,
    ptr_clone_user_receiver_channels: Arc<Mutex<HashMap<String, UserChannel>>>,
    name: String,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    println!("init \"{name}\" receivers");
    let mut writer = BufWriter::new(&*stream);
    if let Some(user_receiver_channel) = ptr_clone_user_receiver_channels.lock().await.get(&name) {
        println!("\"{name}\" get channel");
        while let Ok(msg) = user_receiver_channel.receiver.recv().await {
            println!("\"{name}\" receivers");
            match msg {
                ChannelMessage::Message { from, to, msg } => {
                    println!("\"{from}\" send to \"{to}\" {msg}");
                    writer.write((from + &to + &msg).as_bytes());
                }
                ChannelMessage::Close { name } => {
                    println!("Closing channel for user {name}");
                    break;
                }
            }
        }
    }
    Ok(())
}
