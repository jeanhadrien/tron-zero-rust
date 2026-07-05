# Agent (you)

## Project Development

Do not build the project yourself.

- cargo check — fast compilation check without producing binaries (syntax + type checking)
- cargo clippy — linting (catches errors + idiomatic issues)
- cargo fmt — formatting

## Docs 

Use docs/index.md for library doc URLs instead of reading source directly

## Generic Rust Development

Below are generic Rust gotchas that can help you. Don't get too caught up in those.

### Gotchas (vs other languages)

- **Moves are real** — non-`Copy` value passed anywhere = old binding dead.
- **No `&mut` coexisting** with any other ref.
- **`String` vs `&str`** — `String` is owned/heap, `&str` is borrowed. Can't return `&str` to something you just created.
- **`?` for error propagation**, `Result<T, E>` not exceptions. Idiomatic = per-module error enums.
- **No null, no inheritance** — `Option<T>`, enums for sum types, traits for polymorphism, composition over inheritance.
- **Shadowing is idiomatic** — `let x = x.trim();` is normal.
- **Iterators are lazy** — nothing happens until consumed.
- **Drop order** — fields dropped in declaration order, not reverse.
- **Avoid `.unwrap()`** in production — use `?`, `.ok_or(...)`, or match.

### Performance

- **Allocations matter most** — pre-allocate (`with_capacity`), reuse buffers, avoid `format!()` in hot paths.
- **Generics > `dyn Trait`** for hot paths (monomorphization = static dispatch, no vtable).
- **Don't benchmark in debug** — `--release` is a different language.
- **HashMap** defaults to SipHash. Swap to `FxHashMap`/`ahash` for small keys.
- **Vec beats linked lists** — cache locality always wins.
- **Unsafe is OK in hot paths** (bounds-check elimination, slice splitting) — profile first.
- **`#[inline]`/`#[inline(always)]`** for tiny functions; `-C lto=fat` for cross-crate inlining.
- **SIMD isn't automatic** — structure loops for auto-vectorization, or reach for `std::simd` (nightly).
- **`tokio::spawn` for I/O, `rayon` for CPU** — don't block in async.

### Maintainability

- **Encode invariants in types** — illegal states unrepresentable. Newtypes over raw primitives.
- **`pub` sparingly** — default private, `pub(crate)` for internals.
- **`#[derive]` deliberately** — `Debug` always, `Clone` only when you mean it, `Copy` only for small POD.
- **One concept per module**, re-export via `pub use` for flat public API.
- **`thiserror` for libraries, `anyhow` for applications** — per-module error enums.
- **Traits over deep inheritance** — thin, composable traits (`Read`/`Write` pattern).
- **If `Rc<RefCell<>>` everywhere** — ownership model is screaming. Rethink.
- **`lib.rs` as table of contents** — architecture readable from `mod` + `pub use` alone.
- **Unsafe blocks** — isolated, small, invariants in `// SAFETY:` comments.
