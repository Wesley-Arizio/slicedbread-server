use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::net::SocketAddr;
use tokio::net::TcpListener;

use dotenvy::dotenv;

use crate::server::SliceBreadServer;

mod constants;
mod server;

use clap::{Parser, command};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Server address of the person to greet
    #[arg(long, env = "API_PORT")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    dotenv().ok();
    let args = Args::parse();
    let addr = SocketAddr::from(([127, 0, 0, 1], args.port));

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
