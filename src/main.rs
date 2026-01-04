use crate::server::Server;
use tracing::error;
mod server;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let server = Server::new("127.0.0.1:8080");
    let r = server.listen().await;
    match r {
        Ok(_) => {}
        Err(err) => error!("Error creating the server {}", err),
    }
}
