//! The cloth lab's test outfit, generated from a skeleton's bind pose and its fitted body
//! colliders: a cloak, a long skirt, a ponytail, a greatsword and a bow on the back and a sword
//! at the hip. Shapes are plain on purpose; they exist to try the simulation and its colliders.
//! A cloak draped in Blender ([`Garment`]) replaces the generated one when given.
use super::authored::Garment;
use super::body::{BodyCapsule, group};
use super::solver::{
    Capsule, Carrier, Frames, LAYER_GAP, Layer, Link, PRESS_GAP, ParticleDef, PieceDef, Radial,
    Sink, Tether, grid_cells, radial_push,
};
use bevy::math::{Affine3A, Mat3A, Vec2, Vec3A};
use std::collections::HashMap;

/// Which test pieces a character wears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OutfitChoice {
    pub cloak: bool,
    pub skirt: bool,
    pub ponytail: bool,
    pub greatsword: bool,
    pub bow: bool,
    pub side_sword: bool,
}

impl OutfitChoice {
    pub const ALL: Self = Self {
        cloak: true,
        skirt: true,
        ponytail: true,
        greatsword: true,
        bow: true,
        side_sword: true,
    };
}

/// Surface looks the outfit uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Look {
    Wool,
    Linen,
    Hair,
    Steel,
    DarkSteel,
    Leather,
    Wood,
    Bowstring,
}

/// A collider in bind space carried by one of the plan's bones.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CapsuleDef {
    pub bone: u16,
    pub a: Vec3A,
    pub b: Vec3A,
    pub radius: f32,
    pub group: u32,
    /// Bind-space direction a presser holds particles towards; `radius` is then its
    /// thickness and `width` how far across its axis it holds them.
    pub press: Option<Vec3A>,
    pub width: f32,
}

/// A collider a simulated piece makes of two of its particles, for the pieces after it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Provided {
    pub a: u16,
    pub b: u16,
    pub radius: f32,
    pub group: u32,
    /// A presser's direction, bind space, turning with the piece's anchor (see
    /// [`CapsuleDef::press`]).
    pub press: Option<Vec3A>,
    pub width: f32,
}

/// A wrapped sheet that pieces over it stay outside of ([`super::solver::Layer`]): its axis
/// runs up through `origin` (bind space) and turns with `bone`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayerPlan {
    pub bone: u16,
    pub origin: Vec3A,
}

pub(crate) struct PiecePlan {
    pub name: &'static str,
    pub def: PieceDef,
    /// Collider groups it collides with; [`group::LAYERS`] keeps it outside inner layers.
    pub collides: u32,
    pub provides: Vec<Provided>,
    pub layer: Option<LayerPlan>,
    /// Skinned to the piece's render joints.
    pub parts: Vec<(Look, MeshData)>,
    /// For a draped garment, how to tell its drawn surface going through things (a lab
    /// measure).
    pub pierce: Option<PierceCheck>,
}

/// A draped garment's drawn vertices (drape space, skinned to the render joints) and, per
/// vertex, how near it rests to each collider (in the piece's order) it rests within
/// [`PIERCE_NEAR`] of, and how deep inside the draped inner layer under it. A vertex pierces
/// something when it goes [`PIERCE_DEPTH`] deeper than that, or than the surface where it
/// rested clear of it.
pub(crate) struct PierceCheck {
    pub positions: Vec<Vec3A>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub rests: Vec<Vec<(u16, f32)>>,
    pub layer_rests: Vec<f32>,
}

pub(crate) const PIERCE_NEAR: f32 = 0.03;
pub(crate) const PIERCE_DEPTH: f32 = 0.01;

/// A rigid item skinned to one bone; its vertices are in bind space.
pub(crate) struct ItemPlan {
    pub name: &'static str,
    pub bone: u16,
    pub parts: Vec<(Look, MeshData)>,
}

pub(crate) struct OutfitPlan {
    /// Bones the plan refers to by index.
    pub bones: Vec<String>,
    pub capsules: Vec<CapsuleDef>,
    /// In simulation order: a piece that provides a collider comes before those using it.
    pub pieces: Vec<PiecePlan>,
    pub items: Vec<ItemPlan>,
}

/// Plans the chosen outfit. `binds` holds each joint's bind pose by name; `cloak` and `skirt`
/// are garments draped for this body, used in place of the generated ones.
pub(crate) fn plan(
    choice: OutfitChoice,
    binds: &HashMap<String, Affine3A>,
    body: &[BodyCapsule],
    cloak: Option<&Garment>,
    skirt: Option<&Garment>,
) -> OutfitPlan {
    let mut planner = Planner {
        binds,
        body: body.to_vec(),
        bones: Vec::new(),
        capsules: Vec::new(),
        draped_layer: None,
    };
    for capsule in body {
        if let Some(bone) = planner.bone(&capsule.bone) {
            // Cloth sees forearms and hands fuller than they are, and hands out to the
            // fingertips: thinner than the gaps between its collision points, they passed through
            // it. (Upper arms are full enough, and fuller they squeezed the cloth on the
            // shoulders against the torso.)
            let ends = ["forearm", "hand", "calf", "foot"];
            let end = ends.iter().any(|e| capsule.name.starts_with(e));
            let (mut b, mut radius) = (capsule.b, capsule.radius);
            if end && capsule.group & group::ARMS != 0 {
                radius += ARM_MARGIN;
                if capsule.name.starts_with("hand") {
                    b += (capsule.b - capsule.a).normalize_or_zero() * FINGERS;
                }
            }
            planner.capsules.push(CapsuleDef {
                bone,
                a: capsule.a,
                b,
                radius,
                group: capsule.group | if end { group::LIMB_ENDS } else { 0 },
                press: None,
                width: 0.0,
            });
        }
    }
    let mut pieces = Vec::new();
    // Inner layers first: weapons and the cloak go over the skirt, the cloak over the scabbard.
    if choice.skirt {
        pieces.extend(match skirt {
            Some(garment) => planner.draped_skirt(garment, choice.cloak),
            None => planner.skirt(choice.cloak),
        });
    }
    let mut items = Vec::new();
    let mut back_depth = None;
    // Strapped over a cloak, back weapons sit out by its thickness and press it to the back.
    let over = choice.cloak;
    if choice.greatsword
        && let Some((item, depth)) = planner.greatsword(over)
    {
        back_depth = Some(depth);
        items.push(item);
    }
    if choice.bow
        && let Some(item) = planner.bow(back_depth, over)
    {
        items.push(item);
    }
    if choice.side_sword {
        pieces.extend(planner.side_sword());
    }
    if choice.ponytail {
        pieces.extend(planner.ponytail());
    }
    if choice.cloak {
        pieces.extend(match cloak {
            Some(garment) => planner.draped_cloak(garment),
            None => planner.cloak(),
        });
    }
    OutfitPlan {
        bones: planner.bones,
        capsules: planner.capsules,
        pieces,
        items,
    }
}

/// Room a cloak takes under weapons strapped over it: its particles' radius, the cloth, and
/// some play so the back and the weapon do not squeeze it between them.
const CLOAK_ROOM: f32 = 0.025 + PRESS_GAP + 0.015;

/// A carried item's sizes for colliders: round for most garments, flat (thickness and half
/// width) for a cloak under it.
#[derive(Clone, Copy, Debug)]
struct ItemSize {
    round: f32,
    thick: f32,
    wide: f32,
}

struct Planner<'a> {
    binds: &'a HashMap<String, Affine3A>,
    /// The body's colliders, then those of layers already planned, at rest.
    body: Vec<BodyCapsule>,
    bones: Vec<String>,
    capsules: Vec<CapsuleDef>,
    /// A draped skirt's surface as draped, for a draped cloak over it.
    draped_layer: Option<Layer>,
}

/// A draped garment's particles' radius.
const DRAPED_RADIUS: f32 = 0.02;
/// How much fuller cloth sees the arms than they are, and how far past the hand capsule's end
/// (at the middle finger's second joint) the fingertips reach.
const ARM_MARGIN: f32 = 0.01;
const FINGERS: f32 = 0.035;

impl Planner<'_> {
    fn bone(&mut self, name: &str) -> Option<u16> {
        self.binds.get(name)?;
        if let Some(i) = self.bones.iter().position(|b| b == name) {
            return Some(i as u16);
        }
        self.bones.push(name.to_owned());
        Some((self.bones.len() - 1) as u16)
    }

    fn joint(&self, name: &str) -> Option<Vec3A> {
        self.binds.get(name).map(|b| b.translation)
    }

    fn body_capsule(&self, name: &str) -> Option<&BodyCapsule> {
        self.body.iter().find(|c| c.name == name)
    }

    /// Moves a bind-space point out of the body's colliders in `groups`, `margin` clear.
    fn push_out(&self, mut p: Vec3A, groups: u32, margin: f32) -> Vec3A {
        for _ in 0..4 {
            for c in self.body.iter().filter(|c| c.group & groups != 0) {
                let axis = c.b - c.a;
                let t = ((p - c.a).dot(axis) / axis.length_squared().max(1e-9)).clamp(0.0, 1.0);
                let closest = c.a + axis * t;
                let d = p - closest;
                let reach = c.radius + margin;
                if d.length_squared() < reach * reach {
                    p = closest + d.normalize_or(Vec3A::NEG_Z) * reach;
                }
            }
        }
        p
    }

    /// Adds a back item's colliders: an ordinary round one, and a presser as thick and wide as
    /// the item that holds a cloak under it, against the back, however the cloak swings.
    fn item_capsule(&mut self, bone: u16, a: Vec3A, b: Vec3A, size: ItemSize) {
        self.capsules.push(CapsuleDef {
            bone,
            a,
            b,
            radius: size.round,
            group: group::BACK_ITEMS,
            press: None,
            width: 0.0,
        });
        self.capsules.push(CapsuleDef {
            bone,
            a,
            b,
            radius: size.thick,
            group: group::PRESSERS,
            press: Some(Vec3A::Z),
            width: size.wide,
        });
    }

    /// How far out towards `out` the colliders in `groups` reach at height `y`, from an
    /// upright axis through `origin`: the support function of their slices (as circles), so a
    /// sheet laid at it wraps them convexly, across the gap between the legs.
    fn support(&self, origin: Vec3A, y: f32, out: Vec3A, groups: u32) -> f32 {
        let mut far = 0.0f32;
        for c in self.body.iter().filter(|c| c.group & groups != 0) {
            let along = c.b - c.a;
            if along.y.abs() < 0.5 * along.length() {
                continue;
            }
            let t = ((y - c.a.y) / along.y).clamp(0.0, 1.0);
            let centre = c.a + along * t;
            let beyond = y - centre.y;
            if beyond.abs() >= c.radius {
                continue;
            }
            let slice = (c.radius * c.radius - beyond * beyond).sqrt();
            let flat = Vec3A::new(centre.x - origin.x, 0.0, centre.z - origin.z);
            far = far.max(flat.dot(out) + slice);
        }
        far
    }

    /// How far behind the back (most negative Z) a straight item from `top` to `bottom` must
    /// lie to clear the torso by `margin`.
    fn clear_depth(&self, top: Vec2, bottom: Vec2, margin: f32) -> f32 {
        let mut z = 0.0;
        loop {
            let clear = (0..=16).all(|i| {
                let p = top.lerp(bottom, i as f32 / 16.0);
                let q = Vec3A::new(p.x, p.y, z);
                self.push_out(q, group::TORSO | group::LAYERS, margin) == q
            });
            if clear || z < -0.6 {
                return z;
            }
            z -= 0.005;
        }
    }

    /// A cape: a collar round the neck, a mantle over the shoulders held close to the upper arms
    /// (it lies over them as the bind pose holds them, so it comes down with them) and close to
    /// the shoulder blades behind, and a body hanging down the back, out to the buttocks.
    fn cloak(&mut self) -> Option<PiecePlan> {
        let chest = self.bone("spine_05.x")?;
        let hips = self.bone("root.x")?;
        let arms = [self.bone("arm_stretch.r")?, self.bone("arm_stretch.l")?];
        let collar_bones = [self.bone("shoulder.r")?, self.bone("shoulder.l")?];
        let neck = self.joint("subneck_twist_1.x")?;
        let shoulder = self.joint("c_arm_twist_offset.l")?;
        let elbow = self.joint("forearm_stretch.l")?;
        let knee = self.joint("leg_stretch.l")?;
        let pelvis = self.joint("root.x")?;
        let neck_radius = self.body_capsule("neck").map_or(0.055, |c| c.radius);
        // Over the arms the mantle droops as the bind pose's upper arm does, so it lies along
        // it; behind, it runs steeply down the shoulder blades.
        let arm = elbow - shoulder;
        let arm_droop = (-arm.y).atan2(arm.x.abs());
        const COLUMNS: usize = 13;
        const MANTLE: usize = 3;
        const HANGING: usize = 6;
        let rows = 1 + MANTLE + HANGING;
        let spread = 110f32.to_radians();
        let collar_y = neck.y - 0.01;
        let collar = neck_radius + 0.03;
        let bottom = knee.y - 0.15;
        let axis = Vec3A::new(0.0, 0.0, pelvis.z);
        let hanging_y = |end: f32, h: usize| end + (bottom - end) * h as f32 / HANGING as f32;
        let mut particles = Vec::with_capacity(rows * COLUMNS);
        let mut ends = [Vec3A::ZERO; COLUMNS];
        for r in 0..rows {
            for (c, end) in ends.iter_mut().enumerate() {
                let theta = -spread + 2.0 * spread * c as f32 / (COLUMNS - 1) as f32;
                let (s, k) = theta.sin_cos();
                // Out from the neck: back at 0, the character's left (+X) at +90°.
                let out = Vec3A::new(s, 0.0, -k);
                let side = smoothstep(0.45, 0.9, s.abs());
                // The outer columns catch the wind all the way up.
                let edge = smoothstep(0.8, 1.0, theta.abs() / spread);
                let arm = arms[usize::from(s > 0.0)];
                let start = Vec3A::new(s * collar, collar_y, neck.z - k * collar * 0.9);
                let start = self.push_out(start, group::TORSO | group::HEAD, 0.025);
                let mut particle = ParticleDef {
                    rest: start,
                    carrier: Carrier::blended(
                        chest,
                        collar_bones[usize::from(s > 0.0)],
                        0.3 * side,
                    ),
                    pinned: true,
                    max_distance: 0.0,
                    goal: 0.0,
                    radius: 0.025,
                    exposure: 0.0,
                };
                if (1..=MANTLE).contains(&r) {
                    // Down the slope of the shoulders to the elbows at the sides, a little way
                    // down the shoulder blades behind.
                    let droop = 70f32.to_radians() + (arm_droop - 70f32.to_radians()) * side;
                    let to_elbow = (elbow.x.abs() - collar + 0.02) / arm_droop.cos();
                    let length = 0.14 + (to_elbow - 0.14) * side;
                    let along = length * r as f32 / MANTLE as f32;
                    let slope = out * droop.cos() + Vec3A::NEG_Y * droop.sin();
                    let p = self.push_out(
                        start + slope * along,
                        group::TORSO | group::HEAD | group::ARMS,
                        0.03,
                    );
                    // Over the arms it follows them closely without being pinned to them (a
                    // pinned edge whipped the cape at every arm swing); behind, it settles on the
                    // back.
                    particle.rest = p;
                    particle.carrier =
                        Carrier::blended(chest, arm, side * (0.3 + 0.4 * r as f32 / MANTLE as f32));
                    particle.pinned = false;
                    particle.max_distance = 0.03 + 0.12 * (1.0 - side);
                    particle.goal = 25.0 * side;
                    particle.exposure = 0.5 * edge;
                    if r == MANTLE {
                        *end = p;
                    }
                } else if r > MANTLE {
                    let h = r - MANTLE;
                    let f = h as f32 / HANGING as f32;
                    let end = *end;
                    let towards = Vec3A::new(end.x - axis.x, 0.0, end.z - axis.z);
                    let from_end = towards.length();
                    let dir = towards.normalize_or(out);
                    // Behind, it lies on whatever reaches out furthest from here down (the
                    // shoulder blades, then the buttocks): cloth bridges a hollow back.
                    // (Not out to a skirt's hem: the layer keeps the hanging cape off it.)
                    let bulk = group::TORSO | group::LEGS;
                    let body = (h..=HANGING)
                        .map(|k| self.support(axis, hanging_y(end.y, k), dir, bulk))
                        .fold(0.0f32, f32::max)
                        + 0.035;
                    let groups = bulk | group::LAYERS | group::HIP_ITEMS;
                    let radius = body + (from_end - body) * side + 0.03 * f;
                    let p = Vec3A::new(axis.x, hanging_y(end.y, h), axis.z) + dir * radius;
                    particle.rest = self.push_out(p, groups, 0.03);
                    particle.carrier = if side > 0.3 {
                        Carrier::blended(chest, arm, side * 0.6 * (1.0 - f))
                    } else {
                        Carrier::blended(chest, hips, 0.35 * f)
                    };
                    particle.pinned = false;
                    particle.max_distance = 0.04 + 0.55 * f.powf(1.2);
                    // Air reaches under the cape from its hem up: the top lies in still air.
                    particle.exposure = (0.1 + 0.9 * smoothstep(0.15, 1.0, f)).max(0.6 * edge);
                }
                particles.push(particle);
            }
        }
        let (links, tethers) = sheet_constraints(&particles, COLUMNS, rows, false);
        let def = PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Grid {
                columns: COLUMNS,
                rows,
                wrap: false,
            },
            anchor: chest,
            inertia: 0.7,
            damping: 0.4,
            drag: 0.8,
            lift: 2.0,
            friction: 0.35,
            radial: None,
            self_collide: true,
            shape: Vec::new(),
            sinks: Vec::new(),
            layer_reach: Vec::new(),
        };
        let mut mesh = MeshData::default();
        mesh.add_sheet(&def, 3, Vec2::new(0.0, neck.z));
        Some(PiecePlan {
            pierce: None,
            name: "cloak",
            def,
            collides: group::TORSO
                | group::ARMS
                | group::LEGS
                | group::PRESSERS
                | group::HIP_ITEMS
                | group::LAYERS,
            provides: Vec::new(),
            layer: None,
            parts: vec![(Look::Wool, mesh)],
        })
    }

    /// A garment's particles as authored: carried by its bones (`fallback` if none are on this
    /// skeleton), as free as authored, `RADIUS` round.
    fn draped_particles(&mut self, garment: &Garment, fallback: u16) -> Vec<ParticleDef> {
        let mut particles = Vec::with_capacity(garment.particles.len());
        for (i, authored) in garment.particles.iter().enumerate() {
            let bones: Vec<(u16, f32)> = authored
                .bones
                .iter()
                .filter_map(|(name, weight)| Some((self.bone(name)?, *weight)))
                .collect();
            particles.push(ParticleDef {
                rest: garment.rest(i),
                carrier: if bones.is_empty() {
                    Carrier::on(fallback)
                } else {
                    Carrier::weighted(bones)
                },
                pinned: authored.pinned,
                max_distance: authored.max_distance,
                goal: authored.goal,
                radius: DRAPED_RADIUS,
                exposure: authored.exposure,
            });
        }
        particles
    }

    /// Where a draped garment rests closer to a collider than a particle's radius, or inside one
    /// (capsules are rounder than shoulders and hips), that particle comes that close to that
    /// collider and no closer ([`Sink`]): the body's colliders hold the drape where it lies
    /// instead of pushing it off, and still keep it out of everything else. Colliders are those
    /// the solver will give the piece, in its order, placed as in the pose draped in; a piece
    /// with an axis meets its groups by their hull about it (`radial`: the axis placed so, and
    /// its [`Radial`]), as the solver does. Pressers hold the cloth wherever it rests.
    fn drape_sinks(
        &self,
        garment: &Garment,
        shape: &[Vec3A],
        collides: u32,
        radial: Option<(Vec3A, Vec3A, Radial)>,
    ) -> Vec<Sink> {
        // Each with whether cloth may rest inside it: not a forearm, hand, calf or foot, which
        // swing about under the cloth, so resting inside one in one pose would let it through
        // the cloth in every other.
        let colliders: Vec<Option<(Capsule, bool)>> = self
            .capsules
            .iter()
            .filter(|c| c.group & collides != 0)
            .map(|c| {
                let pose = garment.pose(&self.bones[c.bone as usize]);
                let capsule = Capsule {
                    bulk: radial.is_some_and(|(_, _, r)| c.group & r.groups != 0),
                    ..Capsule::new(
                        pose.transform_point3a(c.a),
                        pose.transform_point3a(c.b),
                        c.radius,
                    )
                };
                let limb = c.group & group::LIMB_ENDS != 0;
                c.press.is_none().then_some((capsule, !limb))
            })
            .collect();
        // The particles, then the cells' centres, as the solver numbers its collision points.
        let mut points = shape.to_vec();
        points.extend(
            grid_cells(garment.columns, garment.rows, garment.wrap)
                .into_iter()
                .map(|cell| cell.iter().map(|&i| shape[i]).sum::<Vec3A>() * 0.25),
        );
        let mut sinks = Vec::new();
        for (i, &p) in points.iter().enumerate() {
            for (k, capsule) in colliders.iter().enumerate() {
                let Some((capsule, inside)) = capsule else {
                    continue;
                };
                let hull = radial.and_then(|(origin, up, axis)| {
                    radial_push(p, capsule, DRAPED_RADIUS, origin, up, axis.reach)
                        .map(|out| DRAPED_RADIUS - out.length())
                });
                let room = hull.unwrap_or_else(|| p.distance(capsule.closest(p)) - capsule.radius);
                if room < DRAPED_RADIUS {
                    sinks.push(Sink {
                        particle: i as u16,
                        collider: k as u16,
                        radius: (room - 0.003).max(if *inside { -0.08 } else { 0.0 }),
                    });
                }
            }
        }
        bevy::log::info!(
            "Cloth: a draped garment rests close to colliders at {} of its {} particles, inside \
             them at {}",
            sinks
                .iter()
                .map(|s| s.particle)
                .collect::<std::collections::HashSet<_>>()
                .len(),
            shape.len(),
            sinks.iter().filter(|s| s.radius < -0.003).count()
        );
        sinks
    }

    /// [`PierceCheck`] for a draped garment's drawn mesh against the colliders in `collides`
    /// (pressers aside) and the draped layer under it, placed as in the pose draped in.
    fn pierce_check(&self, garment: &Garment, mesh: &MeshData, collides: u32) -> PierceCheck {
        let colliders: Vec<Option<Capsule>> = self
            .capsules
            .iter()
            .filter(|c| c.group & collides != 0)
            .map(|c| {
                let pose = garment.pose(&self.bones[c.bone as usize]);
                c.press.is_none().then(|| {
                    Capsule::new(
                        pose.transform_point3a(c.a),
                        pose.transform_point3a(c.b),
                        c.radius,
                    )
                })
            })
            .collect();
        let positions: Vec<Vec3A> = mesh
            .positions
            .iter()
            .map(|p| Vec3A::from_array(*p))
            .collect();
        let rests = positions
            .iter()
            .map(|&p| {
                colliders
                    .iter()
                    .enumerate()
                    .filter_map(|(k, c)| {
                        let c = c.as_ref()?;
                        let room = p.distance(c.closest(p)) - c.radius;
                        (room < PIERCE_NEAR).then_some((k as u16, room))
                    })
                    .collect()
            })
            .collect();
        let layer_rests = match &self.draped_layer {
            Some(layer) if collides & group::LAYERS != 0 => positions
                .iter()
                .map(|&p| layer.push_out(p, 0.0).map_or(0.0, |out| out.distance(p)))
                .collect(),
            _ => Vec::new(),
        };
        PierceCheck {
            positions,
            joints: mesh.joints.clone(),
            weights: mesh.weights.clone(),
            rests,
            layer_rests,
        }
    }

    /// Links and tethers with the lengths and folds as draped.
    fn draped_constraints(
        particles: &[ParticleDef],
        shape: &[Vec3A],
        garment: &Garment,
    ) -> (Vec<Link>, Vec<Tether>) {
        let draped: Vec<ParticleDef> = particles
            .iter()
            .zip(shape)
            .map(|(p, &rest)| ParticleDef { rest, ..p.clone() })
            .collect();
        sheet_constraints(&draped, garment.columns, garment.rows, garment.wrap)
    }

    /// A cloak draped in Blender: its particles are the drape's coarse grid, carried by the
    /// bones and as free as authored, held where they rest ([`Self::drape_sinks`]). Draped over
    /// a draped skirt, it keeps as far outside it as it rests there, not a fixed gap.
    fn draped_cloak(&mut self, garment: &Garment) -> Option<PiecePlan> {
        let chest = self.bone("spine_05.x")?;
        let collides = group::TORSO
            | group::ARMS
            | group::LEGS
            | group::PRESSERS
            | group::HIP_ITEMS
            | group::LAYERS;
        let hips = self.bone("root.x")?;
        let pelvis = self.joint("root.x")?;
        let particles = self.draped_particles(garment, chest);
        let shape: Vec<Vec3A> = (0..particles.len()).map(|i| garment.drape(i)).collect();
        // It does not fall in towards the body's axis where an arm swung away from under it, so
        // the arm meets it from inside when it swings back.
        let (columns, rows) = (garment.columns, garment.rows);
        let row_height = |r: usize| (0..columns).map(|c| shape[r * columns + c].y).sum::<f32>();
        let radial = Radial {
            bone: hips,
            origin: pelvis,
            reach: (row_height(0) - row_height(rows - 1)) / columns as f32 / (rows - 1) as f32
                * 0.5,
            groups: 0,
            inward: Some(0.03),
        };
        let pose = garment.pose("root.x");
        let axis = (
            pose.transform_point3a(radial.origin),
            pose.matrix3.mul_vec3a(Vec3A::Y).normalize_or(Vec3A::Y),
            radial,
        );
        let sinks = self.drape_sinks(garment, &shape, collides, Some(axis));
        let layer_reach = match &self.draped_layer {
            Some(layer) => {
                let gap = DRAPED_RADIUS + LAYER_GAP;
                shape
                    .iter()
                    .map(|&p| {
                        let room = layer
                            .push_out(p, gap)
                            .map_or(gap, |outside| gap - outside.distance(p));
                        (room - 0.003).clamp(-0.03, gap)
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let (links, tethers) = Self::draped_constraints(&particles, &shape, garment);
        let def = PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Grid {
                columns: garment.columns,
                rows: garment.rows,
                wrap: garment.wrap,
            },
            anchor: chest,
            inertia: 0.7,
            damping: 0.4,
            drag: 0.8,
            lift: 2.0,
            friction: 0.35,
            radial: Some(radial),
            self_collide: true,
            shape,
            sinks,
            layer_reach,
        };
        let mesh = garment.mesh();
        let pierce = self.pierce_check(garment, &mesh, collides);
        Some(PiecePlan {
            pierce: Some(pierce),
            name: "cloak",
            def,
            collides,
            provides: Vec::new(),
            layer: None,
            parts: vec![(Look::Wool, mesh)],
        })
    }

    /// A skirt draped in Blender (a wrapped grid), held where it rests like the draped cloak;
    /// it meets the body by the hull of its bulk about the waistband's axis, as the generated
    /// skirt does, and is the layer pieces over it stay outside.
    fn draped_skirt(&mut self, garment: &Garment, under_cloak: bool) -> Option<PiecePlan> {
        let hips = self.bone("root.x")?;
        let collides =
            group::TORSO | group::LEGS | group::ARMS | group::BACK_ITEMS | group::HIP_ITEMS;
        let (columns, rows) = (garment.columns, garment.rows);
        let mut particles = self.draped_particles(garment, hips);
        let shape: Vec<Vec3A> = (0..particles.len()).map(|i| garment.drape(i)).collect();
        // The axis runs up through the waistband's middle, carried by the pelvis.
        let origin = (0..columns).map(|c| garment.rest(c)).sum::<Vec3A>() / columns as f32;
        let hem = (0..columns)
            .map(|c| shape[(rows - 1) * columns + c].y)
            .sum::<f32>()
            / columns as f32;
        let top = (0..columns).map(|c| shape[c].y).sum::<f32>() / columns as f32;
        let reach = (top - hem) / (rows - 1) as f32 * 0.5;
        // A cloak over it keeps the wind off its back.
        if under_cloak {
            for p in &mut particles {
                if p.rest.z < origin.z {
                    p.exposure *= 0.4;
                }
            }
        }
        let pose = garment.pose("root.x");
        let (_, turn, _) = pose.to_scale_rotation_translation();
        let radial = Radial {
            bone: hips,
            origin,
            reach,
            groups: group::TORSO | group::LEGS,
            inward: None,
        };
        let axis = (
            pose.transform_point3a(origin),
            pose.matrix3.mul_vec3a(Vec3A::Y).normalize_or(Vec3A::Y),
            radial,
        );
        let sinks = self.drape_sinks(garment, &shape, collides, Some(axis));
        // Its surface as draped, for a draped cloak over it.
        let angles: Vec<f32> = particles[..columns]
            .iter()
            .map(|p| (p.rest.x - origin.x).atan2(p.rest.z - origin.z))
            .collect();
        let mut layer = Layer::new(rows, &angles);
        layer.update(&shape, axis.0, Mat3A::from_quat(turn));
        // For planning what goes over it, every column stands in for the skirt at rest.
        for c in 0..columns {
            self.body.push(BodyCapsule {
                name: "skirt".into(),
                bone: "root.x".into(),
                a: particles[c].rest,
                b: particles[(rows - 1) * columns + c].rest,
                radius: 0.03,
                group: group::LAYERS,
            });
        }
        let (links, tethers) = Self::draped_constraints(&particles, &shape, garment);
        let def = PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Grid {
                columns,
                rows,
                wrap: garment.wrap,
            },
            anchor: hips,
            inertia: 0.6,
            damping: 0.6,
            drag: 0.8,
            lift: 2.0,
            friction: 0.4,
            radial: Some(radial),
            self_collide: false,
            shape,
            sinks,
            layer_reach: Vec::new(),
        };
        let mesh = garment.mesh();
        let pierce = self.pierce_check(garment, &mesh, collides);
        // Its surface as draped, for a draped cloak over it (after its own check: it is not
        // over itself).
        self.draped_layer = Some(layer);
        Some(PiecePlan {
            pierce: Some(pierce),
            name: "skirt",
            def,
            collides,
            provides: Vec::new(),
            layer: Some(LayerPlan { bone: hips, origin }),
            parts: vec![(Look::Linen, mesh)],
        })
    }

    /// A skirt laid round the hips and thighs (their convex hull) with a little ease, flaring to
    /// the hem; it follows the pelvis at the waistband and both thighs, smoothly blended, below.
    fn skirt(&mut self, under_cloak: bool) -> Option<PiecePlan> {
        let hips = self.bone("root.x")?;
        let left = self.bone("thigh_stretch.l")?;
        let right = self.bone("thigh_stretch.r")?;
        let pelvis = self.joint("root.x")?;
        let knee = self.joint("leg_stretch.l")?;
        let (columns, rows) = (16, 6);
        let top = pelvis.y + 0.05;
        let bottom = knee.y - 0.08;
        let centre = pelvis.z - 0.008;
        let axis = Vec3A::new(0.0, 0.0, centre);
        let mut particles = Vec::new();
        for r in 0..rows {
            let f = r as f32 / (rows - 1) as f32;
            let y = top + (bottom - top) * f;
            for c in 0..columns {
                let phi = (c as f32 + 0.5) / columns as f32 * std::f32::consts::TAU;
                let (s, k) = phi.sin_cos();
                let out = Vec3A::new(s, 0.0, k);
                let radius =
                    self.support(axis, y, out, group::TORSO | group::LEGS) + 0.02 + 0.05 * f;
                let p = Vec3A::new(axis.x, y, axis.z) + out * radius;
                let p = self.push_out(p, group::TORSO | group::LEGS, 0.03);
                let toward_left = smoothstep(-0.12, 0.12, p.x);
                let legs = 0.45 * f;
                particles.push(ParticleDef {
                    rest: p,
                    carrier: Carrier::weighted([
                        (hips, 1.0 - legs),
                        (left, legs * toward_left),
                        (right, legs * (1.0 - toward_left)),
                    ]),
                    pinned: r == 0,
                    max_distance: 0.02 + 0.32 * f,
                    goal: 0.0,
                    radius: 0.03,
                    // A cloak over it keeps the wind off its back.
                    exposure: if under_cloak && k < 0.0 { 0.25 } else { 0.6 },
                });
            }
        }
        let (links, tethers) = sheet_constraints(&particles, columns, rows, true);
        let def = PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Grid {
                columns,
                rows,
                wrap: true,
            },
            anchor: hips,
            inertia: 0.6,
            damping: 0.6,
            drag: 0.8,
            lift: 2.0,
            friction: 0.4,
            radial: Some(Radial {
                bone: hips,
                origin: Vec3A::new(0.0, pelvis.y, centre),
                reach: (top - bottom) / (rows - 1) as f32 * 0.5,
                groups: group::TORSO | group::LEGS,
                inward: None,
            }),
            self_collide: false,
            shape: Vec::new(),
            sinks: Vec::new(),
            layer_reach: Vec::new(),
        };
        let mut mesh = MeshData::default();
        mesh.add_sheet(&def, 3, Vec2::new(0.0, centre));
        // For planning what goes over it, every column stands in for the skirt at rest.
        for c in 0..columns {
            let (a, b) = (
                def.particles[c].rest,
                def.particles[(rows - 1) * columns + c].rest,
            );
            self.body.push(BodyCapsule {
                name: "skirt".into(),
                bone: "root.x".into(),
                a,
                b,
                radius: 0.03,
                group: group::LAYERS,
            });
        }
        Some(PiecePlan {
            pierce: None,
            name: "skirt",
            def,
            collides: group::TORSO
                | group::LEGS
                | group::ARMS
                | group::BACK_ITEMS
                | group::HIP_ITEMS,
            provides: Vec::new(),
            layer: Some(LayerPlan {
                bone: hips,
                origin: Vec3A::new(0.0, pelvis.y, centre),
            }),
            parts: vec![(Look::Linen, mesh)],
        })
    }

    fn ponytail(&mut self) -> Option<PiecePlan> {
        let head = self.bone("head.x")?;
        let skull = self.body_capsule("head")?.clone();
        let root = skull.b + Vec3A::new(0.0, -0.01, -(skull.radius + 0.015));
        const COUNT: usize = 7;
        let goals = [0.0, 30.0, 8.0, 3.0, 1.5, 0.8, 0.4];
        let reach = [0.0, 0.02, 0.06, 0.12, 0.2, 0.3, 0.4];
        let mut particles = Vec::new();
        for i in 0..COUNT {
            let p = root + Vec3A::new(0.0, -0.07 * i as f32, -0.012 * i as f32);
            let p = if i == 0 {
                p
            } else {
                self.push_out(p, group::HEAD | group::TORSO, 0.03)
            };
            particles.push(ParticleDef {
                rest: p,
                carrier: Carrier::on(head),
                pinned: i == 0,
                max_distance: reach[i],
                goal: goals[i],
                radius: 0.022,
                exposure: 1.0,
            });
        }
        let mut links = chain_links(&particles, 0.0);
        links.extend((2..COUNT).map(|i| link(&particles, i - 2, i, 2e-4)));
        let tethers = (1..COUNT)
            .map(|i| Tether {
                particle: i as u16,
                root: 0,
                length: path_length(&particles, 0, i) * 1.02,
            })
            .collect();
        let def = PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Chain { bone: head },
            anchor: head,
            inertia: 0.8,
            damping: 1.2,
            drag: 1.5,
            lift: 0.0,
            friction: 0.2,
            radial: None,
            self_collide: false,
            shape: Vec::new(),
            sinks: Vec::new(),
            layer_reach: Vec::new(),
        };
        let rings: Vec<Ring> = strand_rings(&def, 3, |s| 0.032 + (0.01 - 0.032) * s);
        let mut mesh = MeshData::default();
        mesh.add_tube(&rings, 8, Vec3A::X, false, true);
        Some(PiecePlan {
            pierce: None,
            name: "ponytail",
            def,
            collides: group::HEAD | group::TORSO | group::BACK_ITEMS,
            provides: Vec::new(),
            layer: None,
            parts: vec![(Look::Hair, mesh)],
        })
    }

    /// Worn, not simulated: it hangs from the hip, carried by the pelvis alone (swinging with the
    /// thigh, it swept the cloak at every step). The cloak stays outside it.
    fn side_sword(&mut self) -> Option<PiecePlan> {
        let hips = self.bone("root.x")?;
        let pelvis = self.joint("root.x")?;
        let pivot = self.push_out(
            pelvis + Vec3A::new(0.17, 0.0, 0.03),
            group::TORSO | group::LAYERS,
            0.04,
        );
        // Angled back and a little out, clear of the thigh as it swings back.
        let d = Vec3A::new(0.3, -0.8, -0.6).normalize();
        let particles: Vec<ParticleDef> = [0.0, 0.42, 0.84]
            .iter()
            .map(|&at| ParticleDef {
                rest: pivot + d * at,
                carrier: Carrier::on(hips),
                pinned: true,
                max_distance: 0.0,
                goal: 0.0,
                radius: 0.035,
                exposure: 0.0,
            })
            .collect();
        let def = PieceDef {
            particles,
            links: Vec::new(),
            tethers: Vec::new(),
            cones: Vec::new(),
            frames: Frames::Rigid { bone: hips },
            anchor: hips,
            inertia: 1.0,
            damping: 0.0,
            drag: 0.0,
            lift: 0.0,
            friction: 0.0,
            radial: None,
            self_collide: false,
            shape: Vec::new(),
            sinks: Vec::new(),
            layer_reach: Vec::new(),
        };
        // Flat against the leg: the blade's width lies fore and aft.
        let width = d.cross(Vec3A::X).normalize();
        let thickness = d.cross(width).normalize();
        // For planning the cloak over it.
        self.body.push(BodyCapsule {
            name: "scabbard".into(),
            bone: "root.x".into(),
            a: pivot,
            b: pivot + d * 0.84,
            radius: 0.035,
            group: group::HIP_ITEMS,
        });
        // Scabbard, its chape and throat, then the hilt: guard, grip and pommel.
        let boxes = [
            (Look::Leather, 0.44, 0.43, 0.028, 0.014),
            (Look::DarkSteel, 0.85, 0.03, 0.031, 0.017),
            (Look::DarkSteel, 0.03, 0.03, 0.031, 0.017),
            (Look::Steel, -0.02, 0.012, 0.09, 0.016),
            (Look::Leather, -0.12, 0.09, 0.016, 0.016),
            (Look::Steel, -0.23, 0.02, 0.025, 0.025),
        ];
        Some(PiecePlan {
            pierce: None,
            name: "hip sword",
            def,
            collides: 0,
            layer: None,
            // Round, for the skirt and the cloak. A presser holding the cloak outside it swept
            // the cloak at every step (cape jitter 10.1 mm against 6.6); the cloak's rest
            // shape starts outside it instead.
            provides: vec![Provided {
                a: 0,
                b: 2,
                radius: 0.03,
                group: group::HIP_ITEMS,
                press: None,
                width: 0.0,
            }],
            parts: boxes_by_look(boxes.map(|(look, at, half, w, t)| {
                (look, pivot + d * at, [d * half, width * w, thickness * t])
            })),
        })
    }

    /// Across the back from over the right shoulder (the hilt) to the left hip, flat against the
    /// back (over a cloak when there is one). Returns the item and how deep behind the back it
    /// lies.
    fn greatsword(&mut self, over_cloak: bool) -> Option<(ItemPlan, f32)> {
        let chest = self.bone("spine_05.x")?;
        let (guard, tip) = (Vec2::new(-0.19, 1.46), Vec2::new(0.2, 0.55));
        const BLADE: ItemSize = ItemSize {
            round: 0.035,
            thick: 0.008,
            wide: 0.03,
        };
        let room = BLADE.thick + if over_cloak { CLOAK_ROOM } else { 0.005 };
        let z = self.clear_depth(guard, tip, room);
        let g = Vec3A::new(guard.x, guard.y, z);
        let t = Vec3A::new(tip.x, tip.y, z);
        let axis = (t - g).normalize();
        let pommel = g - axis * 0.3;
        let width = axis.cross(Vec3A::Z).normalize();
        let length = g.distance(t);
        let z = Vec3A::Z;
        let parts = boxes_by_look([
            (
                Look::Steel,
                (g + t) * 0.5,
                [axis * (length * 0.5), width * 0.028, z * 0.006],
            ),
            (Look::DarkSteel, g, [axis * 0.018, width * 0.14, z * 0.02]),
            (
                Look::Leather,
                g - axis * 0.15,
                [axis * 0.13, width * 0.017, z * 0.017],
            ),
            (
                Look::DarkSteel,
                pommel,
                [axis * 0.025, width * 0.03, z * 0.03],
            ),
        ]);
        self.item_capsule(chest, g, t, BLADE);
        self.item_capsule(
            chest,
            g,
            pommel,
            ItemSize {
                round: 0.03,
                thick: 0.02,
                wide: 0.03,
            },
        );
        let item = ItemPlan {
            name: "greatsword",
            bone: chest,
            parts,
        };
        Some((item, g.z))
    }

    /// Across the back the other way, strung over the greatsword when there is one.
    fn bow(&mut self, outside_of: Option<f32>, over_cloak: bool) -> Option<ItemPlan> {
        let chest = self.bone("spine_05.x")?;
        let (upper, lower) = (Vec2::new(0.24, 1.58), Vec2::new(-0.22, 0.68));
        const STAVE: ItemSize = ItemSize {
            round: 0.03,
            thick: 0.017,
            wide: 0.02,
        };
        let z = match outside_of {
            // Over the greatsword's blade, not through it.
            Some(z) => z - 0.008 - STAVE.thick - 0.004,
            None => {
                let room = STAVE.thick + if over_cloak { CLOAK_ROOM } else { 0.005 };
                self.clear_depth(upper, lower, room)
            }
        };
        let (u, l) = (
            Vec3A::new(upper.x, upper.y, z),
            Vec3A::new(lower.x, lower.y, z),
        );
        // Flat against the back: the stave bends across it, not away from it.
        let across = (l - u).cross(Vec3A::Z).normalize();
        let stave = |s: f32| u.lerp(l, s) + across * (0.4 * s * (1.0 - s));
        let rings: Vec<Ring> = (0..=16)
            .map(|i| {
                let s = i as f32 / 16.0;
                Ring {
                    centre: stave(s),
                    radius: 0.009 + 0.008 * (1.0 - (2.0 * s - 1.0).abs()),
                    influences: [(0, 1.0), (0, 0.0)],
                }
            })
            .collect();
        let mut wood = MeshData::default();
        wood.add_tube(&rings, 8, across, true, true);
        let wrap: Vec<Ring> = (0..=2)
            .map(|i| Ring {
                centre: stave(0.45 + 0.05 * i as f32),
                radius: 0.021,
                influences: [(0, 1.0), (0, 0.0)],
            })
            .collect();
        let mut grip = MeshData::default();
        grip.add_tube(&wrap, 8, across, true, true);
        let string: Vec<Ring> = [u, l]
            .iter()
            .map(|&centre| Ring {
                centre,
                radius: 0.0025,
                influences: [(0, 1.0), (0, 0.0)],
            })
            .collect();
        let mut cord = MeshData::default();
        cord.add_tube(&string, 4, across, false, false);
        for k in 0..3 {
            let (a, b) = (stave(k as f32 / 3.0), stave((k + 1) as f32 / 3.0));
            self.item_capsule(chest, a, b, STAVE);
        }
        Some(ItemPlan {
            name: "bow",
            bone: chest,
            parts: vec![
                (Look::Wood, wood),
                (Look::Leather, grip),
                (Look::Bowstring, cord),
            ],
        })
    }
}

fn smoothstep(low: f32, high: f32, x: f32) -> f32 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Boxes on joint 0, one mesh per look.
fn boxes_by_look<const N: usize>(boxes: [(Look, Vec3A, [Vec3A; 3]); N]) -> Vec<(Look, MeshData)> {
    let mut parts: Vec<(Look, MeshData)> = Vec::new();
    for (look, centre, axes) in boxes {
        let i = match parts.iter().position(|(l, _)| *l == look) {
            Some(i) => i,
            None => {
                parts.push((look, MeshData::default()));
                parts.len() - 1
            }
        };
        parts[i].1.add_box(centre, axes, 0);
    }
    parts
}

fn link(particles: &[ParticleDef], a: usize, b: usize, compliance: f32) -> Link {
    Link {
        a: a as u16,
        b: b as u16,
        rest: particles[a].rest.distance(particles[b].rest),
        compliance,
    }
}

fn chain_links(particles: &[ParticleDef], compliance: f32) -> Vec<Link> {
    (1..particles.len())
        .map(|i| link(particles, i - 1, i, compliance))
        .collect()
}

fn path_length(particles: &[ParticleDef], from: usize, to: usize) -> f32 {
    (from + 1..=to)
        .map(|i| particles[i - 1].rest.distance(particles[i].rest))
        .sum()
}

/// Stretch, shear and bend links of a sheet, and tethers from each particle up its column to
/// the pinned top row.
fn sheet_constraints(
    particles: &[ParticleDef],
    columns: usize,
    rows: usize,
    wrap: bool,
) -> (Vec<Link>, Vec<Tether>) {
    let index = |r: usize, c: usize| r * columns + c % columns;
    let across = if wrap { columns } else { columns - 1 };
    let mut links = Vec::new();
    for r in 0..rows {
        for c in 0..across {
            links.push(link(particles, index(r, c), index(r, c + 1), 0.0));
            if r + 1 < rows {
                links.push(link(particles, index(r, c), index(r + 1, c + 1), 2e-5));
                links.push(link(particles, index(r, c + 1), index(r + 1, c), 2e-5));
            }
            if wrap || c + 2 < columns {
                links.push(link(particles, index(r, c), index(r, c + 2), 4e-4));
            }
        }
    }
    for c in 0..columns {
        for r in 1..rows {
            links.push(link(particles, index(r - 1, c), index(r, c), 0.0));
            if r >= 2 {
                links.push(link(particles, index(r - 2, c), index(r, c), 4e-4));
            }
        }
    }
    let mut tethers = Vec::new();
    for c in 0..columns {
        let mut length = 0.0;
        for r in 1..rows {
            length += particles[index(r - 1, c)]
                .rest
                .distance(particles[index(r, c)].rest);
            tethers.push(Tether {
                particle: index(r, c) as u16,
                root: index(0, c) as u16,
                length: length * 1.05,
            });
        }
    }
    (links, tethers)
}

/// A ring of a tube: where it is, how wide, and which two joints carry it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Ring {
    pub centre: Vec3A,
    pub radius: f32,
    pub influences: [(u16, f32); 2],
}

/// Rings along a strand's particles, `per_segment` per link, carried by the two joints at its
/// ends; `radius` maps 0..1 along the strand to a radius.
fn strand_rings(def: &PieceDef, per_segment: usize, radius: impl Fn(f32) -> f32) -> Vec<Ring> {
    let n = def.particles.len();
    let steps = (n - 1) * per_segment;
    (0..=steps)
        .map(|k| {
            let i = (k / per_segment).min(n - 2);
            let f = (k - i * per_segment) as f32 / per_segment as f32;
            let (a, b) = (def.particles[i].rest, def.particles[i + 1].rest);
            Ring {
                centre: a.lerp(b, f),
                radius: radius(k as f32 / steps as f32),
                influences: [(i as u16, 1.0 - f), ((i + 1) as u16, f)],
            }
        })
        .collect()
}

/// Vertex data of a skinned mesh: up to two joint influences per vertex.
#[derive(Clone, Debug, Default)]
pub(crate) struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

impl MeshData {
    fn vertex(&mut self, p: Vec3A, n: Vec3A, uv: [f32; 2], influences: &[(u16, f32)]) -> u32 {
        let mut joints = [0u16; 4];
        let mut weights = [0f32; 4];
        for (k, (joint, weight)) in influences.iter().take(4).enumerate() {
            joints[k] = *joint;
            weights[k] = *weight;
        }
        self.positions.push(p.to_array());
        self.normals.push(n.to_array());
        self.uvs.push(uv);
        self.joints.push(joints);
        self.weights.push(weights);
        (self.positions.len() - 1) as u32
    }

    /// A triangle wound so its front faces the way its vertices' normals point.
    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        let p = |i: u32| Vec3A::from_array(self.positions[i as usize]);
        let n = |i: u32| Vec3A::from_array(self.normals[i as usize]);
        let face = (p(b) - p(a)).cross(p(c) - p(a));
        if face.dot(n(a) + n(b) + n(c)) >= 0.0 {
            self.indices.extend([a, b, c]);
        } else {
            self.indices.extend([a, c, b]);
        }
    }

    /// A box around `centre` with three half-extent axes, flat-shaded, on one joint.
    fn add_box(&mut self, centre: Vec3A, axes: [Vec3A; 3], joint: u16) {
        for k in 0..3 {
            for sign in [1.0, -1.0] {
                let normal = axes[k] * sign;
                let (u, v) = (axes[(k + 1) % 3], axes[(k + 2) % 3]);
                let n = normal.normalize();
                let corners = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
                let first = self.positions.len() as u32;
                for (cu, cv) in corners {
                    let p = centre + normal + u * cu + v * cv;
                    self.vertex(p, n, [(cu + 1.0) * 0.5, (cv + 1.0) * 0.5], &[(joint, 1.0)]);
                }
                self.triangle(first, first + 1, first + 2);
                self.triangle(first, first + 2, first + 3);
            }
        }
    }

    /// A tube through `rings`, its cross-section turned by parallel transport from `reference`.
    fn add_tube(
        &mut self,
        rings: &[Ring],
        sides: usize,
        reference: Vec3A,
        cap_start: bool,
        cap_end: bool,
    ) {
        let n = rings.len();
        let tangent = |k: usize| {
            let (a, b) = (
                rings[k.saturating_sub(1)].centre,
                rings[(k + 1).min(n - 1)].centre,
            );
            (b - a).normalize_or(Vec3A::Y)
        };
        let mut normal = reference;
        let first = self.positions.len() as u32;
        for (k, ring) in rings.iter().enumerate() {
            let t = tangent(k);
            normal = (normal - t * t.dot(normal)).normalize_or(t.any_orthonormal_vector());
            let binormal = t.cross(normal);
            for s in 0..=sides {
                let a = s as f32 / sides as f32 * std::f32::consts::TAU;
                let radial = normal * a.cos() + binormal * a.sin();
                let influences = ring.influences;
                self.vertex(
                    ring.centre + radial * ring.radius,
                    radial,
                    [s as f32 / sides as f32, k as f32 / (n - 1).max(1) as f32],
                    &influences,
                );
            }
        }
        let stride = (sides + 1) as u32;
        for k in 0..n as u32 - 1 {
            for s in 0..sides as u32 {
                let a = first + k * stride + s;
                let b = a + stride;
                self.triangle(a, b, b + 1);
                self.triangle(a, b + 1, a + 1);
            }
        }
        for (cap, k) in [(cap_start, 0), (cap_end, n - 1)] {
            if !cap {
                continue;
            }
            let outward = if k == 0 { -tangent(0) } else { tangent(n - 1) };
            let ring = rings[k];
            let base = (first + k as u32 * stride) as usize;
            // Cap triangles share the rim's positions but face along the axis.
            let rim: Vec<Vec3A> = self.positions[base..base + stride as usize]
                .iter()
                .map(|p| Vec3A::from_array(*p))
                .collect();
            let centre = self.vertex(ring.centre, outward, [0.5, 0.5], &ring.influences);
            for s in 0..sides {
                let a = self.vertex(rim[s], outward, [0.0, 0.0], &ring.influences);
                let b = self.vertex(rim[s + 1], outward, [1.0, 0.0], &ring.influences);
                self.triangle(centre, a, b);
            }
        }
    }

    /// A sheet over a grid piece's particles, `per_cell` quads along each cell edge, each
    /// vertex carried bilinearly by its cell's four corner joints. Normals face away from the
    /// vertical axis through `axis` (XZ), out from the body. Texture coordinates are metres at
    /// rest, along the rows and down the columns; round a wrapped sheet they are a whole number
    /// of metres, so a fabric meets itself at the seam.
    fn add_sheet(&mut self, def: &PieceDef, per_cell: usize, axis: Vec2) {
        let Frames::Grid {
            columns,
            rows,
            wrap,
        } = def.frames
        else {
            return;
        };
        let span = if wrap { columns } else { columns - 1 };
        let (width, height) = (span * per_cell + 1, (rows - 1) * per_cell + 1);
        let rest = |r: usize, c: usize| def.particles[r * columns + c % columns].rest;
        let mut grid = Vec::with_capacity(width * height);
        for v in 0..height {
            for u in 0..width {
                let (cu, cv) = (u as f32 / per_cell as f32, v as f32 / per_cell as f32);
                let c0 = (cu.floor() as usize).min(span - 1);
                let r0 = (cv.floor() as usize).min(rows - 2);
                let (fu, fv) = (cu - c0 as f32, cv - r0 as f32);
                let corners = [
                    (r0, c0, (1.0 - fu) * (1.0 - fv)),
                    (r0, c0 + 1, fu * (1.0 - fv)),
                    (r0 + 1, c0, (1.0 - fu) * fv),
                    (r0 + 1, c0 + 1, fu * fv),
                ];
                let position = corners
                    .iter()
                    .map(|&(r, c, w)| rest(r, c) * w)
                    .sum::<Vec3A>();
                let influences: Vec<(u16, f32)> = corners
                    .iter()
                    .map(|&(r, c, w)| ((r * columns + c % columns) as u16, w))
                    .collect();
                grid.push((position, influences));
            }
        }
        let at = |u: usize, v: usize| grid[v * width + u].0;
        let mut along = vec![0.0f32; width * height];
        let mut down = vec![0.0f32; width * height];
        for v in 0..height {
            for u in 1..width {
                along[v * width + u] = along[v * width + u - 1] + at(u, v).distance(at(u - 1, v));
            }
            if wrap {
                let round = along[v * width + width - 1];
                let whole = round.round().max(1.0);
                for u in 0..width {
                    along[v * width + u] *= whole / round.max(1e-6);
                }
            }
        }
        for v in 1..height {
            for u in 0..width {
                down[v * width + u] = down[(v - 1) * width + u] + at(u, v).distance(at(u, v - 1));
            }
        }
        let first = self.positions.len() as u32;
        for v in 0..height {
            for u in 0..width {
                let du = at((u + 1).min(width - 1), v) - at(u.saturating_sub(1), v);
                let dv = at(u, (v + 1).min(height - 1)) - at(u, v.saturating_sub(1));
                let (position, influences) = &grid[v * width + u];
                let mut normal = du.cross(dv).normalize_or(Vec3A::NEG_Z);
                let out = Vec3A::new(position.x - axis.x, 0.0, position.z - axis.y);
                if normal.dot(out) < 0.0 {
                    normal = -normal;
                }
                let uv = [along[v * width + u], down[v * width + u]];
                self.vertex(*position, normal, uv, influences);
            }
        }
        for v in 0..height as u32 - 1 {
            for u in 0..width as u32 - 1 {
                let a = first + v * width as u32 + u;
                let b = a + width as u32;
                self.triangle(a, b, b + 1);
                self.triangle(a, b + 1, a + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_faces_outward() {
        let mut mesh = MeshData::default();
        mesh.add_box(Vec3A::ZERO, [Vec3A::X, Vec3A::Y * 2.0, Vec3A::Z * 0.5], 0);
        assert_eq!(mesh.positions.len(), 24);
        for tri in mesh.indices.chunks(3) {
            let p = |i: u32| Vec3A::from_array(mesh.positions[i as usize]);
            let face = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
            let centre = (p(tri[0]) + p(tri[1]) + p(tri[2])) / 3.0;
            assert!(face.dot(centre) > 0.0, "triangle {tri:?} faces inward");
        }
    }

    #[test]
    fn a_sheet_vertex_is_carried_by_weights_summing_to_one() {
        let particles: Vec<ParticleDef> = (0..9)
            .map(|i| ParticleDef {
                rest: Vec3A::new((i % 3) as f32, -((i / 3) as f32), -1.0),
                carrier: Carrier::on(0),
                pinned: false,
                max_distance: 1.0,
                goal: 0.0,
                radius: 0.01,
                exposure: 1.0,
            })
            .collect();
        let def = PieceDef {
            particles,
            links: Vec::new(),
            tethers: Vec::new(),
            cones: Vec::new(),
            frames: Frames::Grid {
                columns: 3,
                rows: 3,
                wrap: false,
            },
            anchor: 0,
            inertia: 1.0,
            damping: 0.0,
            drag: 0.0,
            lift: 0.0,
            friction: 0.0,
            radial: None,
            self_collide: false,
            shape: Vec::new(),
            sinks: Vec::new(),
            layer_reach: Vec::new(),
        };
        let mut mesh = MeshData::default();
        mesh.add_sheet(&def, 2, Vec2::ZERO);
        assert_eq!(mesh.positions.len(), 25);
        for (w, n) in mesh.weights.iter().zip(&mesh.normals) {
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
            // The sheet lies at z = -1, so away from the axis is -Z.
            assert!(n[2] < -0.99);
        }
    }
}
