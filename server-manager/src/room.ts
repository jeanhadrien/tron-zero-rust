// Wire types of the lobby API. Field names are a contract with the Rust
// server (crates/server/src/announce.rs) and client (crates/client/src/browser.rs).
export interface RoomEntry {
  id: string; // manager-generated UUID
  host: string;
  port: number;
  secure?: boolean; // game server is reachable over TLS
  displayName: string;
  playerCount: number; // updated via heartbeat
  maxPlayers: number; // set at register
  lastHeartbeat: number; // Date.now() of last register/heartbeat
}

// POST /api/rooms
export interface RegisterPayload {
  host: string;
  port: number;
  secure?: boolean;
  displayName: string;
  maxPlayers: number;
}

// POST /api/rooms/:id/heartbeat
export interface HeartbeatPayload {
  playerCount: number;
}
