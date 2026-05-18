use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::time::Duration;

pub struct AsyncWebServer {
    listener: TcpListener,
    max_connections: usize,
    connection_timeout: Duration,
}

impl AsyncWebServer {
    pub async fn new(addr: &str, max_connections: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind(addr).await?;

        Ok(AsyncWebServer {
            listener,
            max_connections,
            connection_timeout: Duration::from_secs(30),
        })
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        println!("Server listening on {}", self.listener.local_addr()?);

        let mut connection_count = 0;

        loop {
            let (socket, addr) = self.listener.accept().await?;

            if connection_count >= self.max_connections {
                eprintln!("Connection limit reached, rejecting: {}", addr);
                continue;
            }

            connection_count += 1;

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(socket, addr).await {
                    eprintln!("Error handling {}: {}", addr, e);
                }
            });
        }
    }

    async fn handle_connection(mut socket: TcpStream, addr: std::net::SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
        let mut buffer = [0; 4096];

        loop {
            let n = tokio::time::timeout(
                Duration::from_secs(30),
                socket.read(&mut buffer)
            ).await??;

            if n == 0 {
                break;
            }

            // Simple HTTP response
            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 13\r\n\r\nHello, World!";
            socket.write_all(response).await?;
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = AsyncWebServer::new("127.0.0.1:8080", 10000).await?;
    server.run().await
}