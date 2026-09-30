//! One persistent screen-space prompt, independent of rider/camera lifetimes.

use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use lightyear::prelude::{Client, Connected, ControlledBy, Predicted};
use shared::protocol::RespawnOutcome;
use shared::{IsAlive, Player, PlayerInput};

use crate::input::RespawnUi;

#[derive(Component)]
pub struct DeathOverlay;

pub fn setup_death_overlay(mut commands: Commands) {
    commands.spawn((
        DeathOverlay,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(28.0),
            ..Default::default()
        },
        TextColor(Color::srgb(1.0, 0.25, 0.25)),
        BackgroundColor(Color::srgba(0.03, 0.03, 0.03, 0.9)),
        Node {
            position_type: PositionType::Absolute,
            top: px(24.0),
            left: px(24.0),
            padding: UiRect::all(px(16.0)),
            display: Display::None,
            ..Default::default()
        },
    ));
}

fn prompt(alive: bool, respawn: &RespawnUi) -> &'static str {
    if alive {
        ""
    } else if respawn.pending_generation.is_some() {
        "YOU DIED\nRespawn requested — waiting for server..."
    } else {
        match respawn.outcome {
            Some(RespawnOutcome::NoSafeSpace) => {
                "YOU DIED\nNo safe spawn space. Press Space / Enter to retry."
            }
            Some(RespawnOutcome::NotEligible) => {
                "YOU DIED\nServer has not confirmed eligibility. Press Space / Enter to retry."
            }
            _ => "YOU DIED\nPress Space / Enter to respawn",
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn update_death_overlay(
    clients: Query<Entity, (With<Client>, With<Connected>)>,
    players: Query<
        (&IsAlive, Option<&ControlledBy>),
        (
            With<Player>,
            With<Predicted>,
            With<InputMarker<PlayerInput>>,
        ),
    >,
    respawn: Res<RespawnUi>,
    mut overlay: Query<(&mut Text, &mut Node), With<DeathOverlay>>,
) {
    let local = clients.iter().next().and_then(|client| {
        players
            .iter()
            .find(|(_, controlled)| controlled.is_none_or(|c| c.owner == client))
    });
    let text = local.map_or("", |(alive, _)| prompt(alive.0, &respawn));
    for (mut label, mut node) in &mut overlay {
        if label.0 != text {
            label.0 = text.to_owned();
        }
        node.display = if text.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_tracks_dead_pending_rejected_and_alive_states() {
        let mut ui = RespawnUi::default();
        assert!(prompt(false, &ui).contains("Space / Enter"));
        ui.pending_generation = Some(1);
        assert!(prompt(false, &ui).contains("waiting for server"));
        assert_eq!(prompt(true, &ui), "");
        ui.pending_generation = None;
        ui.outcome = Some(RespawnOutcome::NoSafeSpace);
        assert!(prompt(false, &ui).contains("No safe spawn"));
        ui.outcome = Some(RespawnOutcome::NotEligible);
        assert!(prompt(false, &ui).contains("eligibility"));
    }
}
