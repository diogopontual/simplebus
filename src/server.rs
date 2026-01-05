use anyhow::Result;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    net::{TcpListener, TcpStream},
    spawn,
};
use tracing::{error, info};

pub struct Event {
    topic: String,
    payload: Vec<u8>,
}
pub struct Server {
    pub address: String,
}

impl Server {
    pub fn new(address: &str) -> Server {
        Server {
            address: String::from(address),
        }
    }

    async fn read_event(socket: &mut TcpStream) -> Result<Event> {
        let mut reader = BufReader::new(socket);
        let mut buf = vec![0u8; 1024];
        let bytes_read = reader.read_until(b'\n', &mut buf).await?;
        let topic = std::str::from_utf8(&buf[..bytes_read])?
            .trim_end_matches(&['\r', '\n'][..])
            .to_string();
        println!("Bytes Read: {} , Topic: {}", bytes_read, topic);
        let mut length_bytes = [0u8; 4];
        let _ = reader.read_exact(&mut length_bytes).await;
        let length = u32::from_be_bytes(length_bytes);
        let mut payload = vec![0u8; length as usize];
        let _ = reader.read_exact(&mut payload).await?;
        Ok(Event { topic, payload })
    }

    pub async fn listen(&self) -> Result<()> {
        let listener = TcpListener::bind(String::from(&self.address)).await?;
        info!("The server is running on {}", &self.address);
        loop {
            let (mut socket, addr) = listener.accept().await?;
            info!("New connection from {}", addr);
            spawn(async move {
                loop {
                    let event_result = Server::read_event(&mut socket).await;
                    match event_result {
                        Ok(event) => {
                            println!("{}", event.topic);
                        }
                        Err(err) => {
                            error!("Error reading event from sockect: {}", err);
                            break;
                        }
                    }
                }
            });
        }
    }
}
