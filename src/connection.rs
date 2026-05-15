use std::collections::HashSet;
use std::fs;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration, Instant};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;

use crate::match_state::{
    end_room, notify, rooms_status, Match, Matches, RoomId, Rooms, CHANNEL_SIZE,
};

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    Message,
>;
type WsStream = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
>;

const HEARTBEAT_INTERVAL_SECS: u64 = 10;
const HEARTBEAT_TIMEOUT_SECS: u64 = 20;
const WAIT_OPPONENT_SECS: u64 = 70;
const LOBBY_MAX_MSGS: u32 = 100;

pub async fn handle_connection(stream: tokio::net::TcpStream, matches: Matches, rooms: Rooms) {
    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, mut read) = ws.split();

    {
        let r = rooms.lock().await;
        if write.send(rooms_status(&r).into()).await.is_err() {
            return;
        }
    }

    // Lobby: player picks an available room. Increments rooms[idx] on success.
    let room_id = match lobby_phase(&mut write, &mut read, &rooms).await {
        Some(r) => r,
        None => return,
    };

    let (tx, mut rx) = mpsc::channel::<String>(CHANNEL_SIZE);

    // Register in match. Determines player number from which slot is free.
    let player_number: u8 = {
        let mut lock = matches.lock().await;
        let m = lock
            .entry(room_id.as_str().to_string())
            .or_insert_with(Match::new);
        if m.player1.is_none() {
            m.player1 = Some(tx);
            1
        } else if m.player2.is_none() {
            m.player2 = Some(tx);
            2
        } else {
            // Extremely rare: rooms[idx] was < 2 but match is full due to a race.
            // Don't touch rooms count — we never incremented past 2.
            let _ = write.close().await;
            return;
        }
    };

    // Load word list. Both players call this; assign_anagram_id is idempotent.
    let mut words: HashSet<String> = HashSet::new();
    {
        let mut lock = matches.lock().await;
        if let Some(m) = lock.get_mut(room_id.as_str()) {
            m.assign_anagram_id();
            let path = format!("anagrams/{}.txt", m.anagram_id);
            match fs::read_to_string(&path) {
                Ok(content) => {
                    let parts: Vec<&str> = content.trim().split(',').collect();
                    if let Some(&first) = parts.first() {
                        m.anagram = first.to_string();
                    }
                    for w in parts.iter().skip(1) {
                        if !w.is_empty() {
                            words.insert(w.to_string());
                        }
                    }
                }
                Err(_) => {
                    end_room(room_id, &matches, &rooms).await;
                    let _ = write.close().await;
                    return;
                }
            }
        }
    }

    // Wait for opponent. Actively pings to detect silent disconnects.
    if !wait_for_opponent(&mut write, &mut read, room_id, &matches).await {
        end_room(room_id, &matches, &rooms).await;
        let _ = write.close().await;
        return;
    }

    let anagram = {
        let lock = matches.lock().await;
        lock.get(room_id.as_str())
            .map(|m| m.anagram.clone())
            .unwrap_or_default()
    };

    // This was previously .unwrap() — a panic that skipped cleanup.
    if write.send(format!("s:{}", anagram).into()).await.is_err() {
        end_room(room_id, &matches, &rooms).await;
        let _ = write.close().await;
        return;
    }

    // game_started flag inside try_start_timer ensures only one timer runs
    // even though both player tasks reach this point.
    {
        let mut lock = matches.lock().await;
        if let Some(m) = lock.get_mut(room_id.as_str()) {
            m.try_start_timer(room_id, Arc::clone(&matches));
        }
    }

    game_loop(
        &mut write,
        &mut read,
        &mut rx,
        room_id,
        player_number,
        &matches,
        &mut words,
    )
    .await;

    end_room(room_id, &matches, &rooms).await;
    let _ = write.close().await;
}

/// Returns Some(RoomId) when the client claims a slot, or None on disconnect/timeout.
/// Sends updated room status if the client picks a full room so they can retry.
async fn lobby_phase(
    write: &mut WsSink,
    read: &mut WsStream,
    rooms: &Rooms,
) -> Option<RoomId> {
    let mut msg_count = 0u32;

    loop {
        if msg_count > LOBBY_MAX_MSGS {
            return None;
        }
        msg_count += 1;

        let msg = match read.next().await {
            Some(Ok(m)) if m.is_text() => m.to_string(),
            _ => return None,
        };

        let mut lock = rooms.lock().await;

        if msg == "r" {
            let status = rooms_status(&lock);
            drop(lock);
            if write.send(status.into()).await.is_err() {
                return None;
            }
            continue;
        }

        if let Some(room) = RoomId::from_str(&msg) {
            let idx = room.index();
            if lock[idx] < 2 {
                lock[idx] += 1;
                return Some(room);
            }
            // Room full: send updated state so client can pick another.
            let status = rooms_status(&lock);
            drop(lock);
            let _ = write.send(status.into()).await;
        }
        // Unknown message: ignore.
    }
}

/// Polls until both players have joined, or the deadline/disconnect is hit.
/// Sends pings so silent TCP drops are detected within HEARTBEAT_TIMEOUT_SECS.
async fn wait_for_opponent(
    write: &mut WsSink,
    read: &mut WsStream,
    room_id: RoomId,
    matches: &Matches,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(WAIT_OPPONENT_SECS);
    let mut last_pong = Instant::now();
    let mut ping_tick = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let mut poll_tick = interval(Duration::from_millis(250));

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Pong(_))) => { last_pong = Instant::now(); }
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return false,
                    _ => {}
                }
            }
            _ = ping_tick.tick() => {
                if last_pong.elapsed() > Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
                    return false;
                }
                if write.send(Message::Ping(vec![])).await.is_err() {
                    return false;
                }
            }
            _ = poll_tick.tick() => {
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
                let lock = matches.lock().await;
                match lock.get(room_id.as_str()) {
                    Some(m) if m.player1.is_some() && m.player2.is_some() => return true,
                    None => return false,
                    _ => {}
                }
            }
        }
    }
}

async fn game_loop(
    write: &mut WsSink,
    read: &mut WsStream,
    rx: &mut mpsc::Receiver<String>,
    room_id: RoomId,
    player_number: u8,
    matches: &Matches,
    words: &mut HashSet<String>,
) {
    let mut heartbeat = interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
    let mut last_pong = Instant::now();

    loop {
        tokio::select! {
            msg = read.next() => {
                let msg = match msg {
                    Some(Ok(m)) => m,
                    _ => break,
                };
                match msg {
                    Message::Pong(_) => {
                        last_pong = Instant::now();
                    }
                    Message::Close(_) => {
                        notify_opponent(room_id, player_number, matches, "f:x".into()).await;
                        break;
                    }
                    Message::Text(text) => {
                        if !handle_guess(&text, write, room_id, player_number, matches, words).await {
                            break;
                        }
                    }
                    Message::Ping(data) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    _ => {}
                }
            }
            msg = rx.recv() => {
                let msg = match msg {
                    Some(m) => m,
                    None => break,
                };
                let is_final = msg.starts_with("f:");
                if write.send(msg.into()).await.is_err() {
                    break;
                }
                if is_final {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() > Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
                    notify_opponent(room_id, player_number, matches, "f:x".into()).await;
                    break;
                }
                if write.send(Message::Ping(vec![])).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn notify_opponent(room_id: RoomId, player_number: u8, matches: &Matches, msg: String) {
    let lock = matches.lock().await;
    if let Some(m) = lock.get(room_id.as_str()) {
        let opponent = if player_number == 1 { &m.player2 } else { &m.player1 };
        notify(opponent, msg).await;
    }
}

/// Handles a "g:word" guess message. Returns false if the connection should close.
async fn handle_guess(
    text: &str,
    write: &mut WsSink,
    room_id: RoomId,
    player_number: u8,
    matches: &Matches,
    words: &mut HashSet<String>,
) -> bool {
    let guess = match text.strip_prefix("g:") {
        Some(g) if !g.is_empty() => g,
        _ => return true,
    };

    let (my_pts, opponent_tx) = {
        let mut lock = matches.lock().await;
        let m = match lock.get_mut(room_id.as_str()) {
            Some(m) => m,
            None => return false,
        };

        if words.remove(guess) {
            if player_number == 1 {
                m.player_pts_1 += guess.len();
                (m.player_pts_1, m.player2.clone())
            } else {
                m.player_pts_2 += guess.len();
                (m.player_pts_2, m.player1.clone())
            }
        } else {
            return write.send("w:".into()).await.is_ok();
        }
    }; // lock dropped before awaiting

    if write.send(format!("p:{}", my_pts).into()).await.is_err() {
        return false;
    }

    if let Some(tx) = opponent_tx {
        if tx.send(format!("o:{}", my_pts)).await.is_err() {
            let _ = write.send("f:x".into()).await;
            return false;
        }
    }
    true
}
