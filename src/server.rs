use anyhow::Result;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    spawn,
};
use tracing::{error, info};

pub struct Server {
    pub address: String,
}

impl Server {
    pub fn new(address: &str) -> Server {
        Server {
            address: String::from(address),
        }
    }

    pub async fn listen(&self) -> Result<()> {
        let listener = TcpListener::bind(String::from(&self.address)).await?;
        info!("The server is running on {}", &self.address);
        loop {
            let (mut socket, addr) = listener.accept().await?;
            info!("New connection from {}", addr);
            spawn(async move {
                let mut buf = vec![0; 1024];
                loop {
                    let n = match socket.read(&mut buf).await {
                        Ok(0) => return,
                        Ok(n) => n,
                        Err(e) => {
                            error!("Failed to read from socket: {}", e);
                            return;
                        }
                    };
                    if socket.write_all(&buf[0..n]).await.is_err() {
                        return;
                    }
                }
            });
        }
    }
}
