//! Ordered, rollbackable trail geometry on the player itself.

use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Oldest end, turn vertices, then the moving head. The last pair is active.
/// Keeping geometry in one predicted component makes turns, trimming and death
/// atomic under replication/rollback, without predicted child entity churn.
#[derive(Component, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Trail(pub Vec<Vec2>);

impl Trail {
    pub fn new(position: Vec2) -> Self {
        Self(vec![position, position])
    }

    pub fn turn(&mut self, position: Vec2) {
        if self.0.len() >= 2 && self.0[self.0.len() - 2] != position {
            self.0.push(position);
        }
    }

    pub fn length(&self) -> f32 {
        self.0.windows(2).map(|p| p[0].distance(p[1])).sum()
    }

    pub fn advance(&mut self, head: Vec2, maximum: f32) {
        if let Some(last) = self.0.last_mut() {
            *last = head;
        } else {
            *self = Self::new(head);
        }
        let mut excess = (self.length() - maximum).max(0.0);
        let mut consumed = 0;
        while consumed + 1 < self.0.len() && excess > 0.0 {
            let start = self.0[consumed];
            let end = self.0[consumed + 1];
            let length = start.distance(end);
            if excess >= length && consumed + 2 < self.0.len() {
                excess -= length;
                consumed += 1;
            } else {
                if length > 0.0 {
                    self.0[consumed] = start.lerp(end, (excess / length).min(1.0));
                }
                break;
            }
        }
        if consumed > 0 {
            self.0.drain(..consumed);
        }
    }
}
