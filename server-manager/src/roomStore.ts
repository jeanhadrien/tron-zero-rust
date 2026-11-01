import { createHash, timingSafeEqual } from 'crypto';
import type { RoomEntry, RegisterPayload } from './room';

// A room plus the SHA-256 of its secret token. Hashing gives fixed-length
// buffers, so the comparison is constant-time; the plain token is never kept.
export interface StoredRoom {
  entry: RoomEntry;
  tokenHash: Buffer;
}

// Plain in-memory store; rooms vanish on restart and servers re-register on 404.
const rooms = new Map<string, StoredRoom>();

export const MAX_ROOMS = 200;

const hashToken = (token: string): Buffer => createHash('sha256').update(token).digest();

// Hostnames are case-insensitive; compare accordingly.
const sameEndpoint = (room: RoomEntry, host: string, port: number): boolean =>
  room.port === port && room.host.toLowerCase() === host.toLowerCase();

export function isFull(): boolean {
  return rooms.size >= MAX_ROOMS;
}

export function findByEndpoint(host: string, port: number): StoredRoom | undefined {
  for (const room of rooms.values()) {
    if (sameEndpoint(room.entry, host, port)) return room;
  }
  return undefined;
}

export function isAuthorized(room: StoredRoom, token: string | undefined): boolean {
  return token !== undefined && timingSafeEqual(room.tokenHash, hashToken(token));
}

// Replaces any room on the same host:port; the caller has already checked authorization.
export function registerRoom(payload: RegisterPayload, id: string, token: string): RoomEntry {
  const existing = findByEndpoint(payload.host, payload.port);
  if (existing) rooms.delete(existing.entry.id);

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
  rooms.set(id, { entry, tokenHash: hashToken(token) });
  return entry;
}

export function removeRoom(id: string): boolean {
  return rooms.delete(id);
}

export function getRoom(id: string): StoredRoom | undefined {
  return rooms.get(id);
}

// Public view only: entries never carry the token or its hash.
export function getAllRooms(): RoomEntry[] {
  return Array.from(rooms.values(), (room) => room.entry);
}

// Evict rooms that haven't heartbeated within the given timeout
export function evictStale(maxAgeMs: number): number {
  const now = Date.now();
  let removed = 0;
  for (const [id, room] of rooms) {
    if (now - room.entry.lastHeartbeat > maxAgeMs) {
      rooms.delete(id);
      removed++;
    }
  }
  return removed;
}
