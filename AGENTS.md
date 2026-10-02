# Agent (you)

## Original JavaScript Codebase

The original repo is cloned alongside this Rust rewrite at `..\tron-zero-js`
(`C:\dev\tron-zero-js` when this repository is at `C:\dev\tron-zero-rust`).
Consult it for existing gameplay behavior and porting details; `PLAN.md` is
the rewrite plan, not a substitute for checking the original implementation.
Keep changes to the Rust repository unless explicitly asked to edit the JS code.

## Base Game Armagetron

The base game is available at https://github.com/ArmagetronAd/armagetronad

## Project Development

Do not build the project yourself. Test your code using : 

- cargo check — fast compilation check without producing binaries (syntax + type checking)
- cargo clippy — linting (catches errors + idiomatic issues)
- cargo fmt — formatting

## Generic Rust Development

Below are generic Rust gotchas that can help you. Don't get too caught up in those.

### Gotchas

- Moves are real. A non-Copy value passed anywhere leaves the old binding dead.
- No &mut coexisting with any other reference to the same data.
- String vs &str. String is owned and heap-allocated; &str is borrowed. You cannot return a &str pointing at something you just created.
- ? propagates errors. The type is Result<T, E>, not exceptions.
- No null, no inheritance. Use Option<T>, enums for sum types, traits for polymorphism, composition over inheritance.
- Shadowing is idiomatic. let x = x.trim(); is normal.
- Iterators are lazy. Nothing runs until they are consumed.
- Drop order. Locals drop in reverse declaration order. Struct fields drop in declaration order, not reverse.

### Performance

- Allocations usually dominate over instruction-level tweaks. Pre-allocate (with_capacity), reuse buffers, and avoid format!() in hot paths.
- Do not benchmark debug builds. --release is a different language.
- Vec beats linked lists. Cache locality wins.

### Maintainability

- Encode invariants in types so illegal states are unrepresentable. Prefer newtypes over raw primitives.
- pub sparingly. The default is private; use pub(crate) for internals.
- #[derive] deliberately. Debug almost always, Clone only when you mean it, Copy only for small plain data.
- Traits, not deep hierarchies. Keep them thin and composable (Read / Write).
- Rc<RefCell<_>> everywhere means the ownership model is wrong.
- Unsafe stays small. Isolate it and write the invariants in // SAFETY: comments.