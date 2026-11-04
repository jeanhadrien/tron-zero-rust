# Tron Zero Rust — Style Spec

Locked from quiz with JH (2026-10-03). Code style only; architecture rules
(determinism, system order, replication, message split, module homes) live
in `AGENTS.md`. Not checked by CI: run `cargo fmt` and `clippy` before
committing (§7).

## 1. Comments — short, only when not obvious

- `///` on `pub` items only if name + signature don't explain intent.
- `//!` on files only if purpose isn't obvious from filename.
- Prefer rationale ("why") over restatement ("what").

```rust
// Good: explains why, not what.
/// Rider starts outside rubber range.
pub const SPAWN_CLEARANCE: f32 = 80.0;

// Bad: restates the code.
/// The spawn clearance constant set to 80.
```

## 2. Types — newtypes for domain primitives

Keep single-field wrappers even when they look trivial. They prevent
mixing `f32` domains at compile time and allow per-type `Default`,
methods, and Bevy `replicate()`.

```rust
pub struct Rubber(pub f32);
pub struct SpeedMult(pub f32);
pub struct Position(pub Vec2);

impl Default for Rubber {
    fn default() -> Self {
        Self(BASE_RUBBER)
    }
}
```

- Access via `.0` is expected. Add methods (`SpeedMult::base()`) instead
  of free helpers when logic belongs to the type. Document when `Default`
  != zero (e.g. `Rubber::default() = BASE_RUBBER`).

## 3. Visibility — private by default

- `shared/` is a flat API crate — root re-exports in `lib.rs` are intended.
- `server`/`client` internals stay private, escalate to `pub(crate)` then
  `pub` only on need.

## 4. Functions — keep transactions whole, extract pure helpers

- Keep transactional loops whole (build immutable snapshot → mutate).
  Don't split the loop just to hit a line count.
- Extract *pure* helpers instead: `ray_hit`, `nearest`, `spawn_is_safe`.
- Review trigger (not a hard cap): >~150 lines OR a second mutable query.
- Pure helpers stay tiny (<20 lines): `rotate_left`, `arena_walls`,
  `spawn_from_seed`.

## 5. Errors — Option, enum, or validation Result

- Absence (one way to fail) → `Option<T>`.
- Domain branch (many reasons, incl. wire outcomes) → typed enum.
  Label wire enums as wire (e.g. `RespawnOutcome` rides in `RespawnReply`).

```rust
// One failure mode:
pub fn find_safe_spawn(...) -> Option<SafeSpawn>

// Many failure modes (wire type):
pub enum RespawnOutcome { Accepted, NotEligible, NoSafeSpace }
```

- UI/validation → `Result<T, &'static str>` allowed.
- System code: early-return (`let Ok(x) = ... else { return }`) over
  `unwrap`. `unwrap`/`expect` in tests only.
- Reserve `Result<T, E>` + `thiserror` for real IO boundaries.

## 6. Constants — no magic numbers

- Named consts over magic numbers. Shared sim coefficients live in
  `constants.rs` or as module-local `const` next to sole use; UI/client-only
  and channel timeouts stay local to their module.
- Call sites never inline gameplay numbers.

```rust
// Good:
if front < SLOW_DOWN_DISTANCE { ... }

// Bad:
if front < 12.0 { ... }
```

- `const` = compile-time (dev edits code). `Resource`/`Component` = runtime
  (varies per-room without recompile, e.g. `ArenaSize`). Promote to
  `Resource` only if it must vary at runtime.

## 7. Formatting — always enforced

- Must pass: `cargo fmt -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`.
- `#[allow(clippy::type_complexity)]` on Bevy system signatures is accepted.

## 8. Perf hints — `#[inline]` only on cross-crate tiny fns

`#[inline]` is a hint, not a guarantee — no behavior change. Keep it only
on cross-crate 1–5 line helpers (`rotate_left`, `rotate_right`). Never add
for perf without a profile — per-tick allocations dominate call overhead.

```rust
#[inline]
pub fn rotate_left(v: Vec2) -> Vec2 {
    Vec2::new(-v.y, v.x)
}
```

## 9. Logging — ratified

- `warn!` = denied/degraded, `info!` = lifecycle (join/leave/spawn accepted),
  `debug!` = stale/ignorable per-tick noise.
- Never `println!`/`eprintln!` in `crates/` — use `tracing` only. A binary
  installs its log subscriber before reading config, or early warnings are
  dropped silently.
