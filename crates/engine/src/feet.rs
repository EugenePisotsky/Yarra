//! Characters' feet pushing the soft layer down (`terrain_render::GroundPresses`). Each model
//! names its foot joints and sole in the character catalog; every frame each foot lists its sole
//! and how high over the firm ground its heel and ball are. Characters stand on the firm ground,
//! so a planted foot pushes the snow to it and one swinging low through deep snow ploughs it. The
//! legs plough a body-wide channel through deep snow besides, its floor over the feet lanes: a
//! circle at the character, which the prints sweep along its way.
use crate::{
    WorldCatalog, WorldOrigin, WorldViewpoint, actor::CharacterMotion,
    character_catalog::FeetDefinition,
};

use bevy::{math::DVec2, prelude::*, transform::TransformSystems};
use std::collections::HashSet;
use terrain_render::{GroundPress, GroundPressSystems, GroundPresses};

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
        let floor = |joint: Vec3, height: f32| (joint.y - height - ground).max(0.0);
        Press {
            heel: ankle.xz() - forward * (s.heel_behind_ankle_m - s.heel_radius_m),
            ball: toe.xz() + forward * (s.toe_ahead_m - s.ball_radius_m),
            radii: [s.heel_radius_m, s.ball_radius_m],
            floor: [floor(ankle, s.ankle_height_m), floor(toe, s.toe_height_m)],
            arch: SOLE_ARCH,
        }
    }

    /// The legs' channel for a character standing at render xz `at`.
    fn wade(&self, at: Vec2) -> Press {
        Press {
            heel: at,
            ball: at,
            radii: [self.sole.wade_radius_m; 2],
            floor: [self.sole.wade_floor_m; 2],
            arch: 0.0,
        }
    }
}

/// A sole narrows by this share of its width at the arch.
const SOLE_ARCH: f32 = 0.22;

/// A character's presses are told apart by part: its feet (four at most, by the catalog), then
/// the legs' channel.
const WADE: u64 = 4;

/// One press on the ground, in render space (`terrain_render::GroundPress`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    heel: Vec2,
    ball: Vec2,
    radii: [f32; 2],
    floor: [f32; 2],
    arch: f32,
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
        let at = transform.translation();
        let source = |part: u64| character.to_bits() << 3 | part;
        let wade = (WADE, feet.wade(at.xz()));
        let feet = feet.presses(at.y, &joints).into_iter();

        for (part, press) in feet.map(|(foot, p)| (foot as u64, p)).chain([wade]) {
            presses.presses.push(GroundPress {
                source: source(part),
                heel: press.heel,
                ball: press.ball,
                radii: press.radii,
                floor: press.floor,
                arch: press.arch,
            });
        }
    }
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
            wade_radius_m: 0.26,
            wade_floor_m: 0.12,
        })
    }

    #[test]
    fn a_flat_foot_holds_its_whole_sole_on_the_ground() {
        let press = feet().press(
            10.0,
            Vec3::new(0.0, 10.068, 0.0),
            Vec3::new(0.0, 10.012, 0.13),
        );
        assert!(press.floor.iter().all(|f| f.abs() < 1e-5));

        // The sole reaches from 6.5 cm behind the ankle to 6 cm past the toe joint.
        assert!((press.heel.y - press.radii[0] - -0.065).abs() < 1e-5);
        assert!((press.ball.y + press.radii[1] - 0.19).abs() < 1e-5);
        assert_eq!(press.heel.x, 0.0);
    }

    #[test]
    fn a_rolling_foot_lifts_its_heel_and_a_swinging_one_rides_over_the_ground() {
        let f = feet();
        let rolling = f.press(0.0, Vec3::new(0.0, 0.12, 0.0), Vec3::new(0.0, 0.01, 0.12));
        assert!((rolling.floor[0] - 0.052).abs() < 1e-5 && rolling.floor[1] == 0.0);
        let swinging = f.press(0.0, Vec3::new(0.0, 0.22, 0.0), Vec3::new(0.0, 0.15, 0.12));
        assert!((swinging.floor[0] - 0.152).abs() < 1e-5);
        assert!((swinging.floor[1] - 0.138).abs() < 1e-5);
    }

    #[test]
    fn the_legs_plough_a_round_channel_its_sweeps_join_without_a_waist() {
        let wade = feet().wade(Vec2::new(3.0, 4.0));
        assert_eq!(wade.heel, wade.ball);
        assert_eq!(wade.radii, [0.26; 2]);
        assert_eq!(wade.floor, [0.12; 2]);
        assert_eq!(wade.arch, 0.0);
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
