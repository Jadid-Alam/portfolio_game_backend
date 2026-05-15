mod connection;
mod match_state;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use connection::handle_connection;
use match_state::{Matches, Rooms};

#[tokio::main]
async fn main() {
    let matches: Matches = Arc::new(Mutex::new(HashMap::new()));
    let rooms: Rooms = Arc::new(Mutex::new([0u8; 4]));

    let listener = TcpListener::bind("127.0.0.1:8080").await.unwrap();

    while let Ok((stream, _)) = listener.accept().await {
        let matches = Arc::clone(&matches);
        let rooms = Arc::clone(&rooms);
        tokio::spawn(handle_connection(stream, matches, rooms));
    }
}
