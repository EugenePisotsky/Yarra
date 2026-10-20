//! Characters' feet pressing soft ground (`terrain_render::GroundPresses`). Each model names its
//! foot joints and sole in the character catalog; every frame each foot lists the sole under it,
//! each end pressed while it touches the ground, so a planted foot leaves its whole print, a
//! rolling one its ball and a lifted one nothing.
use crate::{
    WorldCatalog, WorldOrigin, WorldViewpoint, actor::CharacterMotion,
    character_catalog::FeetDefinition,
};

use bevy::{math::DVec2, prelude::*, transform::TransformSystems};
use std::collections::HashSet;
use terrain_render::{GroundPress, GroundPressSystems, GroundPresses};

/// How far a foot standing on soft ground sinks into it, metres. Characters walk on the surface
/// they are grounded on; their prints go this far below the soles.
const PRESS_DEPTH: f32 = 0.07;

/// How high over the ground a sole still touches it fully, and where it no longer does, metres.
/// A foot comes down and leaves at a shallow angle: pressing whenever it was within the print's
/// depth of the ground dragged each print out to twice the sole's length.
const CONTACT_METRES: [f32; 2] = [0.015, 0.035];

/// A character's feet: its catalog sole and each foot's ankle and toe joints, once its scene has
/// spawned them.
#[derive(Component, Clone, Debug)]
pub(crate) struct CharacterFeet {
    sole: FeetDefinition,
    joints: Vec<[Option<Entity>; 2]>,
}

impl CharacterFeet {
    pub(crate) fn new(sole: FeetDefinition) -> Self {
        let joints = vec![[None; 2]; sole.joints.len()];
        Self { sole, joints }
    }

    fn attach(&mut self, name: &str, joint: Entity) {
        for (names, joints) in self.sole.joints.iter().zip(&mut self.joints) {
            if names.0 == name {
                joints[0] = Some(joint);
            }

            if names.1 == name {
                joints[1] = Some(joint);
            }
        }
    }

    /// The press of each foot whose joints are known, for a character grounded at `ground`.
    fn presses(&self, ground: f32, joints: &Query<&GlobalTransform>) -> Vec<(usize, Press)> {
        self.joints
            .iter()
            .enumerate()
            .filter_map(|(foot, pair)| {
                let [ankle, toe] = pair.map(|j| j.and_then(|e| joints.get(e).ok()));
                Some((
                    foot,
                    self.press(ground, ankle?.translation(), toe?.translation()),
                ))
            })
            .collect()
    }

    fn press(&self, ground: f32, ankle: Vec3, toe: Vec3) -> Press {
        let s = &self.sole;
        let forward = (toe.xz() - ankle.xz()).normalize_or(Vec2::Y);
        let depth = |joint: Vec3, height: f32| {
            let above = joint.y - height - ground;
            PRESS_DEPTH * (1.0 - smoothstep(CONTACT_METRES[0], CONTACT_METRES[1], above))
        };
        Press {
            heel: ankle.xz() - forward * (s.heel_behind_ankle_m - s.heel_radius_m),
            ball: toe.xz() + forward * (s.toe_ahead_m - s.ball_radius_m),
            radii: [s.heel_radius_m, s.ball_radius_m],
            depth: [depth(ankle, s.ankle_height_m), depth(toe, s.toe_height_m)],
        }
    }
}

/// One foot's sole on the ground, in render space.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    heel: Vec2,
    ball: Vec2,
    radii: [f32; 2],
    depth: [f32; 2],
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            find_foot_joints,
            list_ground_presses
                .in_set(GroundPressSystems)
                .after(TransformSystems::Propagate)
                .run_if(resource_exists::<GroundPresses>),
        )
            .chain(),
    );
}

/// Joints spawned by a character's scene take their places among its feet.
fn find_foot_joints(
    named: Query<(Entity, &Name), Added<Name>>,
    parents: Query<&ChildOf>,
    mut feet: Query<&mut CharacterFeet>,
) {
    if named.is_empty() {
        return;
    }

    let wanted: HashSet<String> = feet
        .iter()
        .flat_map(|f| f.sole.joints.clone())
        .flat_map(|(ankle, toe)| [ankle, toe])
        .collect();

    for (joint, name) in &named {
        if !wanted.contains(name.as_str()) {
            continue;
        }

        let mut at = joint;

        while let Ok(parent) = parents.get(at) {
            at = parent.parent();

            if let Ok(mut character) = feet.get_mut(at) {
                character.attach(name.as_str(), joint);
                break;
            }
        }
    }
}

/// Every character's feet, and the player the prints centre on.
fn list_ground_presses(
    viewpoint: Res<WorldViewpoint>,
    origin: Res<WorldOrigin>,
    catalog: Res<WorldCatalog>,
    characters: Query<(Entity, &GlobalTransform, &CharacterFeet), With<CharacterMotion>>,
    joints: Query<&GlobalTransform>,
    mut presses: ResMut<GroundPresses>,
) {
    let presses = presses.as_mut();
    presses.presses.clear();
    presses.space = origin.space();
    let Some(cell_size) = origin
        .space()
        .and_then(|space| catalog.world_space(space))
        .map(|space| space.cell_size)
    else {
        presses.focus = None;
        return;
    };
    presses.origin = DVec2::from_array(origin.cell().origin(cell_size));
    presses.focus = viewpoint
        .position()
        .filter(|p| Some(p.space) == origin.space())
        .map(|p| Vec3::from_array(p.relative_to(origin.cell(), cell_size)));

    for (character, transform, feet) in &characters {
        let ground = transform.translation().y;

        for (foot, press) in feet.presses(ground, &joints) {
            presses.presses.push(GroundPress {
                source: character.to_bits() << 2 | foot as u64,
                heel: press.heel,
                ball: press.ball,
                radii: press.radii,
                depth: press.depth,
            });
        }
    }
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feet() -> CharacterFeet {
        CharacterFeet::new(FeetDefinition {
            joints: vec![("foot.l".into(), "toes_01.l".into())],
            ankle_height_m: 0.068,
            toe_height_m: 0.012,
            heel_behind_ankle_m: 0.065,
            toe_ahead_m: 0.06,
            heel_radius_m: 0.04,
            ball_radius_m: 0.045,
        })
    }

    #[test]
    fn a_flat_foot_presses_its_whole_sole_to_the_press_depth() {
        let press = feet().press(
            10.0,
            Vec3::new(0.0, 10.068, 0.0),
            Vec3::new(0.0, 10.012, 0.13),
        );
        assert!(press.depth.iter().all(|d| (d - PRESS_DEPTH).abs() < 1e-5));

        // The sole reaches from 6.5 cm behind the ankle to 6 cm past the toe joint.
        assert!((press.heel.y - press.radii[0] - -0.065).abs() < 1e-5);
        assert!((press.ball.y + press.radii[1] - 0.19).abs() < 1e-5);
        assert_eq!(press.heel.x, 0.0);
    }

    #[test]
    fn a_rising_heel_presses_less_and_a_lifted_foot_not_at_all() {
        let f = feet();
        let rolling = f.press(0.0, Vec3::new(0.0, 0.12, 0.0), Vec3::new(0.0, 0.01, 0.12));
        assert!(rolling.depth[0] < 0.02 && (rolling.depth[1] - PRESS_DEPTH).abs() < 1e-5);
        let lifted = f.press(0.0, Vec3::new(0.0, 0.22, 0.0), Vec3::new(0.0, 0.15, 0.12));
        assert!(lifted.depth.iter().all(|&d| d <= 0.0));

        // A sole a few centimetres up on its way down presses nothing yet.
        let landing = f.press(0.0, Vec3::new(0.0, 0.11, 0.0), Vec3::new(0.0, 0.06, 0.12));
        assert!(landing.depth.iter().all(|&d| d <= 0.0));
    }

    #[test]
    fn joints_attach_by_name() {
        let mut f = feet();
        f.attach("toes_01.l", Entity::from_raw_u32(5).unwrap());
        f.attach("hand.l", Entity::from_raw_u32(6).unwrap());
        assert_eq!(
            f.joints,
            vec![[None, Some(Entity::from_raw_u32(5).unwrap())]]
        );
    }
}
