# 1v1 Scramble Game Backend

## Overview
The 1v1 Scramble Game Backend is a high-performance real-time multiplayer game server developed in Rust. It utilizes WebSockets for ultra-low latency gameplay and is hosted on AWS for scalability. The backend efficiently manages game sessions, player interactions, and synchronization to provide a seamless gaming experience.

## Features
- **Real-time Gameplay:** Uses WebSockets to ensure instant communication between players.
- **Ultra-Low Latency:** Optimized for high-speed interactions.
- **AWS Hosting:** Designed for scalability and reliability.
- **Matchmaking System:** Handles game sessions dynamically (No skill-based matchmaking).
- **Automatic Match Timeout:** Ends matches after 66 seconds if unresolved.
- **Heartbeat Mechanism:** Ensures active player connections.
- **Word Scramble Logic:** Manages word anagrams for gameplay.
- **Graceful Disconnection Handling:** Notifies opponents in case of disconnection.

## Technology Stack
- **Language:** Rust
- **Networking:** WebSockets (via `tokio-tungstenite`)
- **Concurrency:** `tokio`, `futures`
- **Storage:** In-memory HashMap for session management
- **Deployment:** Hosted on AWS

## Game Logic
- Players enter a matchmaking queue and are assigned to one of four rooms (`a`, `b`, `c`, `d`).
- Each match consists of two players competing to unscramble words.
- Players earn points by correctly identifying words.
- At the end of the timer, the player with the highest score wins.
- If a player disconnects, the opponent is notified.

## Installation & Running the Server
### Prerequisites
- Rust installed (`cargo` and `rustc`)
- WebSocket-compatible client for testing

### Steps
1. Clone the repository:
    git clone <repo-url>
    cd scramble-game-backend
2. Build and run the server:
    cargo run
3. The server will start listening on port `8080`.

## API Communication
- **Available Rooms**: Sent to client as `a:<room_status>`
- **Start Game**: Sent as `s:<anagram>`
- **Points Update**: Player receives `p:<points>`, opponent receives `o:<points>`
- **Game Over Signals**:
  - `f:u` - Player wins
  - `f:o` - Opponent wins
  - `f:d` - Draw
  - `f:x` - Opponent disconnected

## Future Improvements
- Persistent storage for game history
- Web-based frontend integration

## License
This is a solo project and is not licensed under any specific open-source license.


For any queries or feedback, feel free to reach out!