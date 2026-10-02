//! Turn key bindings (in-memory only, no persistence).
//!
//! Defaults mirror the JS client (`ControlSettings.ts`): left on
//! Q/S/D/ArrowLeft, right on K/L/M/ArrowRight. Validation ports the JS rules:
//! min 1 key per side, max 5 per side, no duplicates within or across sides,
//! Escape never binds (it cancels capture instead).

use bevy::prelude::*;

/// Maximum bindings per side (ports JS `MAX_KEYS_PER_DIRECTION`).
pub const MAX_KEYS_PER_SIDE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnSide {
    Left,
    Right,
}

#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct TurnBindings {
    pub left: Vec<KeyCode>,
    pub right: Vec<KeyCode>,
}

impl Default for TurnBindings {
    fn default() -> Self {
        Self {
            left: vec![
                KeyCode::KeyQ,
                KeyCode::KeyS,
                KeyCode::KeyD,
                KeyCode::ArrowLeft,
            ],
            right: vec![
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::ArrowRight,
            ],
        }
    }
}

impl TurnBindings {
    pub fn keys(&self, side: TurnSide) -> &[KeyCode] {
        match side {
            TurnSide::Left => &self.left,
            TurnSide::Right => &self.right,
        }
    }

    fn keys_mut(&mut self, side: TurnSide) -> &mut Vec<KeyCode> {
        match side {
            TurnSide::Left => &mut self.left,
            TurnSide::Right => &mut self.right,
        }
    }

    pub fn is_left(&self, key: KeyCode) -> bool {
        self.left.contains(&key)
    }

    pub fn is_right(&self, key: KeyCode) -> bool {
        self.right.contains(&key)
    }

    /// Restore the JS defaults and drop any in-progress capture state held elsewhere.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn add_key(&mut self, side: TurnSide, key: KeyCode) -> Result<(), &'static str> {
        validate_add_key(&self.left, &self.right, side, key)?;
        self.keys_mut(side).push(key);
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
        key: KeyCode,
    ) -> Result<(), &'static str> {
        validate_rebind_key(&self.left, &self.right, side, index, key)?;
        self.keys_mut(side)[index] = key;
        Ok(())
    }

    /// Apply a captured key: `index == len` appends a new binding, otherwise it
    /// replaces the binding at `index`. Used by the keyboard system when a
    /// rebind capture completes.
    pub fn apply_capture(
        &mut self,
        side: TurnSide,
        index: usize,
        key: KeyCode,
    ) -> Result<(), &'static str> {
        if index < self.keys(side).len() {
            self.rebind_key(side, index, key)
        } else if index == self.keys(side).len() {
            self.add_key(side, key)
        } else {
            Err("Nothing to rebind")
        }
    }
}

// Pure validation so the rules are unit-testable without Bevy resources.
pub fn validate_add_key(
    left: &[KeyCode],
    right: &[KeyCode],
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
    if own.contains(&key) || other.contains(&key) {
        return Err("Key is already bound");
    }
    if own.len() >= MAX_KEYS_PER_SIDE {
        return Err("Maximum 5 keys per side");
    }
    Ok(())
}

pub fn validate_rebind_key(
    left: &[KeyCode],
    right: &[KeyCode],
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
    if *current == key {
        return Ok(());
    }
    if own.contains(&key) || other.contains(&key) {
        return Err("Key is already bound");
    }
    Ok(())
}

pub fn validate_remove_key(keys: &[KeyCode], index: usize) -> Result<(), &'static str> {
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

// Short human-readable label for a bound key.
pub fn key_label(key: KeyCode) -> String {
    match key {
        KeyCode::ArrowLeft => "←".into(),
        KeyCode::ArrowRight => "→".into(),
        KeyCode::ArrowUp => "↑".into(),
        KeyCode::ArrowDown => "↓".into(),
        KeyCode::Space => "Space".into(),
        KeyCode::Escape => "Esc".into(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::NumpadEnter => "NumEnter".into(),
        _ => {
            let name = format!("{key:?}");
            name.strip_prefix("Key")
                .or_else(|| name.strip_prefix("Digit"))
                .unwrap_or(&name)
                .to_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_js_client() {
        let bindings = TurnBindings::default();
        assert_eq!(
            bindings.left,
            vec![
                KeyCode::KeyQ,
                KeyCode::KeyS,
                KeyCode::KeyD,
                KeyCode::ArrowLeft
            ]
        );
        assert_eq!(
            bindings.right,
            vec![
                KeyCode::KeyK,
                KeyCode::KeyL,
                KeyCode::KeyM,
                KeyCode::ArrowRight
            ]
        );
    }

    #[test]
    fn duplicates_within_and_across_sides_rejected() {
        let mut bindings = TurnBindings::default();
        assert!(bindings.add_key(TurnSide::Left, KeyCode::KeyQ).is_err());
        assert!(bindings.add_key(TurnSide::Left, KeyCode::KeyK).is_err());
        assert!(bindings.rebind_key(TurnSide::Right, 0, KeyCode::KeyS).is_err());
        // Rebinding to the same key already in the slot is a no-op success.
        assert!(bindings.rebind_key(TurnSide::Left, 0, KeyCode::KeyQ).is_ok());
    }

    #[test]
    fn escape_never_binds() {
        let mut bindings = TurnBindings::default();
        assert!(bindings.add_key(TurnSide::Left, KeyCode::Escape).is_err());
        assert!(
            bindings
                .rebind_key(TurnSide::Right, 0, KeyCode::Escape)
                .is_err()
        );
        assert!(
            bindings
                .apply_capture(TurnSide::Left, 0, KeyCode::Escape)
                .is_err()
        );
    }

    #[test]
    fn min_one_and_max_five_per_side() {
        let mut bindings = TurnBindings::default();
        // Fill left to the cap: Q S D Left + one more.
        assert!(bindings.add_key(TurnSide::Left, KeyCode::KeyT).is_ok());
        assert_eq!(
            bindings.add_key(TurnSide::Left, KeyCode::KeyG),
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
        assert!(bindings.apply_capture(TurnSide::Left, len, KeyCode::KeyT).is_ok());
        assert_eq!(bindings.left[len], KeyCode::KeyT);
        assert!(bindings.apply_capture(TurnSide::Left, 0, KeyCode::KeyG).is_ok());
        assert_eq!(bindings.left[0], KeyCode::KeyG);
        assert!(bindings.apply_capture(TurnSide::Left, len + 5, KeyCode::KeyH).is_err());
    }

    #[test]
    fn reset_restores_defaults() {
        let mut bindings = TurnBindings::default();
        bindings.remove_key(TurnSide::Left, 0).unwrap();
        bindings.reset();
        assert_eq!(bindings, TurnBindings::default());
    }
}
