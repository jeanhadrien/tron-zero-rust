// Wire types of the lobby API. Field names are a contract with the Rust
// server (crates/server/src/announce.rs) and client (crates/client/src/browser.rs).
export interface RoomEntry {
  id: string; // manager-generated UUID
  host: string;
  port: number;
  secure?: boolean; // game server is reachable over TLS
  webPort?: number; // WebTransport (QUIC) UDP port for browser clients; absent = native only
  certHash?: string; // hex SHA-256 of a self-signed WebTransport cert; absent = CA-validated cert
  displayName: string;
  playerCount: number; // humans only, updated via heartbeat; maxPlayers caps humans
  botCount?: number; // additive and informational; absent for servers that do not report it
  maxPlayers: number; // set at register
  lastHeartbeat: number; // Date.now() of last register/heartbeat
  ranked?: boolean; // results from this room move ratings; joining needs a ticket
  official?: boolean; // ranked by an official key: may also move bot ratings
}

// POST /api/rooms response: the public entry plus the room's secret. The token
// is returned exactly once and never appears in GET /api/rooms.
export interface RegisterResponse extends RoomEntry {
  token: string;
  // Ranked rooms only: the key's stable id (tickets are bound to it) and the
  // keys to verify tickets with.
  keyId?: string;
  publicKeys?: { kid: string; publicKey: string }[];
}

// POST /api/rooms
export interface RegisterPayload {
  host: string;
  port: number;
  secure?: boolean;
  webPort?: number;
  certHash?: string;
  displayName: string;
  maxPlayers: number;
  rankedKey?: string; // `<keyId>.<secret>`, presented once to register as ranked
}

// POST /api/rooms/:id/heartbeat
export interface HeartbeatPayload {
  playerCount: number;
  botCount?: number;
  // Ranked rooms: who is connected, plus tickets of newly admitted players.
  roster?: { humans: string[]; tickets: string[] };
}
