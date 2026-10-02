//! Turn key bindings (in-memory only, no persistence).
//!
//! Defaults mirror the JS client (`ControlSettings.ts`): left on
//! Q/S/D/ArrowLeft, right on K/L/M/ArrowRight. Validation ports the JS rules:
//! min 1 key per side, max 5 per side, no duplicates within or across sides,
//! Escape never binds (it cancels capture instead).

use bevy::input::keyboard::Key;
use bevy::prelude::*;

/// Maximum bindings per side (ports JS `MAX_KEYS_PER_DIRECTION`).
pub const MAX_KEYS_PER_SIDE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnSide {
    Left,
    Right,
}

/// One turn binding: `code` is the physical match identity, `label` is the
/// layout-aware glyph shown on settings chips. Matching, capture identity,
/// and validation only ever look at `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub code: KeyCode,
    pub label: String,
}

impl Binding {
    pub fn new(code: KeyCode, label: impl Into<String>) -> Self {
        Self {
            code,
            label: label.into(),
        }
    }

    /// QWERTY positional name for defaults and fallback.
    pub fn from_code(code: KeyCode) -> Self {
        let label = physical_label(code);
        Self::new(code, label)
    }

    /// Capture-time constructor: match on `code`, display the logical glyph.
    pub fn from_capture(code: KeyCode, logical: &Key) -> Self {
        let label = label_for(code, logical);
        Self::new(code, label)
    }
}

#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct TurnBindings {
    pub left: Vec<Binding>,
    pub right: Vec<Binding>,
}

impl Default for TurnBindings {
    fn default() -> Self {
        // QWERTY positional names by convention; matching is physical anyway.
        Self {
            left: [
                KeyCode::KeyQ,
                KeyCode::KeyS,
                KeyCode::KeyD,
                KeyCode::ArrowLeft,
            ]
            .into_iter()
            .map(Binding::from_code)
            .collect(),
            right: [
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::ArrowRight,
            ]
            .into_iter()
            .map(Binding::from_code)
            .collect(),
        }
    }
}

impl TurnBindings {
    pub fn keys(&self, side: TurnSide) -> &[Binding] {
        match side {
            TurnSide::Left => &self.left,
            TurnSide::Right => &self.right,
        }
    }

    fn keys_mut(&mut self, side: TurnSide) -> &mut Vec<Binding> {
        match side {
            TurnSide::Left => &mut self.left,
            TurnSide::Right => &mut self.right,
        }
    }

    pub fn is_left(&self, key: KeyCode) -> bool {
        self.left.iter().any(|b| b.code == key)
    }

    pub fn is_right(&self, key: KeyCode) -> bool {
        self.right.iter().any(|b| b.code == key)
    }

    /// Restore the JS defaults and drop any in-progress capture state held elsewhere.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn add_key(
        &mut self,
        side: TurnSide,
        code: KeyCode,
        logical: &Key,
    ) -> Result<(), &'static str> {
        validate_add_key(&self.left, &self.right, side, code)?;
        self.keys_mut(side)
            .push(Binding::from_capture(code, logical));
        Ok(())
    }

    pub fn remove_key(&mut self, side: TurnSide, index: usize) -> Result<(), &'static str> {
        validate_remove_key(self.keys(side), index)?;
        self.keys_mut(side).remove(index);
        Ok(())
    }

    pub fn rebind_key(
        &mut self,
        side: TurnSide,
        index: usize,
        code: KeyCode,
        logical: &Key,
    ) -> Result<(), &'static str> {
        validate_rebind_key(&self.left, &self.right, side, index, code)?;
        self.keys_mut(side)[index] = Binding::from_capture(code, logical);
        Ok(())
    }

    /// Apply a captured key: `index == len` appends a new binding, otherwise it
    /// replaces the binding at `index`. Used by the keyboard system when a
    /// rebind capture completes.
    pub fn apply_capture(
        &mut self,
        side: TurnSide,
        index: usize,
        code: KeyCode,
        logical: &Key,
    ) -> Result<(), &'static str> {
        if index < self.keys(side).len() {
            self.rebind_key(side, index, code, logical)
        } else if index == self.keys(side).len() {
            self.add_key(side, code, logical)
        } else {
            Err("Nothing to rebind")
        }
    }
}

// Pure validation so the rules are unit-testable without Bevy resources.
// Compares physical `code` fields only; labels never affect matching.
pub fn validate_add_key(
    left: &[Binding],
    right: &[Binding],
    side: TurnSide,
    key: KeyCode,
) -> Result<(), &'static str> {
    if key == KeyCode::Escape {
        return Err("Escape cannot be bound");
    }
    let (own, other) = match side {
        TurnSide::Left => (left, right),
        TurnSide::Right => (right, left),
    };
    if own.iter().any(|b| b.code == key) || other.iter().any(|b| b.code == key) {
        return Err("Key is already bound");
    }
    if own.len() >= MAX_KEYS_PER_SIDE {
        return Err("Maximum 5 keys per side");
    }
    Ok(())
}

pub fn validate_rebind_key(
    left: &[Binding],
    right: &[Binding],
    side: TurnSide,
    index: usize,
    key: KeyCode,
) -> Result<(), &'static str> {
    if key == KeyCode::Escape {
        return Err("Escape cannot be bound");
    }
    let (own, other) = match side {
        TurnSide::Left => (left, right),
        TurnSide::Right => (right, left),
    };
    let Some(current) = own.get(index) else {
        return Err("Nothing to rebind");
    };
    if current.code == key {
        return Ok(());
    }
    if own.iter().any(|b| b.code == key) || other.iter().any(|b| b.code == key) {
        return Err("Key is already bound");
    }
    Ok(())
}

pub fn validate_remove_key(keys: &[Binding], index: usize) -> Result<(), &'static str> {
    if keys.len() <= 1 {
        return Err("Each side needs at least one key");
    }
    if index >= keys.len() {
        return Err("Nothing to remove");
    }
    Ok(())
}

/// In-progress rebind capture plus the latest settings error for the UI.
/// `capturing` holds `(side, index)` where `index == len` means "append new".
#[derive(Resource, Default)]
pub struct RebindState {
    pub capturing: Option<(TurnSide, usize)>,
    pub error: Option<String>,
}

// Stored layout-aware label for a bound key (what chips show).
pub fn key_label(binding: &Binding) -> &str {
    &binding.label
}

// QWERTY positional name for a physical code (defaults + fallback).
pub fn physical_label(code: KeyCode) -> String {
    match code {
        KeyCode::ArrowLeft => "←".into(),
        KeyCode::ArrowRight => "→".into(),
        KeyCode::ArrowUp => "↑".into(),
        KeyCode::ArrowDown => "↓".into(),
        KeyCode::Space => "Space".into(),
        KeyCode::Escape => "Esc".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::NumpadEnter => "NumEnter".into(),
        _ => {
            let name = format!("{code:?}");
            name.strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .unwrap_or(&name)
                .to_owned()
        }
    }
}

/// Display label for a capture: the logical glyph when it is a single
/// character (uppercased, so AZERTY physical KeyQ + "a" shows "A"), else the
/// physical name. Special keys keep fixed labels.
pub fn label_for(code: KeyCode, logical: &Key) -> String {
    match code {
        KeyCode::ArrowLeft
        | KeyCode::ArrowRight
        | KeyCode::ArrowUp
        | KeyCode::ArrowDown
        | KeyCode::Space
        | KeyCode::Escape
        | KeyCode::Enter
        | KeyCode::NumpadEnter => return physical_label(code),
        _ => {}
    }
    if let Key::Character(s) = logical
        && s.chars().count() == 1
    {
        return s.to_uppercase();
    }
    physical_label(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(bindings: &[Binding]) -> Vec<KeyCode> {
        bindings.iter().map(|b| b.code).collect()
    }

    #[test]
    fn defaults_match_js_client() {
        let bindings = TurnBindings::default();
        assert_eq!(
            codes(bindings.keys(TurnSide::Left)),
            vec![
                KeyCode::KeyQ,
                KeyCode::KeyS,
                KeyCode::KeyD,
                KeyCode::ArrowLeft
            ]
        );
        assert_eq!(
            codes(bindings.keys(TurnSide::Right)),
            vec![
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::ArrowRight
            ]
        );
        // Defaults show QWERTY positional names.
        assert_eq!(
            bindings
                .keys(TurnSide::Left)
                .iter()
                .map(key_label)
                .collect::<Vec<_>>(),
            vec!["Q", "S", "D", "←"]
        );
        assert_eq!(
            bindings
                .keys(TurnSide::Right)
                .iter()
                .map(key_label)
                .collect::<Vec<_>>(),
            vec!["K", "L", "M", "→"]
        );
    }

    fn unidentified() -> Key {
        Key::Unidentified(bevy::input::keyboard::NativeKey::Unidentified)
    }

    #[test]
    fn duplicates_within_and_across_sides_rejected() {
        let mut bindings = TurnBindings::default();
        assert!(
            bindings
                .add_key(TurnSide::Left, KeyCode::KeyQ, &unidentified())
                .is_err()
        );
        assert!(
            bindings
                .add_key(TurnSide::Left, KeyCode::KeyK, &unidentified())
                .is_err()
        );
        assert!(
            bindings
                .rebind_key(TurnSide::Right, 0, KeyCode::KeyS, &unidentified())
                .is_err()
        );
        // Rebinding to the same key already in the slot is a no-op success.
        assert!(
            bindings
                .rebind_key(TurnSide::Left, 0, KeyCode::KeyQ, &unidentified())
                .is_ok()
        );
    }

    #[test]
    fn duplicates_are_code_based_labels_ignored() {
        let mut bindings = TurnBindings::default();
        // Physical KeyQ already bound as "Q"; capturing it as AZERTY "A" is
        // still the same physical key, so it stays rejected.
        assert!(
            bindings
                .add_key(TurnSide::Right, KeyCode::KeyQ, &Key::Character("a".into()))
                .is_err()
        );
        assert!(
            bindings
                .rebind_key(
                    TurnSide::Right,
                    0,
                    KeyCode::KeyS,
                    &Key::Character("s".into())
                )
                .is_err()
        );
    }

    #[test]
    fn escape_never_binds() {
        let mut bindings = TurnBindings::default();
        assert!(
            bindings
                .add_key(TurnSide::Left, KeyCode::Escape, &unidentified())
                .is_err()
        );
        assert!(
            bindings
                .rebind_key(TurnSide::Right, 0, KeyCode::Escape, &unidentified())
                .is_err()
        );
        assert!(
            bindings
                .apply_capture(TurnSide::Left, 0, KeyCode::Escape, &unidentified())
                .is_err()
        );
    }

    #[test]
    fn min_one_and_max_five_per_side() {
        let mut bindings = TurnBindings::default();
        // Fill left to the cap: Q S D Left + one more.
        assert!(
            bindings
                .add_key(TurnSide::Left, KeyCode::KeyT, &unidentified())
                .is_ok()
        );
        assert_eq!(
            bindings.add_key(TurnSide::Left, KeyCode::KeyG, &unidentified()),
            Err("Maximum 5 keys per side")
        );
        // Drain left down to one key; removal below 1 is blocked.
        for _ in 0..4 {
            assert!(bindings.remove_key(TurnSide::Left, 0).is_ok());
        }
        assert_eq!(bindings.left.len(), 1);
        assert_eq!(
            bindings.remove_key(TurnSide::Left, 0),
            Err("Each side needs at least one key")
        );
    }

    #[test]
    fn capture_appends_at_len_and_rebinds_below_len() {
        let mut bindings = TurnBindings::default();
        let len = bindings.left.len();
        assert!(
            bindings
                .apply_capture(TurnSide::Left, len, KeyCode::KeyT, &unidentified())
                .is_ok()
        );
        assert_eq!(bindings.left[len].code, KeyCode::KeyT);
        assert!(
            bindings
                .apply_capture(TurnSide::Left, 0, KeyCode::KeyG, &unidentified())
                .is_ok()
        );
        assert_eq!(bindings.left[0].code, KeyCode::KeyG);
        assert!(
            bindings
                .apply_capture(TurnSide::Left, len + 5, KeyCode::KeyH, &unidentified())
                .is_err()
        );
    }

    #[test]
    fn capture_stores_layout_glyph_but_matches_physical_code() {
        let mut bindings = TurnBindings::default();
        // AZERTY: physical KeyQ produces logical "a".
        bindings
            .apply_capture(
                TurnSide::Left,
                0,
                KeyCode::KeyQ,
                &Key::Character("a".into()),
            )
            .unwrap();
        assert_eq!(bindings.left[0].code, KeyCode::KeyQ);
        assert_eq!(key_label(&bindings.left[0]), "A");
        assert!(bindings.is_left(KeyCode::KeyQ));
    }

    #[test]
    fn label_for_uses_glyph_with_physical_fallback() {
        // Layout glyph, lower- and upper-case both normalize.
        assert_eq!(label_for(KeyCode::KeyQ, &Key::Character("a".into())), "A");
        assert_eq!(label_for(KeyCode::KeyQ, &Key::Character("Q".into())), "Q");
        // Multi-char logicals fall back to the physical name.
        assert_eq!(
            label_for(KeyCode::KeyQ, &Key::Character("Enter".into())),
            "Q"
        );
        // Non-character keys fall back to the physical name.
        assert_eq!(label_for(KeyCode::KeyQ, &unidentified()), "Q");
        assert_eq!(label_for(KeyCode::KeyQ, &Key::Dead(None)), "Q");
        // Special keys keep fixed labels regardless of logical.
        assert_eq!(
            label_for(KeyCode::ArrowLeft, &Key::Character("a".into())),
            "←"
        );
        assert_eq!(
            label_for(KeyCode::ArrowRight, &Key::Character("m".into())),
            "→"
        );
        assert_eq!(
            label_for(KeyCode::Space, &Key::Character(" ".into())),
            "Space"
        );
    }

    #[test]
    fn reset_restores_defaults() {
        let mut bindings = TurnBindings::default();
        bindings.remove_key(TurnSide::Left, 0).unwrap();
        bindings.reset();
        assert_eq!(bindings, TurnBindings::default());
    }
}
