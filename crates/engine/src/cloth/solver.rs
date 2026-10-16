//! Position-based cloth for characters: a few dozen particles per piece that the animation
//! carries, gravity and wind move, and capsules on the body push out. XPBD in small steps
//! (one pass per substep, Macklin et al. 2019), so stiffness does not depend on the frame rate.
//!
//! Everything is in render space. Where the animation alone would put a particle, its target,
//! comes from the bones that carry it as `deform × rest`: `deform` is a bone's skinning
//! transform (its pose times its inverse bind pose) and `rest` the particle's bind-space position.
use bevy::math::{Affine3A, Mat3A, Quat, Vec3A};

/// How far along an item from where it is worn ([`Radial::outward`]) cloth is left to go
/// round it the nearest way.
const WORN_END: f32 = 0.3;
/// How long after a piece is put on (or reset) it is settling: it starts at rest wherever
/// things worn with it are, and [`Radial::outward`] colliders send it round to their far
/// side only then. Moving on, cloth stays on the side it is on by ordinary pushes; sent round
/// all the time, the cloth at an item's sides was flipped back and forth (jogging jitter 5.9 →
/// 8.8 mm).
const SETTLE: f32 = 0.5;

/// How long a presser takes to bring cloth back under it (a time constant). All at once, it
/// fought every substep a hand pushing the cloth out from under it: a bow's lower end at the
/// hip, where the arm swings back.
const PRESS_TIME: f32 = 0.08;
/// The same for a particle past its `max_distance`: a hand swinging back under a cape holds it
/// out past its leash, and pulled back at once it fought the hand every substep and snapped
/// back when let go.
const LEASH_TIME: f32 = 0.04;
/// The outer part of a particle's margin outside a hull ([`radial_push`]), met softly, over
/// [`HULL_SOFT_TIME`]: legs swinging into a skirt ease it out rather than kick it.
const HULL_SOFT: f32 = 0.015;
const HULL_SOFT_TIME: f32 = 0.025;

/// More substeps than this in one frame means a hitch: the frame's time is cut instead.
const MAX_SUBSTEPS: usize = 12;
/// How far beyond a presser's thickness the cloth under it is held: about the cloth's own.
pub(crate) const PRESS_GAP: f32 = 0.008;
/// How far on its wrong side a presser reaches: cloth that has just slipped under is put back,
/// cloth far away is left until it comes near (a scabbard does not yank the cloak off a leg).
const PRESS_DEPTH: f32 = 0.12;
/// How far apart self-colliding particles that are not neighbours keep: about the cloth's
/// thickness in folds, with room for the surface sagging between particles.
const SELF_GAP: f32 = 0.04;
/// Faster than this (m/s) is a solver blow-up, not cloth.
const MAX_SPEED: f32 = 25.0;

/// The bones that carry a particle, weighted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Carrier {
    bones: [u16; 4],
    weights: [f32; 4],
}

impl Carrier {
    pub fn on(bone: u16) -> Self {
        Self::weighted([(bone, 1.0)])
    }

    /// `bone`, blended towards `other` by `blend`.
    pub fn blended(bone: u16, other: u16, blend: f32) -> Self {
        let blend = blend.clamp(0.0, 1.0);
        Self::weighted([(bone, 1.0 - blend), (other, blend)])
    }

    /// Up to four bones; the weights are normalised.
    pub fn weighted(bones: impl IntoIterator<Item = (u16, f32)>) -> Self {
        let mut carrier = Self {
            bones: [0; 4],
            weights: [0.0; 4],
        };
        for (k, (bone, weight)) in bones.into_iter().take(4).enumerate() {
            carrier.bones[k] = bone;
            carrier.weights[k] = weight.max(0.0);
        }
        let total: f32 = carrier.weights.iter().sum();
        for weight in &mut carrier.weights {
            *weight = if total > 0.0 { *weight / total } else { 0.0 };
        }
        carrier
    }

    fn place(self, rest: Vec3A, deforms: &[Affine3A]) -> Vec3A {
        let mut placed = Vec3A::ZERO;
        for (bone, weight) in self.bones.into_iter().zip(self.weights) {
            if weight > 0.0 {
                placed += deforms[bone as usize].transform_point3a(rest) * weight;
            }
        }
        placed
    }
}

/// A piece's upright axis: `origin` (bind space) carried by `bone`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Radial {
    pub bone: u16,
    pub origin: Vec3A,
    /// Half the height between rows: each particle stands for the cloth that far above and
    /// below it, so a knee between two rows still pushes it.
    pub reach: f32,
    /// Upright colliders the piece stays outside the convex hull of, pushed only outward from
    /// the axis, never between them: a skirt spans the gap between the legs.
    pub groups: u32,
    /// Colliders the piece leaves, while it settles after being put on ([`SETTLE`]), by the
    /// nearest way out unless that leads in towards the axis, then by their far side, past
    /// [`WORN_END`] of their length from their first end (where they are worn): a cape put on
    /// where a scabbard at the hip hangs ends up over it. (Not limbs: cloth on the upper back
    /// lies between the upper arm and the body, and sent round the arm it fought the body.)
    pub outward: u32,
    /// How much nearer the axis than its target a particle may come: a cape over an arm that
    /// swings forward stays outside the arm's way, not falling in behind it to be met from
    /// outside (and pushed further in) when the arm swings back.
    pub inward: Option<f32>,
}

#[derive(Clone, Debug)]
pub(crate) struct ParticleDef {
    /// Bind-space position.
    pub rest: Vec3A,
    pub carrier: Carrier,
    /// Held exactly at its target.
    pub pinned: bool,
    /// How far it may stray from its target.
    pub max_distance: f32,
    /// Pull back towards the target, per second.
    pub goal: f32,
    /// Collision radius (see also [`PieceDef::sinks`]).
    pub radius: f32,
    /// How much of the wind reaches it, 0–1: where the cloth lies against the body the air
    /// between moves with the body.
    pub exposure: f32,
}

/// Keeps two particles at their rest distance; `compliance` (m/N at unit mass) softens it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Link {
    pub a: u16,
    pub b: u16,
    pub rest: f32,
    pub compliance: f32,
}

/// Keeps a particle within `length` of a pinned one: no stretching, however few the passes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tether {
    pub particle: u16,
    pub root: u16,
    pub length: f32,
}

/// Keeps `particle` within an angle of a bone-carried direction from `pivot`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cone {
    pub pivot: u16,
    pub particle: u16,
    pub bone: u16,
    /// Bind-space direction.
    pub axis: Vec3A,
    pub cos_max: f32,
}

/// How a piece's render joints turn.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Frames {
    /// A sheet of `columns × rows` particles, row by row; `wrap` closes each row (a skirt).
    Grid {
        columns: usize,
        rows: usize,
        wrap: bool,
    },
    /// A strand: each joint points at the next particle and rolls with `bone`.
    Chain { bone: u16 },
    /// A rigid body hanging from particle 0 towards the last one, rolling with `bone`.
    Rigid { bone: u16 },
}

#[derive(Clone, Debug)]
pub(crate) struct PieceDef {
    pub particles: Vec<ParticleDef>,
    pub links: Vec<Link>,
    pub tethers: Vec<Tether>,
    pub cones: Vec<Cone>,
    pub frames: Frames,
    /// The bone whose movement carries the piece along (see `inertia`).
    pub anchor: u16,
    /// Share of the anchor's movement the piece resists: 1 leaves it in world space, 0 moves it
    /// rigidly with the anchor.
    pub inertia: f32,
    /// Velocity lost per second.
    pub damping: f32,
    /// Pull towards the air's velocity along the surface (strands: in every direction), per
    /// second.
    pub drag: f32,
    /// Air pushing across a sheet: this times the square of the air's speed across it, per
    /// metre.
    pub lift: f32,
    /// Share of sliding velocity lost on contact.
    pub friction: f32,
    /// Wraps round the body about this axis.
    pub radial: Option<Radial>,
    /// Particles that are not neighbours on the sheet keep apart, so folds do not pass
    /// through each other.
    pub self_collide: bool,
    /// The cloth's own rest shape where it differs from the particles' bind-space `rest`: a
    /// garment draped in another pose than the bind pose (arms down) is carried from bind space
    /// but drawn, and its folds kept, as draped. Empty: the rest positions.
    pub shape: Vec<Vec3A>,
    /// Collision points (particles, then a sheet's cell centres) that rest closer to a
    /// collider than their radius, ordered by point.
    pub sinks: Vec<Sink>,
    /// How far outside inner layers each particle keeps, where it was draped over them closer
    /// than its radius and [`LAYER_GAP`]. Empty: that for every particle.
    pub layer_reach: Vec<f32>,
}

/// A particle's radius against one collider (its index among those the piece collides with)
/// where it rests closer to it than its own radius, or inside it: a garment draped on the body
/// where a capsule is rounder than the body is held where it lies, not pushed off.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sink {
    pub particle: u16,
    pub collider: u16,
    pub radius: f32,
}

impl PieceDef {
    /// Render joints: one per particle, or one for a rigid body.
    pub fn joint_count(&self) -> usize {
        match self.frames {
            Frames::Rigid { .. } => 1,
            _ => self.particles.len(),
        }
    }

    /// Particle `i` in the cloth's own rest shape (see [`Self::shape`]).
    pub fn shape(&self, i: usize) -> Vec3A {
        self.shape.get(i).copied().unwrap_or(self.particles[i].rest)
    }

    /// Joint positions in the space the piece's mesh is in. The mesh is drawn as offsets from
    /// where its vertices' joints blend to (see `to_mesh` in the cloth module).
    pub fn joint_rests(&self) -> Vec<Vec3A> {
        match self.frames {
            Frames::Rigid { .. } => vec![self.particles[0].rest],
            _ => (0..self.particles.len()).map(|i| self.shape(i)).collect(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Capsule {
    pub a: Vec3A,
    pub b: Vec3A,
    pub radius: f32,
    /// Zero for an ordinary collider. Otherwise a presser (a strap or a weapon worn over a
    /// cloak): particles under it, within `width` of its axis across it, are held on this side
    /// of it, `radius` (its thickness) plus [`PRESS_GAP`] from the axis.
    pub press: Vec3A,
    pub width: f32,
    /// Of the groups the piece meets by its axis ([`Radial::groups`]): it stays outside their
    /// convex hull about the axis rather than each one.
    pub bulk: bool,
    /// Of the groups the piece leaves only outward from its axis ([`Radial::outward`]).
    pub outward: bool,
    /// Worn over the body (a weapon, a strap): where cloth is caught between it and the body,
    /// it gives way, the body does not.
    pub worn: bool,
}

impl Capsule {
    pub fn new(a: Vec3A, b: Vec3A, radius: f32) -> Self {
        Self {
            a,
            b,
            radius,
            press: Vec3A::ZERO,
            width: 0.0,
            bulk: false,
            outward: false,
            worn: false,
        }
    }

    pub fn lerp(self, other: Self, t: f32) -> Self {
        Self {
            a: self.a.lerp(other.a, t),
            b: self.b.lerp(other.b, t),
            radius: self.radius + (other.radius - self.radius) * t,
            press: self.press.lerp(other.press, t).normalize_or_zero(),
            width: self.width + (other.width - self.width) * t,
            bulk: self.bulk,
            outward: self.outward,
            worn: self.worn,
        }
    }

    /// How far along the axis from `a` to `b` the point nearest `p` is, 0–1.
    pub fn along(&self, p: Vec3A) -> f32 {
        let axis = self.b - self.a;
        ((p - self.a).dot(axis) / axis.length_squared().max(1e-12)).clamp(0.0, 1.0)
    }

    pub fn closest(&self, p: Vec3A) -> Vec3A {
        let axis = self.b - self.a;
        let length2 = axis.length_squared();
        if length2 <= f32::EPSILON {
            return self.a;
        }
        let t = ((p - self.a).dot(axis) / length2).clamp(0.0, 1.0);
        self.a + axis * t
    }
}

/// What a frame of simulation needs besides the piece.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StepParams {
    pub dt: f32,
    pub substep_hz: f32,
    pub gravity: Vec3A,
    /// Wind velocity, m/s.
    pub wind: Vec3A,
    /// How much the wind varies over a piece, 0–1: gusts and eddies running across it.
    pub flutter: f32,
    /// Seconds, for the flutter's motion.
    pub time: f32,
    /// Replaces every piece's own inertia.
    pub inertia: Option<f32>,
    /// Scale every piece's damping and lift.
    pub damping: f32,
    pub lift: f32,
    /// Share of a collider's push applied each substep.
    pub contact: f32,
    /// Whether pieces that ask for self-collision get it.
    pub self_collide: bool,
}

/// An inner garment as the pieces over it see it: its radius around a bone-carried upright
/// axis by angle and height, read from its particles each frame. Unlike capsules standing in
/// for it, it has no gaps for a particle to slip through. Fits garments that go round the body
/// (a skirt, a coat).
#[derive(Clone, Debug)]
pub(crate) struct Layer {
    columns: usize,
    rows: usize,
    origin: Vec3A,
    /// The layer's frame from world: X across, Y up, Z forward.
    inverse: Mat3A,
    basis: Mat3A,
    /// Each of [`LAYER_BINS`] even steps of [`diamond`] angle: the two columns either side of
    /// it at rest, and how far it lies between them. Columns keep their angles about the axis
    /// closely enough that the table need not search them each frame.
    bins: Vec<(u16, u16, f32)>,
    /// Radius and height per row and bin.
    table: Vec<(f32, f32)>,
    top: f32,
    bottom: f32,
    widest: f32,
}

const LAYER_BINS: usize = 48;
/// Clearance kept beyond a particle's radius from the layer under it: the rendered surfaces
/// of both garments sag a little between their particles.
pub(crate) const LAYER_GAP: f32 = 0.035;

/// A monotonic stand-in for the angle of (x, z) from +Z towards +X, in [0, 4): cheaper than
/// `atan2`, and lookups only need the same measure both ways.
fn diamond(x: f32, z: f32) -> f32 {
    let sum = x.abs() + z.abs();
    if sum <= 1e-9 {
        return 0.0;
    }
    match (x >= 0.0, z >= 0.0) {
        (true, true) => x / sum,
        (true, false) => 1.0 + -z / sum,
        (false, false) => 2.0 + -x / sum,
        (false, true) => 3.0 + z / sum,
    }
}

/// The direction a [`diamond`] measure stands for, as (x, z).
fn undiamond(d: f32) -> (f32, f32) {
    let d = d.rem_euclid(4.0);
    let f = d.fract();
    match d as usize {
        0 => (f, 1.0 - f),
        1 => (1.0 - f, -f),
        2 => (-f, -(1.0 - f)),
        _ => (-(1.0 - f), f),
    }
}

impl Layer {
    /// A wrapped sheet's layer; `angles` are its columns' angles about the axis at rest
    /// (`atan2(x, z)`), going round in order.
    pub fn new(rows: usize, angles: &[f32]) -> Self {
        let columns = angles.len();
        let tau = std::f32::consts::TAU;
        let bins = (0..LAYER_BINS)
            .map(|b| {
                let (x, z) = undiamond(b as f32 * 4.0 / LAYER_BINS as f32);
                let at = x.atan2(z);
                let ahead = |c: usize| (angles[c] - at).rem_euclid(tau);
                // The column just past `at` going round, and the one before it.
                let next = (0..columns)
                    .min_by(|&a, &b| ahead(a).total_cmp(&ahead(b)))
                    .unwrap_or(0);
                let previous = (next + columns - 1) % columns;
                let span = (angles[next] - angles[previous]).rem_euclid(tau).max(1e-4);
                let f = ((at - angles[previous]).rem_euclid(tau) / span).clamp(0.0, 1.0);
                (previous as u16, next as u16, f)
            })
            .collect();
        Self {
            columns,
            rows,
            origin: Vec3A::ZERO,
            inverse: Mat3A::IDENTITY,
            basis: Mat3A::IDENTITY,
            bins,
            table: vec![(0.0, 0.0); rows * LAYER_BINS],
            top: 0.0,
            bottom: 0.0,
            widest: 0.0,
        }
    }

    /// Reads the garment's particles (a wrapped sheet, top row first) about `origin`.
    pub fn update(&mut self, positions: &[Vec3A], origin: Vec3A, basis: Mat3A) {
        self.origin = origin;
        self.basis = basis;
        self.inverse = basis.transpose();
        self.widest = 0.0;
        self.top = f32::MIN;
        self.bottom = f32::MAX;
        let polar = |x: Vec3A| {
            let local = self.inverse * (x - origin);
            (local.x.hypot(local.z), local.y)
        };
        for r in 0..self.rows {
            let row = &positions[r * self.columns..(r + 1) * self.columns];
            for (b, &(previous, next, f)) in self.bins.iter().enumerate() {
                let (r0, h0) = polar(row[previous as usize]);
                let (r1, h1) = polar(row[next as usize]);
                self.table[r * LAYER_BINS + b] = (r0 + (r1 - r0) * f, h0 + (h1 - h0) * f);
            }
            for &x in row {
                let (radius, height) = polar(x);
                self.widest = self.widest.max(radius);
                if r == 0 {
                    self.top = self.top.max(height);
                }
                if r == self.rows - 1 {
                    self.bottom = self.bottom.min(height);
                }
            }
        }
    }

    /// Where `x` must go to lie `reach` outside the garment, if it is inside.
    pub fn push_out(&self, x: Vec3A, reach: f32) -> Option<Vec3A> {
        let local = self.inverse * (x - self.origin);
        if local.y > self.top || local.y < self.bottom {
            return None;
        }
        let r = local.x.hypot(local.z);
        if r >= self.widest + reach {
            return None;
        }
        let at = diamond(local.x, local.z) * LAYER_BINS as f32 / 4.0;
        let b0 = (at as usize).min(LAYER_BINS - 1);
        let b1 = (b0 + 1) % LAYER_BINS;
        let f = at - b0 as f32;
        let read = |row: usize| {
            let (r0, h0) = self.table[row * LAYER_BINS + b0];
            let (r1, h1) = self.table[row * LAYER_BINS + b1];
            (r0 + (r1 - r0) * f, h0 + (h1 - h0) * f)
        };
        let (mut upper_r, mut upper_h) = read(0);
        for row in 1..self.rows {
            let (lower_r, lower_h) = read(row);
            if local.y >= lower_h {
                let f = ((upper_h - local.y) / (upper_h - lower_h).max(1e-4)).clamp(0.0, 1.0);
                let wall = upper_r + (lower_r - upper_r) * f + reach;
                if r >= wall {
                    return None;
                }
                let out = if r > 1e-4 {
                    Vec3A::new(local.x / r, 0.0, local.z / r)
                } else {
                    Vec3A::NEG_Z
                };
                let moved = Vec3A::new(out.x * wall, local.y, out.z * wall);
                return Some(self.origin + self.basis * moved);
            }
            (upper_r, upper_h) = (lower_r, lower_h);
        }
        None
    }
}

/// One piece's moving state.
#[derive(Clone, Debug)]
pub(crate) struct PieceSim {
    x: Vec<Vec3A>,
    v: Vec<Vec3A>,
    start: Vec<Vec3A>,
    targets_prev: Vec<Vec3A>,
    targets: Vec<Vec3A>,
    normals: Vec<Vec3A>,
    contacts: Vec<Vec3A>,
    /// Each substep, where particles were before colliders pushed them, and the velocity of
    /// what last pushed each: a push moves cloth but adds no speed beyond its pusher's.
    free: Vec<Vec3A>,
    pushed_by: Vec<Vec3A>,
    anchor_prev: Option<Affine3A>,
    /// Colliders that may touch each particle this frame, as (particle, collider) pairs with
    /// the particle's radius against that collider.
    pairs: Vec<(u16, u16, f32)>,
    /// Room to put `pairs` in order ([`Self::worn_first`]).
    ordered: Vec<(u16, u16, f32)>,
    /// Each collider where it is this substep.
    now: Vec<Capsule>,
    /// Where each collision point's sinks start in [`PieceDef::sinks`], and where the last ends.
    sinks_from: Vec<u32>,
    /// Sheets: each particle's frame at rest, transposed.
    rest_frames: Vec<Mat3A>,
    /// The piece's links with their particles' inverse masses; links between two pinned
    /// particles are left out.
    links: Vec<SolverLink>,
    /// Each collider's bounding sphere, grown by how far it moves this frame.
    bounds: Vec<(Vec3A, f32)>,
    /// How far each particle can get this frame.
    reach: Vec<f32>,
    /// A sheet's cells, corner particles each ([`grid_cells`]): their centres collide too, so
    /// a collider narrower than the gaps between particles (a scabbard, a hand) cannot pass
    /// between them through the cloth drawn there.
    cells: Vec<[u16; 4]>,
    /// Bounding spheres of runs of eight particles, grown by their reach.
    chunks: Vec<(Vec3A, f32)>,
    /// Pairs of particles, not neighbours on the sheet, that could meet this frame, and how far
    /// apart each keeps.
    self_pairs: Vec<(u16, u16, f32)>,
    /// A sheet's particles' rows and columns.
    grid_at: Vec<(u16, u16)>,
    /// Each cell's radius (its corners' mean) and, this frame, its corners' mean target.
    cell_radius: Vec<f32>,
    cell_targets: Vec<Vec3A>,
    /// Each particle's wind this frame.
    gusts: Vec<Vec3A>,
    /// A wrap-around piece's axis this frame: origin, up and the height each particle covers.
    axis: Option<(Vec3A, Vec3A, f32)>,
    /// Simulated time owed from earlier frames, under one substep.
    carry: f32,
    /// Seconds since the piece was put on (or reset) still to count as settling ([`SETTLE`]).
    settling: f32,
    /// Particles a presser held last substep: [`Radial::inward`] leaves them to it (it holds a
    /// cape in against the back, under a weapon).
    pressed: Vec<bool>,
    /// Positions as shown this frame (see [`Self::step`]).
    shown: Vec<Vec3A>,
    /// Each particle's position the last three frames, for [`Self::shake`].
    history: [Vec<Vec3A>; 3],
    target_history: [Vec<Vec3A>; 3],
    shake: f32,
    target_shake: f32,
}

#[derive(Clone, Copy, Debug)]
struct SolverLink {
    a: u16,
    b: u16,
    rest: f32,
    compliance: f32,
    wa: f32,
    wb: f32,
}

impl PieceSim {
    pub fn new(def: &PieceDef) -> Self {
        let n = def.particles.len();
        Self {
            x: def.particles.iter().map(|p| p.rest).collect(),
            v: vec![Vec3A::ZERO; n],
            start: vec![Vec3A::ZERO; n],
            targets_prev: vec![Vec3A::ZERO; n],
            targets: vec![Vec3A::ZERO; n],
            normals: vec![Vec3A::ZERO; n],
            contacts: vec![Vec3A::ZERO; n],
            free: vec![Vec3A::ZERO; n],
            pushed_by: vec![Vec3A::ZERO; n],
            anchor_prev: None,
            pairs: Vec::new(),
            ordered: Vec::new(),
            now: Vec::new(),
            sinks_from: {
                let points = n + grid_cell_count(&def.frames);
                (0..=points)
                    .map(|i| def.sinks.partition_point(|s| (s.particle as usize) < i) as u32)
                    .collect()
            },
            links: independent_batches(
                def.links
                    .iter()
                    .map(|l| SolverLink {
                        a: l.a,
                        b: l.b,
                        rest: l.rest,
                        compliance: l.compliance,
                        wa: weight(&def.particles[l.a as usize]),
                        wb: weight(&def.particles[l.b as usize]),
                    })
                    .filter(|l| l.wa + l.wb > 0.0),
                def.particles.len(),
            ),
            bounds: Vec::new(),
            reach: Vec::new(),
            cells: match def.frames {
                Frames::Grid {
                    columns,
                    rows,
                    wrap,
                } => grid_cells(columns, rows, wrap)
                    .into_iter()
                    .map(|cell| cell.map(|i| i as u16))
                    .collect(),
                _ => Vec::new(),
            },
            chunks: Vec::new(),
            self_pairs: Vec::new(),
            grid_at: Vec::new(),
            cell_radius: Vec::new(),
            cell_targets: Vec::new(),
            axis: None,
            carry: 0.0,
            settling: SETTLE,
            pressed: vec![false; def.particles.len()],
            shown: def.particles.iter().map(|p| p.rest).collect(),
            history: [
                vec![Vec3A::ZERO; n],
                vec![Vec3A::ZERO; n],
                vec![Vec3A::ZERO; n],
            ],
            target_history: [
                vec![Vec3A::ZERO; n],
                vec![Vec3A::ZERO; n],
                vec![Vec3A::ZERO; n],
            ],
            shake: 0.0,
            target_shake: 0.0,
            gusts: vec![Vec3A::ZERO; n],
            rest_frames: match def.frames {
                Frames::Grid {
                    columns,
                    rows,
                    wrap,
                } => {
                    let rests: Vec<Vec3A> =
                        (0..def.particles.len()).map(|i| def.shape(i)).collect();
                    (0..rows * columns)
                        .map(|i| {
                            grid_frame(&rests, columns, rows, wrap, i / columns, i % columns)
                                .transpose()
                        })
                        .collect()
                }
                _ => Vec::new(),
            },
        }
    }

    pub fn positions(&self) -> &[Vec3A] {
        &self.x
    }

    /// Puts every particle where the animation alone would, at rest.
    pub fn reset(&mut self, def: &PieceDef, deforms: &[Affine3A]) {
        for (i, p) in def.particles.iter().enumerate() {
            let target = p.carrier.place(p.rest, deforms);
            self.x[i] = target;
            self.shown[i] = target;
            self.targets[i] = target;
            self.targets_prev[i] = target;
            self.v[i] = Vec3A::ZERO;
        }
        self.anchor_prev = Some(deforms[def.anchor as usize]);
        self.settling = SETTLE;
    }

    /// Advances one frame. `capsules` holds each collider at the start and end of the frame.
    pub fn step(
        &mut self,
        def: &PieceDef,
        deforms: &[Affine3A],
        capsules: &[(Capsule, Capsule)],
        layers: &[&Layer],
        params: &StepParams,
    ) {
        let anchor = deforms[def.anchor as usize];
        let Some(anchor_prev) = self.anchor_prev else {
            self.reset(def, deforms);
            return;
        };
        if params.dt <= 0.0 {
            return;
        }
        self.settling = (self.settling - params.dt).max(0.0);
        for (i, p) in def.particles.iter().enumerate() {
            self.targets_prev[i] = self.targets[i];
            self.targets[i] = p.carrier.place(p.rest, deforms);
        }

        // Carry the piece along with part of its anchor's movement, the frame's starting targets
        // too, so pinned particles do not then drag the carried ones back over the substeps.
        let keep = params.inertia.unwrap_or(def.inertia).clamp(0.0, 1.0);
        if keep < 1.0 {
            let delta = anchor * anchor_prev.inverse();
            for (i, p) in def.particles.iter().enumerate() {
                let carried = delta.transform_point3a(self.targets_prev[i]);
                self.targets_prev[i] = carried.lerp(self.targets_prev[i], keep);
                if !p.pinned {
                    self.x[i] = delta.transform_point3a(self.x[i]).lerp(self.x[i], keep);
                    self.v[i] = delta.matrix3.mul_vec3a(self.v[i]).lerp(self.v[i], keep);
                }
            }
        }
        self.anchor_prev = Some(anchor);

        // Sheets' normals for the wind come from the last frame's joints ([`Self::joints`]).
        self.axis = def.radial.map(|radial| {
            let deform = deforms[radial.bone as usize];
            (
                deform.transform_point3a(radial.origin),
                deform.matrix3.mul_vec3a(Vec3A::Y).normalize_or(Vec3A::Y),
                radial.reach,
            )
        });
        self.gather_pairs(capsules, def, params.dt);
        self.worn_first(capsules);
        self.gather_self_pairs(def, params.self_collide);
        self.gust(def, params);

        // What the substeps' cell collisions need, once a frame, not once a pair.
        if self.cell_radius.len() != self.cells.len() {
            self.cell_radius = self
                .cells
                .iter()
                .map(|corners| {
                    corners
                        .iter()
                        .map(|&c| def.particles[c as usize].radius)
                        .sum::<f32>()
                        * 0.25
                })
                .collect();
        }
        self.cell_targets.clear();
        self.cell_targets.extend(self.cells.iter().map(|corners| {
            corners
                .iter()
                .map(|&c| self.targets[c as usize])
                .sum::<Vec3A>()
                * 0.25
        }));

        // Substeps of a fixed length, the time left over carried to the next frame. Fitting a
        // whole number of substeps to each frame changed their length with the frame time (4 or
        // 5 for frames either side of 16.67 ms), and where cloth settles depends a little on it:
        // the whole piece bounced with the frame pacing, standing still.
        let h = 1.0 / params.substep_hz;
        self.carry = (self.carry + params.dt).min(MAX_SUBSTEPS as f32 * h);
        let substeps = (self.carry / h) as usize;
        self.carry -= substeps as f32 * h;
        for s in 1..=substeps {
            let t = s as f32 / substeps as f32;
            self.substep(def, deforms, capsules, layers, params, h, t);
        }
        if substeps == 0 {
            // No substep this frame: the targets still move on, so pinned particles do.
            for (i, p) in def.particles.iter().enumerate() {
                if p.pinned {
                    self.x[i] = self.targets[i];
                }
            }
        }
        // Shown at the frame's own time: the simulation lags it by `carry`, so free particles
        // move on by their velocity that long. Shown as simulated, a frame of 3, 4 or 5
        // substeps moved cloth 12.5, 16.7 or 20.8 ms while the body moved one frame.
        for (i, p) in def.particles.iter().enumerate() {
            self.shown[i] = if p.pinned {
                self.x[i]
            } else {
                self.x[i] + self.v[i] * self.carry
            };
        }
        self.measure_shake(def);
    }

    /// How much the piece jitters: the mean third difference over frames of its free particles'
    /// positions, metres. Swinging however hard keeps it small (it changes acceleration only
    /// gradually); flicker from frame to frame does not.
    pub fn shake(&self) -> f32 {
        self.shake
    }

    /// A sheet's folds over: neighbouring particles across a row whose sides face more than
    /// about 110° apart (an edge turned back on the cloth beside it), from the last joints,
    /// where they faced within about 70° of each other at rest (a draped pleat is no fold over).
    pub fn folds(&self, def: &PieceDef) -> usize {
        let Frames::Grid {
            columns,
            rows,
            wrap,
        } = def.frames
        else {
            return 0;
        };
        let across = if wrap { columns } else { columns - 1 };
        (0..rows)
            .flat_map(|r| {
                (0..across).map(move |c| (r * columns + c, r * columns + (c + 1) % columns))
            })
            .filter(|&(a, b)| {
                // A rest frame is stored transposed: its third row is the side it faced.
                let rest = |i: usize| self.rest_frames[i].row(2);
                self.normals[a].dot(self.normals[b]) < -0.35 && rest(a).dot(rest(b)) > 0.35
            })
            .count()
    }

    /// The same for the targets the animation gives them, for comparison.
    pub fn target_shake(&self) -> f32 {
        self.target_shake
    }

    fn measure_shake(&mut self, def: &PieceDef) {
        let (mut sum, mut driven, mut count) = (0.0, 0.0, 0);
        for (i, p) in def.particles.iter().enumerate() {
            let [a, b, c] = [self.history[0][i], self.history[1][i], self.history[2][i]];
            let [ta, tb, tc] = [
                self.target_history[0][i],
                self.target_history[1][i],
                self.target_history[2][i],
            ];
            if !p.pinned {
                sum += (self.shown[i] - 3.0 * a + 3.0 * b - c).length();
                driven += (self.targets[i] - 3.0 * ta + 3.0 * tb - tc).length();
                count += 1;
            }
            self.history[2][i] = b;
            self.history[1][i] = a;
            self.history[0][i] = self.shown[i];
            self.target_history[2][i] = tb;
            self.target_history[1][i] = ta;
            self.target_history[0][i] = self.targets[i];
        }
        let count = count.max(1) as f32;
        self.shake = sum / count;
        self.target_shake = driven / count;
    }

    /// Pairs for self-collision, from the chunk spheres [`Self::gather_pairs`] made: only runs
    /// of particles near each other are compared.
    fn gather_self_pairs(&mut self, def: &PieceDef, enabled: bool) {
        self.self_pairs.clear();
        let Frames::Grid { columns, wrap, .. } = def.frames else {
            return;
        };
        if !def.self_collide || !enabled {
            return;
        }
        let n = self.x.len();
        if self.grid_at.len() != n {
            self.grid_at = (0..n)
                .map(|i| ((i / columns) as u16, (i % columns) as u16))
                .collect();
        }
        let at = &self.grid_at;
        let apart = |i: usize, j: usize| {
            let ((ri, ci), (rj, cj)) = (at[i], at[j]);
            let dc = ci.abs_diff(cj) as usize;
            let dc = if wrap { dc.min(columns - dc) } else { dc };
            ri.abs_diff(rj) > 2 || dc > 2
        };
        for a in 0..self.chunks.len() {
            for b in a..self.chunks.len() {
                let ((ca, ra), (cb, rb)) = (self.chunks[a], self.chunks[b]);
                let limit = ra.max(0.0) + rb.max(0.0) + SELF_GAP;
                if ca.distance_squared(cb) > limit * limit {
                    continue;
                }
                for i in a * 8..((a + 1) * 8).min(n) {
                    let from = if a == b { i + 1 } else { b * 8 };
                    for j in from..((b + 1) * 8).min(n) {
                        let near = self.reach[i].max(0.0) + self.reach[j].max(0.0) + SELF_GAP;
                        if self.x[i].distance_squared(self.x[j]) < near * near && apart(i, j) {
                            // A draped fold rests closer than the gap: keep it as draped.
                            let rest = def.shape(i).distance(def.shape(j));
                            let gap = SELF_GAP.min(0.9 * rest);
                            self.self_pairs.push((i as u16, j as u16, gap));
                        }
                    }
                }
            }
        }
    }

    /// Each particle's wind this frame: the wind, stronger and weaker in waves that run across
    /// the piece, and turning a little aside.
    fn gust(&mut self, def: &PieceDef, params: &StepParams) {
        let speed = params.wind.length();
        let along = params.wind.normalize_or_zero();
        let aside = along.cross(Vec3A::Y);
        let t = params.time;
        for (i, p) in def.particles.iter().enumerate() {
            let q = p.rest;
            let surge =
                wave(t * 2.3 + q.y * 5.0 + q.x * 3.0) * wave(t * 5.1 + q.x * 8.0 - q.y * 4.0);
            let swirl = wave(t * 3.7 + q.y * 7.0 - q.z * 5.0);
            self.gusts[i] = params.wind * (1.0 + params.flutter * surge)
                + (aside * swirl + Vec3A::Y * surge * 0.5) * (params.flutter * 0.5 * speed);
        }
    }

    /// Pairs of particles and colliders that could meet this frame: a particle reaches at most
    /// its speed and its target's movement farther, a collider its endpoints' movement.
    fn gather_pairs(&mut self, capsules: &[(Capsule, Capsule)], def: &PieceDef, dt: f32) {
        self.pairs.clear();
        self.bounds.clear();
        self.bounds.extend(capsules.iter().map(|(before, after)| {
            let travel = before.a.distance(after.a).max(before.b.distance(after.b));
            let centre = (after.a + after.b) * 0.5;
            let pressing = if after.press == Vec3A::ZERO {
                0.0
            } else {
                PRESS_DEPTH
            };
            (
                centre,
                after.a.distance(after.b) * 0.5 + after.radius + travel + pressing,
            )
        }));
        // Collider by collider, so pairs next to each other move different particles and the
        // substeps' pushes do not wait on each other.
        self.reach.clear();
        // Pinned particles reach nowhere: colliders do not move them.
        self.reach
            .extend(def.particles.iter().enumerate().map(|(i, p)| {
                if p.pinned {
                    f32::NEG_INFINITY
                } else {
                    self.v[i].length() * dt
                        + self.targets[i].distance(self.targets_prev[i])
                        + p.radius
                        + 0.03
                }
            }));
        // Spheres round runs of eight particles first: most colliders are near few of them.
        self.chunks.clear();
        for (xs, reaches) in self.x.chunks(8).zip(self.reach.chunks(8)) {
            let centre = xs.iter().copied().sum::<Vec3A>() / xs.len() as f32;
            let radius = xs
                .iter()
                .zip(reaches)
                .filter(|(_, r)| r.is_finite())
                .map(|(x, r)| x.distance(centre) + r)
                .fold(f32::NEG_INFINITY, f32::max);
            self.chunks.push((centre, radius));
        }
        for (k, &(centre, bound)) in self.bounds.iter().enumerate() {
            let (before, after) = capsules[k];
            let travel = before.a.distance(after.a).max(before.b.distance(after.b));
            for (chunk, &(middle, extent)) in self.chunks.iter().enumerate() {
                let limit = bound + extent;
                if extent < 0.0 || middle.distance_squared(centre) > limit * limit {
                    continue;
                }
                let first = chunk * 8;
                for i in first..(first + 8).min(self.x.len()) {
                    let (x, reach) = (self.x[i], self.reach[i]);
                    let limit = bound + reach;
                    if limit < 0.0 || x.distance_squared(centre) > limit * limit {
                        continue;
                    }
                    let gap = if after.press != Vec3A::ZERO {
                        // A presser holds back particles however far out they have swung.
                        f32::NEG_INFINITY
                    } else if let Some((origin, up, band)) = self.axis
                        && after.bulk
                    {
                        // Clear of the hull by this much, as the substeps will test it.
                        radial_push(x, &after, reach, origin, up, band).map_or(f32::MAX, |_| 0.0)
                    } else {
                        x.distance(after.closest(x)) - after.radius
                    };
                    if gap < reach + travel {
                        let radius = self.contact_radius(def, i, k, def.particles[i].radius);
                        self.pairs.push((i as u16, k as u16, radius));
                    }
                }
            }
        }
        // Cell centres: a few hundred, each against every collider's sphere.
        let n = self.x.len();
        for (c, corners) in self.cells.iter().enumerate() {
            let reach = corners
                .iter()
                .map(|&i| self.reach[i as usize])
                .fold(f32::NEG_INFINITY, f32::max);
            let x = self.cell_centre(c);
            let own = corners
                .iter()
                .map(|&i| def.particles[i as usize].radius)
                .sum::<f32>()
                * 0.25;
            for (k, &(centre, bound)) in self.bounds.iter().enumerate() {
                let limit = bound + reach;
                if x.distance_squared(centre) > limit * limit {
                    continue;
                }
                let (before, after) = capsules[k];
                let travel = before.a.distance(after.a).max(before.b.distance(after.b));
                let gap = if after.press != Vec3A::ZERO {
                    f32::NEG_INFINITY
                } else if let Some((origin, up, band)) = self.axis
                    && after.bulk
                {
                    radial_push(x, &after, reach, origin, up, band).map_or(f32::MAX, |_| 0.0)
                } else {
                    x.distance(after.closest(x)) - after.radius
                };
                if gap < reach + travel {
                    let radius = self.contact_radius(def, n + c, k, own);
                    self.pairs.push(((n + c) as u16, k as u16, radius));
                }
            }
        }
    }

    /// Collision point `i`'s radius against collider `k`: a particle's, or past the particles
    /// a cell's centre ([`grid_cells`]), `radius` unless sunk ([`PieceDef::sinks`]).
    fn contact_radius(&self, def: &PieceDef, i: usize, k: usize, radius: f32) -> f32 {
        let (from, to) = (self.sinks_from[i] as usize, self.sinks_from[i + 1] as usize);
        def.sinks[from..to]
            .iter()
            .find(|s| s.collider as usize == k)
            .map_or(radius, |s| s.radius)
    }

    /// Pressers first, then other things worn, the body last: where cloth is caught between
    /// them, what comes last wins, and the body does not give way. (A cape caught between a
    /// hand swinging back and a bow's lower end at the hip was flung about, half a metre's
    /// jolt in a frame; a presser last pushed it into the arms standing still.)
    ///
    /// Otherwise in the order gathered: a pass for each, not a sort (sorting a cape's
    /// thousand-odd pairs took about 20 µs a character).
    fn worn_first(&mut self, capsules: &[(Capsule, Capsule)]) {
        let rank = |&(_, k, _): &(u16, u16, f32)| {
            let c = capsules[k as usize].1;
            usize::from(c.press == Vec3A::ZERO) * 2 + usize::from(!c.worn)
        };
        self.ordered.clear();
        for want in 0..4 {
            self.ordered
                .extend(self.pairs.iter().filter(|pair| rank(pair) == want));
        }
        std::mem::swap(&mut self.pairs, &mut self.ordered);
    }

    /// Records collision point `i` (a particle, or a cell centre's corners) as held by a presser.
    fn mark_pressed(&mut self, i: usize) {
        let n = self.x.len();
        if i < n {
            self.pressed[i] = true;
        } else {
            for c in self.cells[i - n] {
                self.pressed[c as usize] = true;
            }
        }
    }

    /// Where a cell's centre is: its corners' mean.
    fn cell_centre(&self, cell: usize) -> Vec3A {
        self.cells[cell]
            .iter()
            .map(|&i| self.x[i as usize])
            .sum::<Vec3A>()
            * 0.25
    }

    /// Moves collision point `i` by `delta`: a particle itself, or a cell centre's free corners
    /// each by `delta`. That moves the centre all the way where no corner is pinned, part of the
    /// way where some are (all of it there would land, doubled or more, on the free ones).
    /// `velocity` is the pusher's, there.
    fn displace(
        &mut self,
        particles: &[ParticleDef],
        i: usize,
        delta: Vec3A,
        normal: Vec3A,
        velocity: Vec3A,
    ) {
        let n = self.x.len();
        if i < n {
            self.x[i] += delta;
            self.contacts[i] = normal;
            self.pushed_by[i] = velocity;
            return;
        }
        let corners = self.cells[i - n];
        for &c in &corners {
            let c = c as usize;
            if !particles[c].pinned {
                self.x[c] += delta;
                self.contacts[c] = normal;
                self.pushed_by[c] = velocity;
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // One substep of `step`, with its frame's inputs.
    fn substep(
        &mut self,
        def: &PieceDef,
        deforms: &[Affine3A],
        capsules: &[(Capsule, Capsule)],
        layers: &[&Layer],
        params: &StepParams,
        h: f32,
        t: f32,
    ) {
        let particles = &def.particles;
        let frame = 1.0 / params.dt.max(1e-4);
        let rate = |time: f32| 1.0 - (-h / time).exp();
        let (leash, press, hull_soft) = (rate(LEASH_TIME), rate(PRESS_TIME), rate(HULL_SOFT_TIME));
        for (i, p) in particles.iter().enumerate() {
            let target = self.targets_prev[i].lerp(self.targets[i], t);
            self.start[i] = self.x[i];
            if p.pinned {
                self.x[i] = target;
                continue;
            }
            // Air: across the surface it pushes as the square of the speed (gentle on slow
            // swinging, quick to stop a flick); along it, and for strands, in proportion.
            let carried = (self.targets[i] - self.targets_prev[i]) * frame;
            let air = carried + (self.gusts[i] - carried) * p.exposure;
            let relative = air - self.v[i];
            let n = self.normals[i];
            let across = n.dot(relative);
            let accel = params.gravity
                + (relative - n * across) * def.drag
                + n * (across * across.abs() * def.lift * params.lift);
            self.v[i] += accel * h;
            self.x[i] += self.v[i] * h;
        }

        let inverse_h2 = 1.0 / (h * h);
        let x = &mut self.x;
        for link in &self.links {
            let (a, b) = (link.a as usize, link.b as usize);
            let d = x[b] - x[a];
            let length = d.length();
            if length <= f32::EPSILON {
                continue;
            }
            let lambda = (length - link.rest) / (link.wa + link.wb + link.compliance * inverse_h2);
            let correction = d * (lambda / length);
            x[a] += correction * link.wa;
            x[b] -= correction * link.wb;
        }
        for tether in &def.tethers {
            let (i, root) = (tether.particle as usize, tether.root as usize);
            let d = self.x[i] - self.x[root];
            let length = d.length();
            if length > tether.length {
                self.x[i] = self.x[root] + d * (tether.length / length);
            }
        }
        for cone in &def.cones {
            let (pivot, i) = (cone.pivot as usize, cone.particle as usize);
            let axis = deforms[cone.bone as usize]
                .matrix3
                .mul_vec3a(cone.axis)
                .normalize_or_zero();
            let d = self.x[i] - self.x[pivot];
            let length = d.length();
            if length <= f32::EPSILON || axis == Vec3A::ZERO {
                continue;
            }
            let direction = d / length;
            let cos = direction.dot(axis);
            if cos < cone.cos_max {
                let side = (direction - axis * cos).normalize_or(axis.any_orthonormal_vector());
                let sin_max = (1.0 - cone.cos_max * cone.cos_max).max(0.0).sqrt();
                self.x[i] = self.x[pivot] + (axis * cone.cos_max + side * sin_max) * length;
            }
        }
        for (i, p) in particles.iter().enumerate() {
            if p.pinned {
                continue;
            }
            let target = self.targets_prev[i].lerp(self.targets[i], t);
            if p.goal > 0.0 {
                self.x[i] = self.x[i].lerp(target, (p.goal * h).min(1.0));
            }
            let d = self.x[i] - target;
            let distance = d.length();
            if distance > p.max_distance {
                // Back over [`LEASH_TIME`].
                let limit = target + d * (p.max_distance / distance);
                self.x[i] = self.x[i].lerp(limit, leash);
            }
            if let (Some((origin, up, _)), Some(margin)) =
                (self.axis, def.radial.and_then(|r| r.inward))
                && !self.pressed[i]
            {
                let flat = |at: Vec3A| (at - origin) - up * (at - origin).dot(up);
                let (now, aim) = (flat(self.x[i]), flat(target));
                let floor = aim.length() - margin;
                let r = now.length();
                if r < floor && r > 1e-5 {
                    self.x[i] += now * (floor / r - 1.0);
                }
            }
        }
        // Colliders last: being outside the body matters more than staying near the pose.
        self.free.copy_from_slice(&self.x);
        self.contacts.fill(Vec3A::ZERO);
        self.pressed.fill(false);
        let n = self.x.len();
        // Once a substep for each collider, not for each of its pairs.
        self.now.clear();
        self.now.extend(
            capsules
                .iter()
                .map(|&(before, after)| before.lerp(after, t)),
        );
        for p in 0..self.pairs.len() {
            let (i, k, radius) = self.pairs[p];
            let i = i as usize;
            let (x, own, target) = if i < n {
                (self.x[i], particles[i].radius, self.targets[i])
            } else {
                (
                    self.cell_centre(i - n),
                    self.cell_radius[i - n],
                    self.cell_targets[i - n],
                )
            };
            let (before, after) = capsules[k as usize];
            let capsule = self.now[k as usize];
            let closest = capsule.closest(x);
            let d = x - closest;
            let reach = capsule.radius + radius;
            if capsule.press != Vec3A::ZERO {
                let under = d.dot(capsule.press);
                let across = d - capsule.press * under;
                let held = capsule.radius + PRESS_GAP;
                // Wider than the gaps between particles, so cloth cannot straddle the item.
                let band = capsule.width + 2.4 * own;
                if under < held
                    && under > held - PRESS_DEPTH
                    && across.length_squared() < band * band
                {
                    // No farther out than it was: the wind does not blow it out through the
                    // item. Pushed out by something else (a hand, which comes later and wins),
                    // back under over [`PRESS_TIME`].
                    let x0 = if i < n {
                        self.start[i]
                    } else {
                        self.cells[i - n]
                            .iter()
                            .map(|&c| self.start[c as usize])
                            .sum::<Vec3A>()
                            * 0.25
                    };
                    let was = (x0 - capsule.closest(x0)).dot(capsule.press);
                    let limit = held.min(was + (held - was) * press);
                    if under < limit {
                        let velocity = surface_velocity(before, after, x, frame);
                        let push = capsule.press * (limit - under);
                        self.displace(particles, i, push, capsule.press, velocity);
                    }
                    self.mark_pressed(i);
                }
                continue;
            }
            if let Some((origin, up, reach)) = self.axis
                && let Some(out) = radial_push(x, &capsule, radius, origin, up, reach)
            {
                let velocity = surface_velocity(before, after, x, frame);
                let normal = out.normalize_or_zero();
                let out = soften(out, radius, hull_soft);
                self.displace(particles, i, out * params.contact, normal, velocity);
                continue;
            }
            let distance2 = d.length_squared();
            if distance2 >= reach * reach {
                continue;
            }
            let distance = distance2.sqrt();
            // On the axis itself, leave the way the target lies.
            let mut normal = if distance > 1e-6 {
                d / distance
            } else {
                (target - closest).normalize_or(Vec3A::Y)
            };
            // Out only away from the body: cloth between the collider and the body goes round to
            // its far side. Not near the end it is worn by (a scabbard's hilt sits on the hip, the
            // cloth there under it).
            if capsule.outward
                && self.settling > 0.0
                && capsule.along(x) > WORN_END
                && let Some((origin, up, _)) = self.axis
            {
                let flat = (x - origin) - up * (x - origin).dot(up);
                let away = flat.normalize_or_zero();
                let inward = normal.dot(away);
                if inward < 0.0 {
                    normal = (normal - away * (2.0 * inward)).normalize_or(away);
                }
            }
            // `params.contact` of the way: all of it but in tuning. (Part of the way left cloth
            // inside colliders for substeps on end, through the cloth drawn there.)
            let push = closest + normal * reach - x;
            let velocity = surface_velocity(before, after, x, frame);
            self.displace(particles, i, push * params.contact, normal, velocity);
        }

        for &(i, j, gap) in &self.self_pairs {
            let (i, j) = (i as usize, j as usize);
            let (wi, wj) = (weight(&particles[i]), weight(&particles[j]));
            if wi + wj == 0.0 {
                continue;
            }
            let d = self.x[j] - self.x[i];
            let distance = d.length();
            if distance >= gap || distance <= 1e-6 {
                continue;
            }
            let correction = d * ((gap - distance) / (distance * (wi + wj)));
            self.x[i] -= correction * wi;
            self.x[j] += correction * wj;
        }
        for layer in layers {
            for (i, p) in particles.iter().enumerate() {
                if p.pinned {
                    continue;
                }
                let reach = def
                    .layer_reach
                    .get(i)
                    .copied()
                    .unwrap_or(p.radius + LAYER_GAP);
                if let Some(outside) = layer.push_out(self.x[i], reach) {
                    self.contacts[i] = (outside - self.x[i]).normalize_or_zero();
                    // The garment under moves about as the body carries this one.
                    self.pushed_by[i] = (self.targets[i] - self.targets_prev[i]) * frame;
                    let push = outside - self.x[i];
                    self.x[i] += push * params.contact;
                }
            }
            for c in 0..self.cells.len() {
                let corners = self.cells[c];
                if corners.iter().all(|&i| particles[i as usize].pinned) {
                    continue;
                }
                let reach = corners
                    .iter()
                    .map(|&i| {
                        let i = i as usize;
                        def.layer_reach
                            .get(i)
                            .copied()
                            .unwrap_or(particles[i].radius + LAYER_GAP)
                    })
                    .sum::<f32>()
                    * 0.25;
                let x = self.cell_centre(c);
                if let Some(outside) = layer.push_out(x, reach) {
                    let push = outside - x;
                    let carried = corners
                        .iter()
                        .map(|&i| self.targets[i as usize] - self.targets_prev[i as usize])
                        .sum::<Vec3A>()
                        * (0.25 * frame);
                    self.displace(
                        particles,
                        n + c,
                        push * params.contact,
                        push.normalize_or_zero(),
                        carried,
                    );
                }
            }
        }

        // Damp motion relative to where the animation carries each particle, not in the world:
        // the body's own movement is not slowed, the cloth's swinging and shivering about it is.
        let damping = 1.0 / (1.0 + def.damping * params.damping * h);
        for (i, p) in particles.iter().enumerate() {
            if p.pinned {
                self.v[i] = (self.x[i] - self.start[i]) / h;
                continue;
            }
            let mut v = (self.x[i] - self.start[i]) / h;
            let n = self.contacts[i];
            if n != Vec3A::ZERO {
                // Pushed out, no faster than what pushed it (or than it was going anyway): the
                // whole push taken as speed, a centimetre's in a substep sent it off at 2.4 m/s,
                // to fall back and be pushed again.
                let pushed = self.x[i] - self.free[i];
                let out = pushed.normalize_or_zero();
                let free = (self.free[i] - self.start[i]) / h;
                let allowed = free.dot(out).max(self.pushed_by[i].dot(out));
                let excess = v.dot(out) - allowed;
                if excess > 0.0 {
                    v -= out * excess;
                }
                // Sliding slowed in the world, not against the collider: slowed against the
                // collider, cloth was dragged about by the legs (skirt jogging 7.4 → 8.6 mm).
                let sliding = v - n * v.dot(n);
                v -= sliding * def.friction;
            }
            let carried = (self.targets[i] - self.targets_prev[i]) * frame;
            v = carried + (v - carried) * damping;
            let speed2 = v.length_squared();
            if speed2 > MAX_SPEED * MAX_SPEED {
                v *= MAX_SPEED / speed2.sqrt();
            }
            self.v[i] = v;
        }
    }

    /// The render joints' world poses: each joint's turn from its rest
    /// ([`PieceDef::joint_rests`]) and where it is.
    /// Sheets also keep each frame's normals for the next frame's wind.
    pub fn joints(&mut self, def: &PieceDef, deforms: &[Affine3A], out: &mut Vec<(Quat, Vec3A)>) {
        out.clear();
        match def.frames {
            Frames::Grid {
                columns,
                rows,
                wrap,
            } => {
                for r in 0..rows {
                    for c in 0..columns {
                        let i = r * columns + c;
                        let now = grid_frame(&self.shown, columns, rows, wrap, r, c);
                        self.normals[i] = now.z_axis;
                        let rotation = Quat::from_mat3a(&(now * self.rest_frames[i]));
                        out.push((rotation.normalize(), self.shown[i]));
                    }
                }
            }
            Frames::Chain { bone } => {
                let turn = bone_rotation(deforms, bone);
                let n = self.shown.len();
                for i in 0..n {
                    let (a, b) = if i + 1 < n { (i, i + 1) } else { (i - 1, i) };
                    let rest = def.particles[b].rest - def.particles[a].rest;
                    let now = self.shown[b] - self.shown[a];
                    out.push((carried_turn(turn, rest, now), self.shown[i]));
                }
            }
            Frames::Rigid { bone } => {
                let turn = bone_rotation(deforms, bone);
                let last = self.shown.len() - 1;
                let rest = def.particles[last].rest - def.particles[0].rest;
                let now = self.shown[last] - self.shown[0];
                out.push((carried_turn(turn, rest, now), self.shown[0]));
            }
        }
    }
}

/// Orders links in batches that share no particle, so neighbouring solves do not wait on each
/// other's writes (one link after another along a row would). Greedy colouring.
fn independent_batches(
    links: impl Iterator<Item = SolverLink>,
    particles: usize,
) -> Vec<SolverLink> {
    let mut batches: Vec<(Vec<bool>, Vec<SolverLink>)> = Vec::new();
    for link in links {
        let (a, b) = (link.a as usize, link.b as usize);
        let batch = match batches.iter().position(|(used, _)| !used[a] && !used[b]) {
            Some(i) => i,
            None => {
                batches.push((vec![false; particles], Vec::new()));
                batches.len() - 1
            }
        };
        let (used, batch) = &mut batches[batch];
        used[a] = true;
        used[b] = true;
        batch.push(link);
    }
    batches.into_iter().flat_map(|(_, links)| links).collect()
}

/// How fast `before`'s surface near `x` moves to `after`'s over the frame (`frame` is 1/dt).
fn surface_velocity(before: Capsule, after: Capsule, x: Vec3A, frame: f32) -> Vec3A {
    let s = after.along(x);
    (after.a.lerp(after.b, s) - before.a.lerp(before.b, s)) * frame
}

/// `push`, the part of its depth within `margin` and [`HULL_SOFT`] of the start taken at `rate`.
fn soften(push: Vec3A, margin: f32, rate: f32) -> Vec3A {
    let depth = push.length();
    if depth <= 1e-9 {
        return push;
    }
    let zone = HULL_SOFT.min(margin.max(0.0));
    let hard = (depth - zone).max(0.0);
    push * ((hard + depth.min(zone) * rate) / depth)
}

/// For a wrap-around piece: how far to move `x` outward from the axis to lie outside a bulk
/// `capsule`'s support along its direction, over the heights `reach` either side of it (`None`
/// if it already does, or the capsule is not bulk). Taken over every bulk capsule, that keeps
/// the piece outside their convex hull: it spans the gap between the legs as cloth does, and a
/// particle never hops from one side of a leg to the other.
///
/// A capsule is spheres along its axis, so its slice at a height reaches out as far as the
/// farthest of theirs: at any lean, a raised knee or a shin swung back included. (Leaning past
/// 60° a capsule used to be left out: a jogging shin dropped out of the hull and back in each
/// stride, and the skirt's hem was kicked out from inside it.)
pub(crate) fn radial_push(
    x: Vec3A,
    capsule: &Capsule,
    radius: f32,
    origin: Vec3A,
    up: Vec3A,
    reach: f32,
) -> Option<Vec3A> {
    if !capsule.bulk {
        return None;
    }
    let rel = x - origin;
    let height = rel.dot(up);
    let flat = rel - up * height;
    let r = flat.length();
    let out = if r > 1e-4 { flat / r } else { return None };
    // The capsule's axis, from `a`: out along `out`, and up.
    let along = capsule.b - capsule.a;
    let (out_a, up_a) = ((capsule.a - origin).dot(out), (capsule.a - origin).dot(up));
    let (outward, rise) = (along.dot(out), along.dot(up));
    let round = capsule.radius;
    let mut wall = f32::MIN;
    for h in [height - reach, height, height + reach] {
        // How far out the sphere at `t` along the axis reaches at height `h` is
        // out_a + outward·t + √(round² − (above − rise·t)²): concave in `t`, so the farthest
        // lies where its slope is zero, or at the nearest end of the axis within `round` of `h`.
        let above = h - up_a;
        let t = if rise.abs() > 1e-6 {
            let (low, high) = ((above - round) / rise, (above + round) / rise);
            let (low, high) = (low.min(high).max(0.0), low.max(high).min(1.0));
            if low > high {
                continue;
            }
            let level = -outward * round * rise.signum() / outward.hypot(rise).max(1e-9);
            ((above - level) / rise).clamp(low, high)
        } else if outward > 0.0 {
            1.0
        } else {
            0.0
        };
        let off = above - rise * t;
        if off.abs() >= round {
            continue;
        }
        wall = wall.max(out_a + outward * t + (round * round - off * off).sqrt() + radius);
    }
    (r < wall).then(|| out * (wall - r))
}

/// A smooth wave through -1..1 with a period of 2π, close to `sin` and much cheaper.
fn wave(x: f32) -> f32 {
    let f = (x * std::f32::consts::FRAC_1_PI * 0.5).fract().abs();
    let s = 1.0 - 4.0 * (f - 0.5).abs();
    s * (1.5 - 0.5 * s * s)
}

fn weight(p: &ParticleDef) -> f32 {
    if p.pinned { 0.0 } else { 1.0 }
}

/// A bone's rotation from its bind pose, without any stretch.
fn bone_rotation(deforms: &[Affine3A], bone: u16) -> Quat {
    let (_, rotation, _) = deforms[bone as usize].to_scale_rotation_translation();
    rotation
}

/// `turn`, then the shortest swing taking the turned rest direction onto the current one.
fn carried_turn(turn: Quat, rest: Vec3A, now: Vec3A) -> Quat {
    let (Some(from), Some(to)) = ((turn * rest).try_normalize(), now.try_normalize()) else {
        return turn;
    };
    (Quat::from_rotation_arc(from.into(), to.into()) * turn).normalize()
}

/// How many cells [`grid_cells`] makes of a piece.
fn grid_cell_count(frames: &Frames) -> usize {
    match *frames {
        Frames::Grid {
            columns,
            rows,
            wrap,
        } => (if wrap { columns } else { columns - 1 }) * rows.saturating_sub(1),
        _ => 0,
    }
}

/// A sheet's cells as their corner particles: (row, column), the next column, the next row,
/// both; a wrapped sheet's last column closes on its first.
pub(crate) fn grid_cells(columns: usize, rows: usize, wrap: bool) -> Vec<[usize; 4]> {
    let across = if wrap { columns } else { columns - 1 };
    let mut cells = Vec::with_capacity(across * rows.saturating_sub(1));
    for r in 0..rows.saturating_sub(1) {
        for c in 0..across {
            let at = |r: usize, c: usize| r * columns + c % columns;
            cells.push([at(r, c), at(r, c + 1), at(r + 1, c), at(r + 1, c + 1)]);
        }
    }
    cells
}

fn grid_index(columns: usize, wrap: bool, r: usize, c: isize) -> usize {
    let c = if wrap {
        c.rem_euclid(columns as isize) as usize
    } else {
        c.clamp(0, columns as isize - 1) as usize
    };
    r * columns + c
}

/// Orthonormal frame of a sheet at one particle: across the row, down the column, normal.
fn grid_frame(x: &[Vec3A], columns: usize, rows: usize, wrap: bool, r: usize, c: usize) -> Mat3A {
    let ci = c as isize;
    let across = x[grid_index(columns, wrap, r, ci + 1)] - x[grid_index(columns, wrap, r, ci - 1)];
    let (up, down) = (r.saturating_sub(1), (r + 1).min(rows - 1));
    let along = x[grid_index(columns, wrap, down, ci)] - x[grid_index(columns, wrap, up, ci)];
    let e1 = across.normalize_or(Vec3A::X);
    let e3 = e1.cross(along).normalize_or(e1.any_orthonormal_vector());
    let e2 = e3.cross(e1);
    Mat3A::from_cols(e1, e2, e3)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMS: StepParams = StepParams {
        dt: 1.0 / 60.0,
        substep_hz: 240.0,
        gravity: Vec3A::new(0.0, -9.81, 0.0),
        wind: Vec3A::ZERO,
        flutter: 0.0,
        time: 0.0,
        inertia: None,
        damping: 1.0,
        lift: 1.0,
        contact: 1.0,
        self_collide: true,
    };

    /// A vertical strand of `n` particles 0.1 m apart hanging from a pinned top one.
    fn strand(n: usize) -> PieceDef {
        let particles = (0..n)
            .map(|i| ParticleDef {
                rest: Vec3A::new(0.0, -0.1 * i as f32, 0.0),
                carrier: Carrier::on(0),
                pinned: i == 0,
                max_distance: 10.0,
                goal: 0.0,
                radius: 0.02,
                exposure: 1.0,
            })
            .collect();
        let links = (1..n)
            .map(|i| Link {
                a: (i - 1) as u16,
                b: i as u16,
                rest: 0.1,
                compliance: 0.0,
            })
            .collect();
        let tethers = (1..n)
            .map(|i| Tether {
                particle: i as u16,
                root: 0,
                length: 0.1 * i as f32,
            })
            .collect();
        PieceDef {
            particles,
            links,
            tethers,
            cones: Vec::new(),
            frames: Frames::Chain { bone: 0 },
            anchor: 0,
            inertia: 1.0,
            damping: 2.0,
            drag: 0.0,
            lift: 0.0,
            friction: 0.0,
            radial: None,
            self_collide: false,
            shape: Vec::new(),
            sinks: Vec::new(),
            layer_reach: Vec::new(),
        }
    }

    fn run(
        def: &PieceDef,
        sim: &mut PieceSim,
        deforms: &[Affine3A],
        capsules: &[Capsule],
        frames: usize,
    ) {
        let pairs: Vec<_> = capsules.iter().map(|c| (*c, *c)).collect();
        for _ in 0..frames {
            sim.step(def, deforms, &pairs, &[], &PARAMS);
        }
    }

    #[test]
    fn a_strand_pushed_aside_settles_hanging_under_its_pin_without_stretching() {
        let def = strand(6);
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        sim.x[5] = Vec3A::new(0.4, -0.3, 0.0);
        run(&def, &mut sim, &deforms, &[], 600);
        let tip = sim.positions()[5];
        assert!(
            tip.x.abs() < 0.01 && tip.z.abs() < 0.01,
            "tip {tip:?} still swinging"
        );
        assert!(
            (tip.y + 0.5).abs() < 0.01,
            "strand stretched or bunched: tip at {tip:?}"
        );
    }

    #[test]
    fn particles_end_outside_a_capsule_in_their_way() {
        let def = strand(6);
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        // A horizontal bar right under the third particle.
        let bar = Capsule::new(
            Vec3A::new(-0.5, -0.25, 0.02),
            Vec3A::new(0.5, -0.25, 0.02),
            0.05,
        );
        run(&def, &mut sim, &deforms, &[bar], 300);
        for (i, x) in sim.positions().iter().enumerate().skip(1) {
            let gap = x.distance(bar.closest(*x)) - bar.radius - 0.02;
            assert!(gap > -2e-3, "particle {i} at {x:?} is {gap} inside the bar");
        }
    }

    #[test]
    fn a_rod_between_rows_of_particles_is_caught_by_the_cells() {
        // A sheet hanging in the plane z = 0, particles 0.2 m apart, the top row pinned.
        let (columns, rows) = (3, 4);
        let particles: Vec<ParticleDef> = (0..columns * rows)
            .map(|i| ParticleDef {
                rest: Vec3A::new(0.2 * (i % columns) as f32, -0.2 * (i / columns) as f32, 0.0),
                carrier: Carrier::on(0),
                pinned: i < columns,
                max_distance: 10.0,
                goal: 0.0,
                radius: 0.02,
                exposure: 1.0,
            })
            .collect();
        let mut links = Vec::new();
        for i in 0..columns * rows {
            let mut link = |j: usize| {
                links.push(Link {
                    a: i as u16,
                    b: j as u16,
                    rest: particles[i].rest.distance(particles[j].rest),
                    compliance: 0.0,
                })
            };
            if i % columns + 1 < columns {
                link(i + 1);
            }
            if i + columns < columns * rows {
                link(i + columns);
            }
        }
        let def = PieceDef {
            particles,
            links,
            frames: Frames::Grid {
                columns,
                rows,
                wrap: false,
            },
            ..strand(2)
        };
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        // A thin rod across the sheet halfway between its second and third rows, just in front:
        // 0.1 m from every particle, out of their reach, but through the cloth between them.
        let rod = Capsule::new(
            Vec3A::new(-0.5, -0.3, 0.01),
            Vec3A::new(0.9, -0.3, 0.01),
            0.01,
        );
        run(&def, &mut sim, &deforms, &[rod], 300);
        for cell in columns - 1..2 * (columns - 1) {
            let centre = sim.cell_centre(cell);
            let gap = centre.distance(rod.closest(centre)) - rod.radius - 0.02;
            assert!(
                gap > -3e-3,
                "cell {cell} centre {centre:?} is {gap} inside the rod"
            );
        }
    }

    #[test]
    fn moving_the_anchor_carries_a_rigid_piece_and_lags_a_free_one() {
        let mut rigid = strand(4);
        rigid.inertia = 0.0;
        let free = strand(4);
        let mut sims = [PieceSim::new(&rigid), PieceSim::new(&free)];
        for (def, sim) in [&rigid, &free].into_iter().zip(&mut sims) {
            sim.reset(def, &[Affine3A::IDENTITY]);
            // One frame, half a metre sideways.
            sim.step(
                def,
                &[Affine3A::from_translation(bevy::math::Vec3::X * 0.5)],
                &[],
                &[],
                &PARAMS,
            );
        }
        assert!((sims[0].positions()[3].x - 0.5).abs() < 1e-3);
        assert!(sims[1].positions()[3].x < 0.4, "free tip followed at once");
    }

    #[test]
    fn diamond_angles_order_directions_as_angles_do_and_invert() {
        let mut previous = -1.0;
        for k in 0..64 {
            let a = k as f32 / 64.0 * std::f32::consts::TAU;
            let d = diamond(a.sin(), a.cos());
            assert!(d > previous, "not monotonic at {a}");
            previous = d;
            let (x, z) = undiamond(d);
            assert!((x.atan2(z).rem_euclid(std::f32::consts::TAU) - a).abs() < 1e-4);
        }
    }

    #[test]
    fn a_layer_keeps_particles_outside_it_between_its_columns() {
        // A skirt-like ring of 8 columns, 0.2 m round, from 1.0 down to 0.4 m.
        let (columns, rows) = (8, 3);
        let positions: Vec<Vec3A> = (0..rows * columns)
            .map(|i| {
                let a = (i % columns) as f32 / columns as f32 * std::f32::consts::TAU;
                Vec3A::new(
                    a.sin() * 0.2,
                    1.0 - 0.3 * (i / columns) as f32,
                    a.cos() * 0.2,
                )
            })
            .collect();
        let angles: Vec<f32> = positions[..columns]
            .iter()
            .map(|p| p.x.atan2(p.z))
            .collect();
        let mut layer = Layer::new(rows, &angles);
        layer.update(&positions, Vec3A::ZERO, Mat3A::IDENTITY);
        // Halfway between two columns, inside the polygon: pushed out to the wall.
        let a = 0.5 / columns as f32 * std::f32::consts::TAU;
        let inside = Vec3A::new(a.sin() * 0.1, 0.7, a.cos() * 0.1);
        let out = layer.push_out(inside, 0.02).expect("inside the layer");
        let r = out.x.hypot(out.z);
        let wall = 0.2 * (std::f32::consts::PI / columns as f32).cos();
        assert!(r > wall && r < 0.2 + 0.021, "pushed to {r}");
        assert!((out.y - 0.7).abs() < 1e-5);
        // Outside, above or below it: left alone.
        assert!(layer.push_out(Vec3A::new(0.0, 0.7, 0.3), 0.02).is_none());
        assert!(layer.push_out(Vec3A::new(0.0, 1.2, 0.05), 0.02).is_none());
        assert!(layer.push_out(Vec3A::new(0.0, 0.3, 0.05), 0.02).is_none());
    }

    #[test]
    fn radial_pushes_keep_a_skirt_outside_the_legs_hull() {
        let origin = Vec3A::new(0.0, 0.5, 0.0);
        let leg = |x: f32| Capsule {
            bulk: true,
            ..Capsule::new(Vec3A::new(x, 1.0, 0.0), Vec3A::new(x, 0.0, 0.0), 0.07)
        };
        let (left, right) = (leg(0.1), leg(-0.1));
        // Inside the left leg, on the axis side: out past its far side, not between the legs.
        let x = Vec3A::new(0.06, 0.5, 0.0);
        let out = radial_push(x, &left, 0.02, origin, Vec3A::Y, 0.0).expect("inside the hull");
        assert!(((x + out).x - (0.1 + 0.09)).abs() < 1e-4, "{:?}", x + out);
        // In front of the gap between the legs: held at the hull's front, the legs' radius.
        let front = Vec3A::new(0.0, 0.5, 0.05);
        let pushed = [left, right]
            .iter()
            .filter_map(|c| radial_push(front, c, 0.02, origin, Vec3A::Y, 0.0))
            .fold(
                Vec3A::ZERO,
                |a, b| if b.length() > a.length() { b } else { a },
            );
        assert!(
            ((front + pushed).z - 0.09).abs() < 1e-4,
            "{:?}",
            front + pushed
        );
        // A lying capsule reaches out as far as its end: a thigh raised level lifts the skirt
        // to the knee.
        let thigh = Capsule {
            bulk: true,
            ..Capsule::new(Vec3A::new(0.1, 0.5, 0.0), Vec3A::new(0.1, 0.5, 0.4), 0.06)
        };
        let ahead = Vec3A::new(0.0, 0.5, 0.2);
        let out = radial_push(ahead, &thigh, 0.02, origin, Vec3A::Y, 0.0).expect("under the knee");
        assert!(((ahead + out).z - 0.48).abs() < 1e-4, "{:?}", ahead + out);
        // Leaning, the same reach as the spheres along it give, whatever the lean.
        for lean in [0.2f32, 0.8, 1.2, 1.5] {
            let (s, c) = lean.sin_cos();
            let leg = Capsule {
                bulk: true,
                ..Capsule::new(origin, origin + Vec3A::new(0.0, -c, s) * 0.5, 0.06)
            };
            let probe = origin + Vec3A::new(0.0, -0.02, 0.01);
            let wall = (0..=1000)
                .map(|k| {
                    let at = leg.a.lerp(leg.b, k as f32 / 1000.0);
                    let off = probe.y - at.y;
                    if off.abs() < 0.06 {
                        at.z + (0.06 * 0.06 - off * off).sqrt()
                    } else {
                        f32::MIN
                    }
                })
                .fold(f32::MIN, f32::max);
            let out = radial_push(probe, &leg, 0.0, origin, Vec3A::Y, 0.0).expect("inside");
            assert!(
                ((probe + out).z - wall).abs() < 1e-3,
                "{lean}: {:?} {wall}",
                probe + out
            );
        }
        // One not of the bulk is left to the ordinary push.
        let item = Capsule::new(Vec3A::new(0.1, 1.0, 0.0), Vec3A::new(0.1, 0.0, 0.0), 0.07);
        assert!(radial_push(x, &item, 0.02, origin, Vec3A::Y, 0.0).is_none());
    }

    #[test]
    fn a_presser_brings_particles_back_under_it() {
        let def = strand(6);
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        // A vertical strap just behind the strand, pressing towards +Z; the strand pushed out
        // past it (as a hand does, from under a bow), and let go, is back in front of it well
        // within half a second.
        let strap = strap();
        for i in 1..6 {
            sim.x[i].z = -0.06;
        }
        run(&def, &mut sim, &deforms, &[strap], 30);
        for (i, x) in sim.positions().iter().enumerate().skip(1) {
            assert!(
                x.z > STRAP_HELD - 2e-3,
                "particle {i} at {x:?} got past the strap"
            );
        }
    }

    const STRAP_HELD: f32 = -0.05 + 0.012 + PRESS_GAP;

    fn strap() -> Capsule {
        let mut strap = Capsule::new(
            Vec3A::new(0.0, 0.0, -0.05),
            Vec3A::new(0.0, -0.6, -0.05),
            0.012,
        );
        strap.press = Vec3A::Z;
        strap.width = 0.03;
        strap
    }

    #[test]
    fn the_wind_does_not_blow_cloth_out_through_a_presser() {
        let mut def = strand(6);
        def.drag = 5.0;
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        // A gale towards -Z, through the strap: the strand stays in front of it. Brought back
        // only gradually, it was blown 3 cm out through it.
        let strap = strap();
        let params = StepParams {
            wind: Vec3A::new(0.0, 0.0, -18.0),
            ..PARAMS
        };
        for _ in 0..60 {
            sim.step(&def, &deforms, &[(strap, strap)], &[], &params);
        }
        for (i, x) in sim.positions().iter().enumerate().skip(1) {
            assert!(
                x.z > STRAP_HELD - 1e-3,
                "particle {i} at {x:?} was blown through the strap"
            );
        }
    }

    #[test]
    fn a_push_out_of_a_collider_adds_no_speed_of_its_own() {
        let def = strand(2);
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        // A still bar across the hanging particle, 2 cm into it: put outside, and left there.
        let bar = Capsule::new(
            Vec3A::new(-0.2, -0.1, 0.07),
            Vec3A::new(0.2, -0.1, 0.07),
            0.07 + 0.02 - def.particles[1].radius,
        );
        run(&def, &mut sim, &deforms, &[bar], 1);
        let outside = (sim.x[1] - bar.closest(sim.x[1])).length();
        assert!(
            outside >= bar.radius + def.particles[1].radius - 1e-4,
            "{:?}",
            sim.x[1]
        );
        // Taken as speed, the push sent it off at 4 m/s.
        assert!(sim.v[1].length() < 0.2, "{:?}", sim.v[1]);
    }

    #[test]
    fn a_cone_keeps_a_pendulum_near_its_axis() {
        let mut def = strand(3);
        def.cones.push(Cone {
            pivot: 0,
            particle: 2,
            bone: 0,
            axis: Vec3A::NEG_Y,
            cos_max: 20f32.to_radians().cos(),
        });
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        sim.v[1] = Vec3A::new(6.0, 0.0, 0.0);
        sim.v[2] = Vec3A::new(12.0, 0.0, 0.0);
        let mut widest = 0f32;
        for _ in 0..120 {
            run(&def, &mut sim, &deforms, &[], 1);
            let d = sim.positions()[2] - sim.positions()[0];
            widest = widest.max(d.normalize().dot(Vec3A::NEG_Y).acos().to_degrees());
        }
        assert!(widest < 20.5, "swung {widest}° from the axis");
    }

    #[test]
    fn sheet_joints_are_identity_at_rest() {
        let columns = 3;
        let rows = 3;
        let particles = (0..rows * columns)
            .map(|i| ParticleDef {
                rest: Vec3A::new(
                    (i % columns) as f32 * 0.1,
                    -((i / columns) as f32) * 0.1,
                    0.0,
                ),
                carrier: Carrier::on(0),
                pinned: i < columns,
                max_distance: 1.0,
                goal: 0.0,
                radius: 0.02,
                exposure: 1.0,
            })
            .collect();
        let def = PieceDef {
            particles,
            links: Vec::new(),
            tethers: Vec::new(),
            cones: Vec::new(),
            frames: Frames::Grid {
                columns,
                rows,
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
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        let mut joints = Vec::new();
        sim.joints(&def, &deforms, &mut joints);
        for ((rotation, position), particle) in joints.iter().zip(&def.particles) {
            assert!(rotation.angle_between(Quat::IDENTITY) < 1e-4);
            assert!(position.distance(particle.rest) < 1e-6);
        }
    }

    /// `cargo test --release -p yarra-engine bench_ -- --ignored --nocapture`: a frame of a
    /// cloak-sized and a skirt-sized sheet among 43 capsules, as the lab's characters have.
    #[test]
    #[ignore]
    fn bench_sheets_among_capsules() {
        fn sheet(columns: usize, rows: usize, wrap: bool, radius: f32) -> PieceDef {
            let mut particles = Vec::new();
            for r in 0..rows {
                for c in 0..columns {
                    let a = c as f32 / columns as f32 * std::f32::consts::TAU;
                    particles.push(ParticleDef {
                        rest: Vec3A::new(a.sin() * radius, 1.4 - 0.13 * r as f32, a.cos() * radius),
                        carrier: Carrier::on(0),
                        pinned: r == 0,
                        max_distance: 0.02 + 0.1 * r as f32,
                        goal: 0.0,
                        radius: 0.025,
                        exposure: 1.0,
                    });
                }
            }
            let index = |r: usize, c: usize| r * columns + c % columns;
            let across = if wrap { columns } else { columns - 1 };
            let mut links = Vec::new();
            let mut link = |a: usize, b: usize, compliance: f32| {
                links.push(Link {
                    a: a as u16,
                    b: b as u16,
                    rest: particles[a].rest.distance(particles[b].rest),
                    compliance,
                })
            };
            for r in 0..rows {
                for c in 0..across {
                    link(index(r, c), index(r, c + 1), 0.0);
                    if r + 1 < rows {
                        link(index(r, c), index(r + 1, c + 1), 2e-5);
                        link(index(r, c + 1), index(r + 1, c), 2e-5);
                    }
                    link(index(r, c), index(r, c + 2), 4e-4);
                }
            }
            for c in 0..columns {
                for r in 1..rows {
                    link(index(r - 1, c), index(r, c), 0.0);
                    if r >= 2 {
                        link(index(r - 2, c), index(r, c), 4e-4);
                    }
                }
            }
            let tethers = (columns..rows * columns)
                .map(|i| Tether {
                    particle: i as u16,
                    root: (i % columns) as u16,
                    length: 0.13 * (i / columns) as f32 * 1.05,
                })
                .collect();
            PieceDef {
                particles,
                links,
                tethers,
                cones: Vec::new(),
                frames: Frames::Grid {
                    columns,
                    rows,
                    wrap,
                },
                anchor: 0,
                inertia: 0.7,
                damping: 0.4,
                drag: 0.25,
                lift: 2.5,
                friction: 0.35,
                radial: None,
                self_collide: false,
                shape: Vec::new(),
                sinks: Vec::new(),
                layer_reach: Vec::new(),
            }
        }
        // The lab's skirt and cape; the cape stays outside the skirt.
        let mut pieces = [sheet(16, 6, true, 0.25), sheet(13, 10, false, 0.3)];
        let variant = std::env::var("BENCH_VARIANT").unwrap_or_default();
        if variant.contains("nolinks") {
            for p in &mut pieces {
                p.links.clear();
            }
        }
        let count = if variant.contains("nocapsules") {
            0
        } else {
            43
        };
        let capsules: Vec<(Capsule, Capsule)> = (0..count)
            .map(|k| {
                let a = k as f32 * 0.7;
                let c = Capsule::new(
                    Vec3A::new(a.sin() * 0.12, 0.3 + 0.025 * k as f32, a.cos() * 0.1),
                    Vec3A::new(a.sin() * 0.12, 0.5 + 0.025 * k as f32, a.cos() * 0.1),
                    0.07,
                );
                (c, c)
            })
            .collect();
        let mut sims: Vec<PieceSim> = pieces.iter().map(PieceSim::new).collect();
        let angles: Vec<f32> = pieces[0].particles[..16]
            .iter()
            .map(|p| p.rest.x.atan2(p.rest.z))
            .collect();
        let mut layer = Layer::new(6, &angles);
        let mut out = Vec::new();
        let frames: usize = std::env::var("BENCH_FRAMES")
            .ok()
            .and_then(|f| f.parse().ok())
            .unwrap_or(2000);
        let mut deforms = [Affine3A::IDENTITY];
        for (def, sim) in pieces.iter().zip(&mut sims) {
            sim.reset(def, &deforms);
        }
        let started = std::time::Instant::now();
        for f in 0..frames {
            // Walk forward and sway, as a character does.
            let t = f as f32 / 60.0;
            deforms[0] = Affine3A::from_rotation_translation(
                Quat::from_rotation_y((t * 3.0).sin() * 0.2),
                bevy::math::Vec3::new(0.0, 0.0, t * 1.9),
            );
            let moved: Vec<(Capsule, Capsule)> = capsules
                .iter()
                .map(|(c, _)| {
                    let m = Capsule::new(
                        deforms[0].transform_point3a(c.a),
                        deforms[0].transform_point3a(c.b),
                        c.radius,
                    );
                    (m, m)
                })
                .collect();
            let (skirt, cape) = sims.split_at_mut(1);
            skirt[0].step(&pieces[0], &deforms, &moved, &[], &PARAMS);
            skirt[0].joints(&pieces[0], &deforms, &mut out);
            let layers: Vec<&Layer> = if variant.contains("nolayer") {
                Vec::new()
            } else {
                layer.update(
                    skirt[0].positions(),
                    deforms[0].translation,
                    Mat3A::IDENTITY,
                );
                vec![&layer]
            };
            cape[0].step(&pieces[1], &deforms, &moved, &layers, &PARAMS);
            cape[0].joints(&pieces[1], &deforms, &mut out);
        }
        let per_frame = started.elapsed().as_secs_f64() * 1e6 / frames as f64;
        let pairs: usize = sims.iter().map(|s| s.pairs.len()).sum();
        println!("BENCH sheets: {per_frame:.1} us per frame, {pairs} collision pairs last frame");
    }
}
