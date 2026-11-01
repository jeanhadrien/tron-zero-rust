import type { RoomEntry, RegisterPayload } from './room';

// Plain in-memory store; rooms vanish on restart and servers re-register on 404.
const rooms = new Map<string, RoomEntry>();

export const MAX_ROOMS = 200;

export function isFull(): boolean {
  return rooms.size >= MAX_ROOMS;
}

// True if registering this host:port would replace an existing room (so it never grows the store).
export function hasEndpoint(host: string, port: number): boolean {
  for (const room of rooms.values()) {
    if (room.host === host && room.port === port) return true;
  }
  return false;
}

export function registerRoom(payload: RegisterPayload, id: string): RoomEntry {
  // Evict any existing room with the same host:port to prevent duplicates on restart
  for (const [existingId, room] of rooms) {
    if (room.host === payload.host && room.port === payload.port) {
      rooms.delete(existingId);
      break;
    }
  }

  const entry: RoomEntry = {
    id,
    host: payload.host,
    port: payload.port,
    secure: payload.secure,
    displayName: payload.displayName,
    maxPlayers: payload.maxPlayers,
    playerCount: 0,
    lastHeartbeat: Date.now(),
  };
  rooms.set(id, entry);
  return entry;
}

export function removeRoom(id: string): boolean {
  return rooms.delete(id);
}

export function getRoom(id: string): RoomEntry | undefined {
  return rooms.get(id);
}

export function getAllRooms(): RoomEntry[] {
  return Array.from(rooms.values());
}

// Evict rooms that haven't heartbeated within the given timeout
export function evictStale(maxAgeMs: number): number {
  const now = Date.now();
  let removed = 0;
  for (const [id, room] of rooms) {
    if (now - room.lastHeartbeat > maxAgeMs) {
      rooms.delete(id);
      removed++;
    }
  }
  return removed;
}
