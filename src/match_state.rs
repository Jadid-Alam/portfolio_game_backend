use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::time::Duration;
use tokio_util::sync::CancellationToken;

pub const CHANNEL_SIZE: usize = 10;
pub const GAME_DURATION_SECS: u64 = 66;

pub type Matches = Arc<Mutex<HashMap<String, Match>>>;
pub type Rooms = Arc<Mutex<[u8; 4]>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoomId {
    A = 0,
    B = 1,
    C = 2,
    D = 3,
}

impl RoomId {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "a" => Some(Self::A),
            "b" => Some(Self::B),
            "c" => Some(Self::C),
            "d" => Some(Self::D),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
            Self::C => "c",
            Self::D => "d",
        }
    }
}

#[derive(Debug)]
pub struct Match {
    pub player1: Option<mpsc::Sender<String>>,
    pub player2: Option<mpsc::Sender<String>>,
    pub player_pts_1: usize,
    pub player_pts_2: usize,
    pub anagram: String,
    pub anagram_id: usize,
    timer: CancellationToken,
    game_started: bool,
}

impl Match {
    pub fn new() -> Self {
        Self {
            player1: None,
            player2: None,
            player_pts_1: 0,
            player_pts_2: 0,
            anagram: String::new(),
            anagram_id: 0,
            timer: CancellationToken::new(),
            game_started: false,
        }
    }

    /// Starts the game timer exactly once; subsequent calls are no-ops.
    pub fn try_start_timer(&mut self, room_id: RoomId, matches: Matches) {
        if self.game_started {
            return;
        }
        self.game_started = true;
        let token = self.timer.clone();
        let key = room_id.as_str().to_string();

        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(GAME_DURATION_SECS)) => {
                    let lock = matches.lock().await;
                    if let Some(m) = lock.get(&key) {
                        let (msg1, msg2) = outcome_messages(m.player_pts_1, m.player_pts_2);
                        notify(&m.player1, msg1).await;
                        notify(&m.player2, msg2).await;
                    }
                }
                _ = token.cancelled() => {}
            }
        });
    }

    pub fn cancel_timer(&self) {
        self.timer.cancel();
    }

    pub fn assign_anagram_id(&mut self) {
        if self.anagram_id == 0 {
            self.anagram_id = rand::random::<usize>() % 50 + 1;
        }
    }
}

pub fn outcome_messages(pts1: usize, pts2: usize) -> (String, String) {
    match pts1.cmp(&pts2) {
        std::cmp::Ordering::Greater => ("f:u".into(), "f:o".into()),
        std::cmp::Ordering::Less => ("f:o".into(), "f:u".into()),
        std::cmp::Ordering::Equal => ("f:d".into(), "f:d".into()),
    }
}

pub async fn notify(tx: &Option<mpsc::Sender<String>>, msg: String) {
    if let Some(sender) = tx {
        let _ = sender.send(msg).await;
    }
}

pub fn rooms_status(rooms: &[u8; 4]) -> String {
    format!("a:{}{}{}{}", rooms[0], rooms[1], rooms[2], rooms[3])
}

/// Resets a room slot to 0 and removes its match entry.
/// Safe to call multiple times; second call is a no-op.
pub async fn end_room(room: RoomId, matches: &Matches, rooms: &Rooms) {
    {
        let mut r = rooms.lock().await;
        r[room.index()] = 0;
    }
    let mut m = matches.lock().await;
    if let Some(entry) = m.get(room.as_str()) {
        entry.cancel_timer();
    }
    m.remove(room.as_str());
}
