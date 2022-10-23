use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use async_std::channel::{Receiver, Sender};
use async_std::io::prelude::BufReadExt;
use async_std::io::{BufReader, WriteExt};
use async_std::net::{TcpListener, TcpStream};
use async_std::stream::StreamExt;
use async_std::task::JoinHandle;
use async_std::{channel, task};

fn main() {
    let addr = "127.0.0.1:8080";
    if let Err(err) = task::block_on(init_listener(addr)) {
        eprintln!("{}", err);
    }
}

fn spawn_and_log_error<F>(fut: F) -> JoinHandle<()>
where
    F: Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send + 'static,
{
    task::spawn(async move {
        if let Err(e) = fut.await {
            eprintln!("{e}");
        }
    })
}

async fn init_listener(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(addr).await?;
    println!("tcplistener initialized, listening on {addr}");
    let (sender, receiver) = channel::unbounded();
    let handle_user_streams_dispatcher = spawn_and_log_error(user_streams_dispatcher(receiver));
    while let Some(tcp_stream) = listener.incoming().next().await {
        let tcp_stream = tcp_stream?;
        let tcp_stream = Arc::new(tcp_stream);
        let sender_clone = sender.clone();
        let tcp_stream_clone = tcp_stream.clone();
        spawn_and_log_error(user_thread(tcp_stream_clone, sender_clone));
    }
    drop(sender);
    handle_user_streams_dispatcher.await;
    Ok(())
}

async fn user_thread(
    stream: Arc<TcpStream>,
    sender: Sender<ChannelMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    spawn_and_log_error(write_to_stream(
        stream.clone(),
        "Welcome! Please enter your name:\r\n".to_string(),
    ));
    let stream_clone = stream.clone();
    let reader = BufReader::new(&*stream_clone);
    let mut lines = reader.lines();
    let username;
    let name = lines.next().await.expect("need username");
    username = name?;
    if let Err(err) = sender
        .send(ChannelMessage::NewUser {
            username: username.clone(),
            stream,
        })
        .await
    {
        println!("{}", err);
    };
    while let Some(msg) = lines.next().await {
        let msg = msg?;
        match msg.as_str() {
            "quit" => {
                sender
                    .send(ChannelMessage::Close {
                        username: username.clone(),
                    })
                    .await?;
                break;
            }
            _ => {
                if let Some(splitter_index) = msg.find(":") {
                    sender
                        .send(ChannelMessage::Message {
                            from: username.clone(),
                            to: msg[0..splitter_index].to_string(),
                            content: msg[splitter_index + 1..].to_string(),
                        })
                        .await?;
                }
            }
        }
    }
    sender.send(ChannelMessage::Close { username }).await?;
    Ok(())
}

#[derive(Debug)]
enum ChannelMessage {
    NewUser {
        username: String,
        stream: Arc<TcpStream>,
    },
    Close {
        username: String,
    },
    Message {
        from: String,
        to: String,
        content: String,
    },
}

async fn user_streams_dispatcher(
    receiver: Receiver<ChannelMessage>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut user_streams: HashMap<String, Arc<TcpStream>> = HashMap::new();
    let mut writers = Vec::new();
    while let Ok(msg) = receiver.recv().await {
        match msg {
            ChannelMessage::NewUser { username, stream } => match user_streams.entry(username) {
                Entry::Occupied(_) => {
                    println!("name exists")
                }
                Entry::Vacant(entry) => {
                    println!("{} logged in", entry.key());
                    writers.push(spawn_and_log_error(write_to_stream(
                        stream.clone(),
                        format!("{} logged in as {}\r\n", stream.peer_addr()?, entry.key()),
                    )));
                    entry.insert(stream);
                }
            },
            ChannelMessage::Close { username } => {
                println!("{username} quits");
                user_streams.remove(&username);
            }
            ChannelMessage::Message { from, to, content } => {
                for t in to.split(",") {
                    if let Some(stream) = user_streams.get(&t.to_string()) {
                        writers.push(spawn_and_log_error(write_to_stream(
                            stream.clone(),
                            format!("{}:{}\r\n", from, content),
                        )));
                    } else {
                        if let Some(stream) = user_streams.get(&from) {
                            writers.push(spawn_and_log_error(write_to_stream(
                                stream.clone(),
                                format!("user \"{}\" not online.\r\n", t),
                            )));
                        }
                    }
                }
            }
        };
        let userlist = user_streams
            .keys()
            .map(|k| k.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        println!("userlist: {}", userlist);
    }
    drop(user_streams);
    for writer in writers {
        writer.await;
    }
    Ok(())
}

async fn write_to_stream(
    stream: Arc<TcpStream>,
    content: String,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    (&*stream).write_all(content.as_bytes()).await?;
    Ok(())
}
