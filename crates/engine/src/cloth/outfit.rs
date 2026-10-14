//! The cloth lab's test outfit, generated from a skeleton's bind pose and its fitted body
//! colliders: a cloak, a long skirt, a ponytail, a greatsword and a bow on the back and a sword
//! at the hip. Shapes are plain on purpose; they exist to try the simulation and its colliders.
use super::body::{BodyCapsule, group};
use super::solver::{Carrier, Cone, Frames, Link, ParticleDef, PieceDef, Tether};
use bevy::math::{Affine3A, Vec2, Vec3A};
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
    /// Back weapons are strapped over the cloak, pressing it to the back, instead of the cloak
    /// draping over them.
    pub weapons_over_cloak: bool,
}

impl OutfitChoice {
    pub const ALL: Self = Self {
        cloak: true,
        skirt: true,
        ponytail: true,
        greatsword: true,
        bow: true,
        side_sword: true,
        weapons_over_cloak: false,
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
    /// Bind-space direction a presser holds particles towards.
    pub press: Option<Vec3A>,
}

/// A collider a simulated piece makes of two of its particles, for the pieces after it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Provided {
    pub a: u16,
    pub b: u16,
    pub radius: f32,
    pub group: u32,
}

pub(crate) struct PiecePlan {
    pub name: &'static str,
    pub def: PieceDef,
    /// Collider groups it collides with.
    pub collides: u32,
    pub provides: Vec<Provided>,
    /// Skinned to the piece's render joints.
    pub parts: Vec<(Look, MeshData)>,
}

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

/// Plans the chosen outfit. `binds` holds each joint's bind pose by name.
pub(crate) fn plan(
    choice: OutfitChoice,
    binds: &HashMap<String, Affine3A>,
    body: &[BodyCapsule],
) -> OutfitPlan {
    let mut planner = Planner {
        binds,
        body: body.to_vec(),
        bones: Vec::new(),
        capsules: Vec::new(),
    };
    for capsule in body {
        if let Some(bone) = planner.bone(&capsule.bone) {
            planner.capsules.push(CapsuleDef {
                bone,
                a: capsule.a,
                b: capsule.b,
                radius: capsule.radius,
                group: capsule.group,
                press: None,
            });
        }
    }
    let mut items = Vec::new();
    let mut back_depth = None;
    // Strapped over a cloak, weapons sit out by its thickness and press it to the back.
    let over = choice.cloak && choice.weapons_over_cloak;
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
    let mut pieces = Vec::new();
    // Inner layers first: the skirt is a collider for the scabbard and the cloak, the scabbard
    // for the cloak.
    if choice.skirt {
        pieces.extend(planner.skirt());
    }
    if choice.side_sword {
        pieces.extend(planner.side_sword());
    }
    if choice.ponytail {
        pieces.extend(planner.ponytail());
    }
    if choice.cloak {
        pieces.extend(planner.cloak(over));
    }
    OutfitPlan {
        bones: planner.bones,
        capsules: planner.capsules,
        pieces,
        items,
    }
}

struct Planner<'a> {
    binds: &'a HashMap<String, Affine3A>,
    /// The body's colliders, then those of layers already planned, at rest.
    body: Vec<BodyCapsule>,
    bones: Vec<String>,
    capsules: Vec<CapsuleDef>,
}

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

    /// Moves a bind-space point along `outward` until it clears the back items' colliders.
    fn push_past_items(&self, mut p: Vec3A, outward: Vec3A, margin: f32) -> Vec3A {
        for _ in 0..60 {
            let inside = self.capsules.iter().any(|c| {
                if c.group & group::BACK_ITEMS == 0 {
                    return false;
                }
                let axis = c.b - c.a;
                let t = ((p - c.a).dot(axis) / axis.length_squared().max(1e-9)).clamp(0.0, 1.0);
                p.distance(c.a + axis * t) < c.radius + margin
            });
            if !inside {
                break;
            }
            p += outward * 0.01;
        }
        p
    }

    /// Adds an item's colliders: an ordinary one, and a presser for the cloak that keeps it on
    /// one side of the item however it swings: under the item (pressed to the back) when the
    /// item is strapped over it, else over it.
    fn item_capsule(&mut self, bone: u16, a: Vec3A, b: Vec3A, radius: f32, over_cloak: bool) {
        self.capsules.push(CapsuleDef {
            bone,
            a,
            b,
            radius,
            group: group::BACK_ITEMS,
            press: None,
        });
        self.capsules.push(CapsuleDef {
            bone,
            a,
            b,
            radius,
            group: group::PRESSERS,
            press: Some(if over_cloak { Vec3A::Z } else { Vec3A::NEG_Z }),
        });
    }

    /// How far behind the back (most negative Z) a straight item from `top` to `bottom` must
    /// lie to clear the torso by `margin`.
    fn clear_depth(&self, top: Vec2, bottom: Vec2, margin: f32) -> f32 {
        let mut z = 0.0;
        loop {
            let clear = (0..=16).all(|i| {
                let p = top.lerp(bottom, i as f32 / 16.0);
                let q = Vec3A::new(p.x, p.y, z);
                self.push_out(q, group::TORSO, margin) == q
            });
            if clear || z < -0.6 {
                return z;
            }
            z -= 0.005;
        }
    }

    fn cloak(&mut self, under_weapons: bool) -> Option<PiecePlan> {
        let chest = self.bone("spine_05.x")?;
        let left = self.bone("shoulder.l")?;
        let right = self.bone("shoulder.r")?;
        let hips = self.bone("root.x")?;
        let shoulder = self.joint("c_arm_twist_offset.l")?;
        let neck = self.joint("subneck_twist_1.x")?;
        let knee = self.joint("leg_stretch.l")?;
        let (columns, rows) = (9, 8);
        let top = shoulder.y + 0.05;
        let bottom = knee.y - 0.2;
        let spread = 75f32.to_radians();
        let mut particles = Vec::new();
        for r in 0..rows {
            let f = r as f32 / (rows - 1) as f32;
            let y = top + (bottom - top) * f;
            let (rx, rz) = (shoulder.x.abs() + 0.02 + 0.2 * f, 0.1 + 0.14 * f);
            for c in 0..columns {
                let theta = -spread + 2.0 * spread * c as f32 / (columns - 1) as f32;
                let (s, k) = theta.sin_cos();
                let p = Vec3A::new(s * rx, y, neck.z - k * rz);
                let p = self.push_out(p, group::TORSO | group::HEAD | group::LAYERS, 0.03);
                // Draped over the weapons, it starts outside them.
                let p = if under_weapons {
                    p
                } else {
                    let outward = Vec3A::new(p.x, 0.0, p.z - neck.z).normalize_or(Vec3A::NEG_Z);
                    self.push_past_items(p, outward, 0.03)
                };
                let carrier = if r == 0 {
                    Carrier::blended(chest, if s > 0.0 { left } else { right }, 0.6 * s * s)
                } else {
                    Carrier::blended(chest, hips, 0.35 * f)
                };
                particles.push(ParticleDef {
                    rest: p,
                    carrier,
                    pinned: r == 0,
                    max_distance: 0.02 + 0.6 * f.powf(1.3),
                    goal: 0.0,
                    radius: 0.025,
                });
            }
        }
        let (links, tethers) = sheet_constraints(&particles, columns, rows, false);
        let def = PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Grid {
                columns,
                rows,
                wrap: false,
            },
            anchor: chest,
            inertia: 0.7,
            damping: 0.4,
            drag: 0.25,
            lift: 2.5,
            friction: 0.35,
        };
        let mut mesh = MeshData::default();
        mesh.add_sheet(&def, 3, Vec2::new(0.0, neck.z));
        Some(PiecePlan {
            name: "cloak",
            def,
            collides: group::TORSO
                | group::ARMS
                | group::LEGS
                | group::PRESSERS
                | group::HIP_ITEMS
                | group::LAYERS,
            provides: Vec::new(),
            parts: vec![(Look::Wool, mesh)],
        })
    }

    fn skirt(&mut self) -> Option<PiecePlan> {
        let hips = self.bone("root.x")?;
        let left = self.bone("thigh_stretch.l")?;
        let right = self.bone("thigh_stretch.r")?;
        let pelvis = self.joint("root.x")?;
        let knee = self.joint("leg_stretch.l")?;
        let (columns, rows) = (16, 6);
        let top = pelvis.y + 0.05;
        let bottom = knee.y - 0.08;
        let centre = pelvis.z - 0.008;
        let mut particles = Vec::new();
        for r in 0..rows {
            let f = r as f32 / (rows - 1) as f32;
            let y = top + (bottom - top) * f;
            let (rx, rz) = (0.15 + 0.15 * f, 0.12 + 0.14 * f);
            for c in 0..columns {
                let phi = (c as f32 + 0.5) / columns as f32 * std::f32::consts::TAU;
                let (s, k) = phi.sin_cos();
                let p = Vec3A::new(s * rx, y, centre + k * rz);
                let p = self.push_out(p, group::TORSO | group::LEGS, 0.035);
                let thigh = if s > 0.0 { left } else { right };
                particles.push(ParticleDef {
                    rest: p,
                    carrier: Carrier::blended(hips, thigh, 0.45 * f),
                    pinned: r == 0,
                    max_distance: 0.02 + 0.32 * f,
                    goal: 0.0,
                    radius: 0.03,
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
            drag: 0.25,
            lift: 1.5,
            friction: 0.4,
        };
        let mut mesh = MeshData::default();
        mesh.add_sheet(&def, 3, Vec2::new(0.0, centre));
        // Every other column, in two pieces, stands in for the skirt under other garments.
        let provides: Vec<Provided> = (0..columns)
            .step_by(2)
            .flat_map(|c| [(0, 2), (2, rows - 1)].map(|(a, b)| (a * columns + c, b * columns + c)))
            .map(|(a, b)| Provided {
                a: a as u16,
                b: b as u16,
                radius: 0.035,
                group: group::LAYERS,
            })
            .collect();
        for p in &provides {
            let (a, b) = (
                def.particles[p.a as usize].rest,
                def.particles[p.b as usize].rest,
            );
            self.body.push(BodyCapsule {
                name: "skirt".into(),
                bone: "root.x".into(),
                a,
                b,
                radius: p.radius,
                group: p.group,
            });
        }
        Some(PiecePlan {
            name: "skirt",
            def,
            collides: group::TORSO | group::LEGS | group::ARMS | group::BACK_ITEMS,
            provides,
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
            drag: 0.6,
            lift: 0.0,
            friction: 0.2,
        };
        let rings: Vec<Ring> = strand_rings(&def, 3, |s| 0.032 + (0.01 - 0.032) * s);
        let mut mesh = MeshData::default();
        mesh.add_tube(&rings, 8, Vec3A::X, false, true);
        Some(PiecePlan {
            name: "ponytail",
            def,
            collides: group::HEAD | group::TORSO | group::BACK_ITEMS,
            provides: Vec::new(),
            parts: vec![(Look::Hair, mesh)],
        })
    }

    fn side_sword(&mut self) -> Option<PiecePlan> {
        let hips = self.bone("root.x")?;
        let pelvis = self.joint("root.x")?;
        let pivot = self.push_out(
            pelvis + Vec3A::new(0.17, 0.0, 0.03),
            group::TORSO | group::LAYERS,
            0.04,
        );
        let d = Vec3A::new(0.15, -0.8, -0.58).normalize();
        let positions = [pivot, pivot + d * 0.42, pivot + d * 0.84];
        let particles: Vec<ParticleDef> = positions
            .iter()
            .enumerate()
            .map(|(i, &p)| ParticleDef {
                rest: p,
                carrier: Carrier::on(hips),
                pinned: i == 0,
                max_distance: [0.0, 0.6, 0.9][i],
                goal: 0.8,
                radius: 0.035,
            })
            .collect();
        let mut links = chain_links(&particles, 0.0);
        links.push(link(&particles, 0, 2, 0.0));
        let def = PieceDef {
            particles,
            links,
            tethers: Vec::new(),
            cones: vec![Cone {
                pivot: 0,
                particle: 2,
                bone: hips,
                axis: d,
                cos_max: 40f32.to_radians().cos(),
            }],
            frames: Frames::Rigid { bone: hips },
            anchor: hips,
            inertia: 0.85,
            damping: 1.0,
            drag: 0.05,
            lift: 0.0,
            friction: 0.3,
        };
        // Flat against the leg: the blade's width lies fore and aft.
        let width = d.cross(Vec3A::X).normalize();
        let thickness = d.cross(width).normalize();
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
            name: "side sword",
            def,
            collides: group::LEGS | group::TORSO | group::LAYERS,
            provides: vec![Provided {
                a: 0,
                b: 2,
                radius: 0.03,
                group: group::HIP_ITEMS,
            }],
            parts: boxes_by_look(boxes.map(|(look, at, half, w, t)| {
                (look, pivot + d * at, [d * half, width * w, thickness * t])
            })),
        })
    }

    /// Across the back from over the right shoulder (the hilt) to the left hip, clear of the
    /// torso. Returns the item and how deep behind the back it lies.
    fn greatsword(&mut self, over_cloak: bool) -> Option<(ItemPlan, f32)> {
        let chest = self.bone("spine_05.x")?;
        let (guard, tip) = (Vec2::new(-0.19, 1.46), Vec2::new(0.2, 0.55));
        let z = self.clear_depth(guard, tip, 0.04) - if over_cloak { 0.05 } else { 0.0 };
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
        for (a, b, radius) in [(g, t, 0.035), (g, pommel, 0.03)] {
            self.item_capsule(chest, a, b, radius, over_cloak);
        }
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
        let z = outside_of.map_or_else(
            || self.clear_depth(upper, lower, 0.03) - if over_cloak { 0.05 } else { 0.0 },
            |z| z - 0.045,
        );
        let (u, l) = (
            Vec3A::new(upper.x, upper.y, z),
            Vec3A::new(lower.x, lower.y, z),
        );
        let stave = |s: f32| u.lerp(l, s) + Vec3A::NEG_Z * (0.52 * s * (1.0 - s));
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
        wood.add_tube(&rings, 8, Vec3A::Z, true, true);
        let wrap: Vec<Ring> = (0..=2)
            .map(|i| Ring {
                centre: stave(0.45 + 0.05 * i as f32),
                radius: 0.021,
                influences: [(0, 1.0), (0, 0.0)],
            })
            .collect();
        let mut grip = MeshData::default();
        grip.add_tube(&wrap, 8, Vec3A::Z, true, true);
        let string: Vec<Ring> = [u, l]
            .iter()
            .map(|&centre| Ring {
                centre,
                radius: 0.0025,
                influences: [(0, 1.0), (0, 0.0)],
            })
            .collect();
        let mut cord = MeshData::default();
        cord.add_tube(&string, 4, Vec3A::Z, false, false);
        for k in 0..3 {
            let (a, b) = (stave(k as f32 / 3.0), stave((k + 1) as f32 / 3.0));
            self.item_capsule(chest, a, b, 0.03, over_cloak);
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
    /// vertical axis through `axis` (XZ), out from the body.
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
                let uv = [
                    u as f32 / (width - 1) as f32,
                    v as f32 / (height - 1) as f32,
                ];
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
