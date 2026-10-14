//! Capsules that stand in for a character's body, fitted once per body mesh from its bind pose:
//! each capsule runs between two joints and takes its radius from the vertices those bones move
//! most. Torso sections are wider than deep, so they become two capsules side by side.
use bevy::math::{Affine3A, Vec3A};
use std::collections::HashMap;

/// Collider groups a piece can choose to collide with.
pub(crate) mod group {
    pub const TORSO: u32 = 1;
    pub const ARMS: u32 = 1 << 1;
    pub const LEGS: u32 = 1 << 2;
    pub const HEAD: u32 = 1 << 3;
    pub const BACK_ITEMS: u32 = 1 << 4;
    pub const HIP_ITEMS: u32 = 1 << 5;
    /// Garments worn under others, for the ones over them.
    pub const LAYERS: u32 = 1 << 6;
    /// Back items as the cloak sees them: keeping it on one side of them.
    pub const PRESSERS: u32 = 1 << 7;
}

/// A collider in bind space, carried by one bone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BodyCapsule {
    pub name: String,
    pub bone: String,
    pub a: Vec3A,
    pub b: Vec3A,
    pub radius: f32,
    pub group: u32,
}

#[derive(Clone, Copy)]
enum Point {
    Joint(&'static str),
    /// A joint's bind position moved by a bind-space offset.
    Offset(&'static str, [f32; 3]),
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    Round,
    /// Wider than deep: two capsules side by side.
    Flat,
}

struct Spec {
    name: &'static str,
    bone: &'static str,
    from: Point,
    to: Point,
    members: &'static [&'static str],
    shape: Shape,
    group: u32,
    /// Used when the mesh gives too few vertices to fit.
    fallback: f32,
}

#[allow(clippy::too_many_arguments)] // One row of the table below.
const fn spec(
    name: &'static str,
    bone: &'static str,
    from: Point,
    to: Point,
    members: &'static [&'static str],
    shape: Shape,
    group: u32,
    fallback: f32,
) -> Spec {
    Spec {
        name,
        bone,
        from,
        to,
        members,
        shape,
        group,
        fallback,
    }
}

use Point::{Joint, Offset};
use Shape::{Flat, Round};
use group::{ARMS, HEAD, LEGS, TORSO};

/// The colliders of the `yarra_humanoid_v1` rig (Auto-Rig Pro game export).
const HUMANOID_V1: &[Spec] = &[
    spec(
        "hips",
        "root.x",
        Offset("root.x", [0.0, 0.09, 0.0]),
        Offset("root.x", [0.0, -0.1, 0.0]),
        &["root.x", "spine_01.x", "thigh_twist.l", "thigh_twist.r"],
        Flat,
        TORSO,
        0.12,
    ),
    spec(
        "belly",
        "spine_02.x",
        Joint("spine_02.x"),
        Joint("spine_04.x"),
        &["spine_01.x", "spine_02.x", "spine_03.x"],
        Flat,
        TORSO,
        0.1,
    ),
    spec(
        "chest",
        "spine_04.x",
        Joint("spine_04.x"),
        Offset("spine_05.x", [0.0, -0.02, 0.0]),
        &["spine_04.x", "spine_05.x"],
        Flat,
        TORSO,
        0.11,
    ),
    spec(
        "shoulders",
        "spine_05.x",
        Joint("c_arm_twist_offset.l"),
        Joint("c_arm_twist_offset.r"),
        &[
            "shoulder.l",
            "shoulder.r",
            "spine_05.x",
            "subneck_twist_1.x",
        ],
        Round,
        TORSO,
        0.07,
    ),
    spec(
        "neck",
        "neck.x",
        Joint("subneck_twist_1.x"),
        Joint("head.x"),
        &["subneck_twist_1.x", "neck.x"],
        Round,
        HEAD,
        0.05,
    ),
    spec(
        "head",
        "head.x",
        Offset("head.x", [0.0, 0.03, 0.0]),
        Offset("head.x", [0.0, 0.1, 0.0]),
        &["head.x"],
        Round,
        HEAD,
        0.09,
    ),
    spec(
        "upper_arm.l",
        "arm_stretch.l",
        Joint("c_arm_twist_offset.l"),
        Joint("forearm_stretch.l"),
        &["c_arm_twist_offset.l", "arm_twist_2.l", "arm_stretch.l"],
        Round,
        ARMS,
        0.045,
    ),
    spec(
        "upper_arm.r",
        "arm_stretch.r",
        Joint("c_arm_twist_offset.r"),
        Joint("forearm_stretch.r"),
        &["c_arm_twist_offset.r", "arm_twist_2.r", "arm_stretch.r"],
        Round,
        ARMS,
        0.045,
    ),
    spec(
        "forearm.l",
        "forearm_stretch.l",
        Joint("forearm_stretch.l"),
        Joint("hand.l"),
        &["forearm_stretch.l", "forearm_twist_2.l", "forearm_twist.l"],
        Round,
        ARMS,
        0.035,
    ),
    spec(
        "forearm.r",
        "forearm_stretch.r",
        Joint("forearm_stretch.r"),
        Joint("hand.r"),
        &["forearm_stretch.r", "forearm_twist_2.r", "forearm_twist.r"],
        Round,
        ARMS,
        0.035,
    ),
    spec(
        "hand.l",
        "hand.l",
        Joint("hand.l"),
        Joint("c_middle2.l"),
        &[
            "hand.l",
            "c_index1_base.l",
            "c_middle1_base.l",
            "c_ring1_base.l",
            "c_pinky1_base.l",
        ],
        Round,
        ARMS,
        0.03,
    ),
    spec(
        "hand.r",
        "hand.r",
        Joint("hand.r"),
        Joint("c_middle2.r"),
        &[
            "hand.r",
            "c_index1_base.r",
            "c_middle1_base.r",
            "c_ring1_base.r",
            "c_pinky1_base.r",
        ],
        Round,
        ARMS,
        0.03,
    ),
    spec(
        "thigh.l",
        "thigh_stretch.l",
        Joint("thigh_twist.l"),
        Joint("leg_stretch.l"),
        &["thigh_twist.l", "thigh_twist_2.l", "thigh_stretch.l"],
        Round,
        LEGS,
        0.07,
    ),
    spec(
        "thigh.r",
        "thigh_stretch.r",
        Joint("thigh_twist.r"),
        Joint("leg_stretch.r"),
        &["thigh_twist.r", "thigh_twist_2.r", "thigh_stretch.r"],
        Round,
        LEGS,
        0.07,
    ),
    spec(
        "calf.l",
        "leg_stretch.l",
        Joint("leg_stretch.l"),
        Joint("foot.l"),
        &["leg_stretch.l", "leg_twist_2.l", "leg_twist.l"],
        Round,
        LEGS,
        0.05,
    ),
    spec(
        "calf.r",
        "leg_stretch.r",
        Joint("leg_stretch.r"),
        Joint("foot.r"),
        &["leg_stretch.r", "leg_twist_2.r", "leg_twist.r"],
        Round,
        LEGS,
        0.05,
    ),
    spec(
        "foot.l",
        "foot.l",
        Joint("foot.l"),
        Joint("toes_01.l"),
        &["foot.l", "toes_01.l"],
        Round,
        LEGS,
        0.04,
    ),
    spec(
        "foot.r",
        "foot.r",
        Joint("foot.r"),
        Joint("toes_01.r"),
        &["foot.r", "toes_01.r"],
        Round,
        LEGS,
        0.04,
    ),
];

/// A skinned body in bind pose: vertex positions with their strongest joint, and each joint's
/// name and bind pose.
pub(crate) struct BindBody<'a> {
    pub positions: &'a [[f32; 3]],
    /// Index into `joints` of the joint that moves each vertex most.
    pub dominant: &'a [u16],
    pub joints: &'a [(String, Affine3A)],
}

/// Fits the body's colliders. Specs whose joints the rig lacks are left out.
pub(crate) fn fit(body: &BindBody) -> Vec<BodyCapsule> {
    let by_name: HashMap<&str, usize> = body
        .joints
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.as_str(), i))
        .collect();
    let position = |point: Point| -> Option<Vec3A> {
        let (name, offset) = match point {
            Joint(name) => (name, [0.0; 3]),
            Offset(name, offset) => (name, offset),
        };
        let &i = by_name.get(name)?;
        Some(body.joints[i].1.translation + Vec3A::from_array(offset))
    };
    let mut capsules = Vec::new();
    for spec in HUMANOID_V1 {
        let (Some(a), Some(b), true) = (
            position(spec.from),
            position(spec.to),
            by_name.contains_key(spec.bone),
        ) else {
            continue;
        };
        let members: Vec<usize> = spec
            .members
            .iter()
            .filter_map(|m| by_name.get(m).copied())
            .collect();
        let samples = section_samples(body, &members, a, b);
        let push = |name: String, a: Vec3A, b: Vec3A, radius: f32| BodyCapsule {
            name,
            bone: spec.bone.to_owned(),
            a,
            b,
            radius,
            group: spec.group,
        };
        if samples.len() < 12 {
            capsules.push(push(spec.name.into(), a, b, spec.fallback));
            continue;
        }
        match spec.shape {
            Round => {
                let mut radial: Vec<f32> = samples.iter().map(|s| s.radial).collect();
                let radius = percentile(&mut radial, 0.8);
                capsules.push(push(spec.name.into(), a, b, radius));
            }
            Flat => {
                let mut depth: Vec<f32> = samples.iter().map(|s| s.depth).collect();
                let low = percentile(&mut depth, 0.08);
                let high = percentile(&mut depth, 0.92);
                let mut width: Vec<f32> = samples.iter().map(|s| s.lateral.abs()).collect();
                let half_width = percentile(&mut width, 0.9);
                let radius = (high - low) * 0.5;
                let frame = section_frame(a, b);
                let centre = frame.depth * ((high + low) * 0.5);
                let offset = (half_width - radius).max(0.0);
                if offset < 0.01 {
                    capsules.push(push(spec.name.into(), a + centre, b + centre, radius));
                } else {
                    for (side, sign) in [("l", 1.0), ("r", -1.0)] {
                        let shift = centre + frame.lateral * (offset * sign);
                        capsules.push(push(
                            format!("{}.{side}", spec.name),
                            a + shift,
                            b + shift,
                            radius,
                        ));
                    }
                }
            }
        }
    }
    capsules
}

struct SectionFrame {
    lateral: Vec3A,
    depth: Vec3A,
}

/// Across the body (+X, the character's left) and front to back (+Z, forward), square to the
/// section's axis.
fn section_frame(a: Vec3A, b: Vec3A) -> SectionFrame {
    let axis = (b - a).normalize_or(Vec3A::Y);
    let lateral = (Vec3A::X - axis * axis.x).normalize_or(axis.any_orthonormal_vector());
    let depth = lateral.cross(axis);
    let depth = if depth.z < 0.0 { -depth } else { depth };
    SectionFrame { lateral, depth }
}

struct Sample {
    radial: f32,
    lateral: f32,
    depth: f32,
}

/// Vertices the members move most, lying along the section's middle.
fn section_samples(body: &BindBody, members: &[usize], a: Vec3A, b: Vec3A) -> Vec<Sample> {
    let axis = b - a;
    let length2 = axis.length_squared().max(f32::EPSILON);
    let frame = section_frame(a, b);
    body.positions
        .iter()
        .zip(body.dominant)
        .filter(|(_, joint)| members.contains(&(**joint as usize)))
        .filter_map(|(p, _)| {
            let p = Vec3A::from_array(*p);
            let t = (p - a).dot(axis) / length2;
            if !(0.1..=0.9).contains(&t) {
                return None;
            }
            let offset = p - (a + axis * t);
            Some(Sample {
                radial: offset.length(),
                lateral: offset.dot(frame.lateral),
                depth: offset.dot(frame.depth),
            })
        })
        .collect()
}

fn percentile(values: &mut [f32], q: f32) -> f32 {
    values.sort_by(f32::total_cmp);
    let i = ((values.len() - 1) as f32 * q).round() as usize;
    values[i]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cylinder of `radius` around the Y axis from `y0` to `y1`, all moved by joint 0.
    fn cylinder(radius_x: f32, radius_z: f32, y0: f32, y1: f32) -> Vec<[f32; 3]> {
        let mut positions = Vec::new();
        for i in 0..20 {
            let y = y0 + (y1 - y0) * i as f32 / 19.0;
            for k in 0..24 {
                let a = k as f32 / 24.0 * std::f32::consts::TAU;
                positions.push([a.cos() * radius_x, y, a.sin() * radius_z]);
            }
        }
        positions
    }

    fn joints(names: &[(&str, [f32; 3])]) -> Vec<(String, Affine3A)> {
        names
            .iter()
            .map(|(n, p)| ((*n).to_owned(), Affine3A::from_translation((*p).into())))
            .collect()
    }

    #[test]
    fn a_round_limb_gets_its_own_radius() {
        let positions = cylinder(0.06, 0.06, 0.1, 0.5);
        let dominant = vec![0u16; positions.len()];
        let joints = joints(&[
            ("thigh_stretch.l", [0.0, 0.5, 0.0]),
            ("thigh_twist.l", [0.0, 0.5, 0.0]),
            ("leg_stretch.l", [0.0, 0.1, 0.0]),
        ]);
        let capsules = fit(&BindBody {
            positions: &positions,
            dominant: &dominant,
            joints: &joints,
        });
        let thigh = capsules.iter().find(|c| c.name == "thigh.l").unwrap();
        assert!((thigh.radius - 0.06).abs() < 0.003, "{thigh:?}");
        assert_eq!(capsules.len(), 1, "only the thigh's joints exist");
    }

    #[test]
    fn a_wide_torso_becomes_two_capsules_as_deep_as_it_is() {
        let positions = cylinder(0.16, 0.1, 0.9, 1.1);
        let dominant = vec![0u16; positions.len()];
        let joints = joints(&[("root.x", [0.0, 1.0, 0.0])]);
        let capsules = fit(&BindBody {
            positions: &positions,
            dominant: &dominant,
            joints: &joints,
        });
        assert_eq!(capsules.len(), 2, "{capsules:?}");
        for c in &capsules {
            assert!((c.radius - 0.1).abs() < 0.01, "{c:?}");
            let side = c.a.x.abs() + c.radius;
            assert!((side - 0.16).abs() < 0.015, "{c:?} misses the flank");
        }
    }
}
