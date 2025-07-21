use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use tokio::net::TcpListener;

use crate::server::SliceBreadServer;

mod server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let listener = TcpListener::bind(addr).await?;
    println!("Listening on http://{}", addr);

    let server = SliceBreadServer {};

    loop {
        let (stream, _) = listener.accept().await?;
        let server = server.clone();
        let io = TokioIo::new(stream);
        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new().serve_connection(io, server).await {
                println!("Failed to serve connection: {:?}", err);
            }
        });
    }
}
