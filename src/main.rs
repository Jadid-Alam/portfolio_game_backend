/* ************************************************
Author:                                 Jadid Alam
Date:                                   09/03/2025
Version:                                       1.1

Description: Rust-based multiplayer game server
handling game logic for 1v1 Scramble, featured in
my portfolio. Designed for high performance,
scalability, and secure real-time gameplay
synchronization.
Notes:
f:_ = finished, f:o = opp wins. f:u = user wins,
f:d = draw, f:x = opponent disconnected,
p:_ = client earned points, o:_ opponent earned
points, s:_ = start game, a:_ = available rooms
*************************************************** */


use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use tokio::time::{interval, Duration, Instant};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;


type MatchID = String;
const CHANNEL_SIZE: usize = 10;

#[derive(Debug)]
struct Match {
    player1: Option<mpsc::Sender<String>>,
    player2: Option<mpsc::Sender<String>>,
    player_pts_1: usize,
    player_pts_2: usize,
    anagram: String,
    anagram_id: usize,
}

impl Match {
    fn start_timer(&self, match_id: String, matches: Matches) {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(66)).await; // Wait 60s (game time) + 6s (getting ready)
            let matches_lock = matches.lock().await;
            if let Some(game_match) = matches_lock.get(&match_id) {
                let player_pts_1 = game_match.player_pts_1;
                let player_pts_2 = game_match.player_pts_2;
                let (msg_p1, msg_p2) = if player_pts_1 > player_pts_2 {
                    ("f:u".to_string(), "f:o".to_string())
                } else if player_pts_2 > player_pts_1 {
                    ("f:o".to_string(), "f:u".to_string())
                } else {
                    ("f:d".to_string(), "f:d".to_string())
                };

                if let Some(tx) = &game_match.player1 {
                    let _ = tx.send(msg_p1).await.is_err();
                }

                if let Some(tx) = &game_match.player2 {
                    let _ = tx.send(msg_p2).await.is_err();
                }
            }
            drop(matches_lock);
        });
    }
    fn rng_word(&mut self) {
        if self.anagram_id == 0 {
            let mut rng = rand::thread_rng();
            self.anagram_id = rng.gen_range(1..=49);
        }

    }
}

type Matches = Arc<Mutex<HashMap<MatchID, Match>>>;
type Available = Arc<Mutex<[u8; 4]>>;


async fn handle_connection(stream: tokio::net::TcpStream, matches: Matches, available: Available) {
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("WebSocket handshake failed: {}", e);
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();

    // Sending available rooms.
    let available_lock = available.lock().await;
    if write.send(format!("a:{}{}{}{}", available_lock[0],available_lock[1],available_lock[2],available_lock[3]).into()).await.is_err() {
        eprintln!("Failed to send match request message.");
        return;
    }
    drop(available_lock);

    let mut player_number = 0;
    let mut match_id = String::new();

    let mut max_count_disconnect = 0;
    // when client asks for it, send data of available rooms until the client picks a room. Also disconnects client if they are connected for more than 60s without choosing a match
    loop {
        match_id = match read.next().await {
            Some(Ok(msg)) if msg.is_text() => {
                let id = msg.to_string();
                let mut locked_array = available.lock().await;
                match id.as_str() {
                    "a" => if locked_array[0] < 2 {
                        locked_array[0] += 1;
                        player_number = locked_array[0];
                    },
                    "b" => if locked_array[1] < 2 {
                        locked_array[1] += 1;
                        player_number = locked_array[1];
                    },
                    "c" => if locked_array[2] < 2 {
                        locked_array[2] += 1;
                        player_number = locked_array[2];
                    },
                    "d" => if locked_array[3] < 2 {
                        locked_array[3] += 1;
                        player_number = locked_array[3];
                    },
                    "r" => {
                        max_count_disconnect += 1; // each r message comes in every 5s so 6 = 30s
                        if write.send(format!("a:{}{}{}{}", locked_array[0],locked_array[1],locked_array[2],locked_array[3]).into()).await.is_err() {
                            eprintln!("Failed to send match request message.");
                            return;
                        }
                    },
                    _ => println!("Invalid input {}",msg),
                }
                drop(locked_array);
                id
            },
            _ => {
                eprintln!("Failed to receive match ID.");
                return;
            }
        };
        if match_id == "a" || match_id == "b" || match_id == "c" || match_id == "d"{
            break;
        }
        if max_count_disconnect > 6 {
            let m1 = Arc::clone(&matches);
            let a1 = Arc::clone(&available);
            disconnect_player(&match_id,m1,a1).await;
            // disconnect client
            let _ = write.close().await;
            return;
        }
    }

    let (tx, mut rx) = mpsc::channel::<String>(CHANNEL_SIZE);

    // Assigning players correctly. Becasue of how mutex behaves first clint who chose a room will always have player_no = 1
    {
        let mut matches_lock = matches.lock().await;
        let game_match = matches_lock.entry(match_id.clone()).or_insert(Match {
            player1: None,
            player2: None,
            player_pts_1: 0,
            player_pts_2: 0,
            anagram: String::from(""),
            anagram_id: 0,
        });

        if game_match.player1.is_none() {
            game_match.player1 = Some(tx.clone());
        } else if game_match.player2.is_none() {
            game_match.player2 = Some(tx.clone());
        } else {
            eprintln!("Match {} is already full!", match_id);
            return;
        }
        drop(matches_lock);
    }


    // Setting the words and making the hashmap for the game logic
    let mut words = HashMap::new();
    let mut matches_lock = matches.lock().await;
    if let Some(game_match) = matches_lock.get_mut(&match_id) {
        game_match.rng_word();
        let file_name = String::from("anagrams/") + &game_match.anagram_id.to_string() + ".txt";
        let content = fs::read_to_string(file_name).expect("Failed to read file");
        let values: Vec<String> = content.trim().split(',').map(|s| s.to_string()).collect();
        game_match.anagram = values.get(0).expect("File is empty?").to_string();
        for index in 1..values.len() {
            words.insert(values.get(index).expect("File is empty?").to_string(), 0);
        }
    };
    drop(matches_lock);

    // Seeing if both players have joined
    max_count_disconnect = 0;
    loop {
        let matches_lock = matches.lock().await;
        if let Some(game_match) = matches_lock.get(&match_id) {
            if game_match.player1.is_some() && game_match.player2.is_some() {
                write.send(format!("s:{}", game_match.anagram).into()).await.unwrap();
                game_match.start_timer(match_id.clone(),Arc::clone(&matches));
                break; // Exit the loop once both players have joined
            }
        }
        if max_count_disconnect > 140 { // 70s, 1 = 0.5s so we wait for 70s before kicking out the client to avoid zombie clients
            let m1 = Arc::clone(&matches);
            let a1 = Arc::clone(&available);
            disconnect_player(&match_id,m1,a1).await;
            // disconnect client
            let _ = write.close().await;
            return;
        }
        drop(matches_lock);
        max_count_disconnect += 1;
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    }

    // heartbeat makes sure that the client is still connected
    let mut heartbeat_interval = interval(Duration::from_secs(10));
    let mut is_alive = Instant::now();
    // Handle incoming messages and game logic
    loop {
        tokio::select! {
            Some(msg) = read.next() => {
                if let Ok(msg) = msg {
                    match msg {
                        Message::Pong(_) => {
                            is_alive = Instant::now(); // Update last pong time
                            continue; // no need to send the pong message to the game logic
                        },
                        Message::Close(_) => {
                            let mut matches_lock = matches.lock().await;
                            if let Some(game_match) = matches_lock.get_mut(&match_id) {
                                // making sure the opponent knows that the client has disconnected
                                let opponent;
                                if player_number == 1 {
                                    opponent = &game_match.player2;
                                } else {
                                    opponent = &game_match.player1;
                                }
                                if let Some(opponent_tx) = opponent {
                                    if opponent_tx.send("f:x".to_string()).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            drop(matches_lock);
                            break;
                        },
                        _ => {}
                    }
                    // Game logic Implementation:
                    let mut matches_lock = matches.lock().await;
                    if let Some(game_match) = matches_lock.get_mut(&match_id) {
                        // getting client guessed words
                        if &msg.to_string()[0..2] == "g:" {
                            let s = &msg.to_string()[2..];
                            if words.contains_key(s) {
                                // send client and opponent confimation that client has gained points
                                if player_number == 1 {
                                    game_match.player_pts_1 += s.len();
                                    if write.send(format!("p:{}", game_match.player_pts_1).into()).await.is_err() {
                                        eprintln!("Failed to send answer message.");
                                    }
                                    let opponent = &game_match.player2;
                                    if let Some(opponent_tx) = opponent {
                                        if opponent_tx.send(format!("o:{}", game_match.player_pts_1)).await.is_err() {
                                            if write.send("f:x".into()).await.is_err() {
                                                println!("Failed to send disconnect message message.");
                                            }
                                            break;
                                        }
                                    }
                                }
                                else {
                                    game_match.player_pts_2 += s.len();
                                    if write.send(format!("p:{}", game_match.player_pts_2).into()).await.is_err() {
                                        eprintln!("Failed to send answer message.");
                                    }
                                    let opponent = &game_match.player1;
                                    if let Some(opponent_tx) = opponent {
                                        if opponent_tx.send(format!("o:{}", game_match.player_pts_2)).await.is_err() {
                                            if write.send("f:x".into()).await.is_err() {
                                                println!("Failed to send disconnect message message.");
                                            }
                                            break;
                                        }
                                    }
                                }
                                words.remove(s);
                            } else {
                                // if client gets the answer wrong, they get their un saved points. (tells client they didn't get it correct)
                                if player_number == 1 {
                                    if write.send(format!("p:{}", game_match.player_pts_1).into()).await.is_err() {
                                        eprintln!("Failed to send answer message.");
                                    }
                                }
                                else {
                                    if write.send(format!("p:{}", game_match.player_pts_2).into()).await.is_err() {
                                        eprintln!("Failed to send answer message.");
                                    }
                                }
                            }

                        } else { println!("Failed to receive guess message."); }
                    }
                    drop(matches_lock);
                }
            },
            Some(msg) = rx.recv() => {
                // after receiving the finish message from the internal server it sends to the client.
                if &msg.to_string()[0..2] == "f:" {
                    if write.send(msg.into()).await.is_err() {}
                    break;
                } else if write.send(msg.into()).await.is_err() {
                    break;
                }
            },
            _ = heartbeat_interval.tick() => {
                // ping pong logic:
                if is_alive.elapsed() > Duration::from_secs(20) {
                    let mut matches_lock = matches.lock().await;
                    if let Some(game_match) = matches_lock.get_mut(&match_id) {
                        let opponent;
                        if player_number == 1 {
                            opponent = &game_match.player2;
                        } else {
                            opponent = &game_match.player1;
                        }
                        if let Some(opponent_tx) = opponent {
                            if opponent_tx.send(format!("f:x")).await.is_err() {
                                break;
                            }
                        }
                    }
                    drop(matches_lock);
                    break;
                }
                if write.send(Message::Ping(vec![])).await.is_err() {
                    println!("Failed to send ping. Closing connection.");
                    break;
                }
            },
        }
    }

    // Disconnecting player after the match has finished
    let m1 = Arc::clone(&matches);
    let a1 = Arc::clone(&available);
    disconnect_player(&match_id,m1,a1).await;
    // disconnect client
    let _ = write.close().await;
    return;

}

async fn disconnect_player(match_id: &str, matches: Matches, available: Available) {
    let mut matches_lock = matches.lock().await;
    if let Some(_game_match) = matches_lock.get_mut(match_id) {
        let mut locked_array = available.lock().await;
        match match_id {
            "a" => if locked_array[0] > 0 { locked_array[0] = 0; },
            "b" => if locked_array[1] > 0 { locked_array[1] = 0; },
            "c" => if locked_array[2] > 0 { locked_array[2] = 0; },
            "d" => if locked_array[3] > 0 { locked_array[3] = 0; },
            _ => println!("Invalid input"),
        }
        drop(locked_array);
        if match_id == "a" || match_id == "b" || match_id == "c" || match_id == "d"{
            matches_lock.remove(match_id);
        }

        drop(matches_lock);
    } else {
        eprintln!("Failed to find the match");
    }
}


#[tokio::main]
async fn main() {
    let matches = Arc::new(Mutex::new(HashMap::new()));
    let available = Arc::new(Mutex::new([0,0,0,0]));

    let listener = TcpListener::bind("0.0.0.0:8080").await.unwrap();

    while let Ok((stream, _)) = listener.accept().await {
        let matches = Arc::clone(&matches);
        let available = Arc::clone(&available);
        tokio::spawn(handle_connection(stream, matches, available));
    }
}

