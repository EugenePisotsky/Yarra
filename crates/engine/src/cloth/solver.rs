//! Position-based cloth for characters: a few dozen particles per piece that the animation
//! carries, gravity and wind move, and capsules on the body push out. XPBD in small steps
//! (one pass per substep, Macklin et al. 2019), so stiffness does not depend on the frame rate.
//!
//! Everything is in render space. Where the animation alone would put a particle, its target,
//! comes from the bones that carry it as `deform × rest`: `deform` is a bone's skinning
//! transform (its pose times its inverse bind pose) and `rest` the particle's bind-space position.
use bevy::math::{Affine3A, Mat3A, Quat, Vec3A};

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
    bones: [u16; 3],
    weights: [f32; 3],
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

    /// Up to three bones; the weights are normalised.
    pub fn weighted<const N: usize>(bones: [(u16, f32); N]) -> Self {
        let mut carrier = Self {
            bones: [0; 3],
            weights: [0.0; 3],
        };
        let total: f32 = bones.iter().map(|(_, w)| w.max(0.0)).sum();
        for (k, (bone, weight)) in bones.into_iter().take(3).enumerate() {
            carrier.bones[k] = bone;
            carrier.weights[k] = if total > 0.0 {
                weight.max(0.0) / total
            } else {
                0.0
            };
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

/// A wrap-around piece's upright axis: `origin` (bind space) carried by `bone`. Its particles
/// leave upright colliders outward from the axis, never between them (a skirt between the legs).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Radial {
    pub bone: u16,
    pub origin: Vec3A,
    /// Half the height between rows: each particle stands for the cloth that far above and
    /// below it, so a knee between two rows still pushes it.
    pub reach: f32,
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
    /// Collision radius.
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
}

impl PieceDef {
    /// Render joints: one per particle, or one for a rigid body.
    pub fn joint_count(&self) -> usize {
        match self.frames {
            Frames::Rigid { .. } => 1,
            _ => self.particles.len(),
        }
    }

    /// Bind-space joint positions; a joint's inverse bind pose is a translation by minus this.
    pub fn joint_rests(&self) -> Vec<Vec3A> {
        match self.frames {
            Frames::Rigid { .. } => vec![self.particles[0].rest],
            _ => self.particles.iter().map(|p| p.rest).collect(),
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
    /// Part of the body's bulk (torso, legs): wrap-around pieces stay outside the convex hull
    /// of these rather than each one ([`Radial`]).
    pub bulk: bool,
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
        }
    }

    fn closest(&self, p: Vec3A) -> Vec3A {
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
const LAYER_GAP: f32 = 0.035;

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
    fn push_out(&self, x: Vec3A, reach: f32) -> Option<Vec3A> {
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
    anchor_prev: Option<Affine3A>,
    /// Colliders that may touch each particle this frame, as (particle, collider) pairs.
    pairs: Vec<(u16, u16)>,
    /// Sheets: each particle's frame at rest, transposed.
    rest_frames: Vec<Mat3A>,
    /// The piece's links with their particles' inverse masses; links between two pinned
    /// particles are left out.
    links: Vec<SolverLink>,
    /// Each collider's bounding sphere, grown by how far it moves this frame.
    bounds: Vec<(Vec3A, f32)>,
    /// How far each particle can get this frame.
    reach: Vec<f32>,
    /// Bounding spheres of runs of eight particles, grown by their reach.
    chunks: Vec<(Vec3A, f32)>,
    /// Pairs of particles, not neighbours on the sheet, that could meet this frame.
    self_pairs: Vec<(u16, u16)>,
    /// Each particle's wind this frame.
    gusts: Vec<Vec3A>,
    /// A wrap-around piece's axis this frame: origin, up and the height each particle covers.
    axis: Option<(Vec3A, Vec3A, f32)>,
    /// Simulated time owed from earlier frames, under one substep.
    carry: f32,
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
            anchor_prev: None,
            pairs: Vec::new(),
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
            chunks: Vec::new(),
            self_pairs: Vec::new(),
            axis: None,
            carry: 0.0,
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
                    let rests: Vec<Vec3A> = def.particles.iter().map(|p| p.rest).collect();
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
        self.gather_self_pairs(def, params.self_collide);
        self.gust(def, params);

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
        let apart = |i: usize, j: usize| {
            let (ri, ci, rj, cj) = (i / columns, i % columns, j / columns, j % columns);
            let dc = ci.abs_diff(cj);
            let dc = if wrap { dc.min(columns - dc) } else { dc };
            ri.abs_diff(rj) > 2 || dc > 2
        };
        let n = self.x.len();
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
                        if apart(i, j) && self.x[i].distance_squared(self.x[j]) < near * near {
                            self.self_pairs.push((i as u16, j as u16));
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
                        self.pairs.push((i as u16, k as u16));
                    }
                }
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
                self.x[i] = target + d * (p.max_distance / distance);
            }
        }
        // Colliders last: being outside the body matters more than staying near the pose.
        self.contacts.fill(Vec3A::ZERO);
        for &(i, k) in &self.pairs {
            let i = i as usize;
            let (before, after) = capsules[k as usize];
            let capsule = before.lerp(after, t);
            let closest = capsule.closest(self.x[i]);
            let d = self.x[i] - closest;
            let reach = capsule.radius + particles[i].radius;
            if capsule.press != Vec3A::ZERO {
                let under = d.dot(capsule.press);
                let across = d - capsule.press * under;
                let held = capsule.radius + PRESS_GAP;
                // Wider than the gaps between particles, so cloth cannot straddle the item.
                let band = capsule.width + 2.4 * particles[i].radius;
                if under < held
                    && under > held - PRESS_DEPTH
                    && across.length_squared() < band * band
                {
                    // All the way: a presser is worn over the cloth, it does not strike it.
                    self.x[i] += capsule.press * (held - under);
                    self.contacts[i] = capsule.press;
                }
                continue;
            }
            if let Some((origin, up, reach)) = self.axis
                && let Some(out) =
                    radial_push(self.x[i], &capsule, particles[i].radius, origin, up, reach)
            {
                self.x[i] += out * params.contact;
                self.contacts[i] = out.normalize_or_zero();
                continue;
            }
            let distance2 = d.length_squared();
            if distance2 >= reach * reach {
                continue;
            }
            let distance = distance2.sqrt();
            // On the axis itself, leave the way the target lies.
            let normal = if distance > 1e-6 {
                d / distance
            } else {
                (self.targets[i] - closest).normalize_or(Vec3A::Y)
            };
            // Part of the way each substep: a collider meets cloth over a few milliseconds
            // rather than kicking it in one.
            let push = closest + normal * reach - self.x[i];
            self.x[i] += push * params.contact;
            self.contacts[i] = normal;
        }

        for &(i, j) in &self.self_pairs {
            let (i, j) = (i as usize, j as usize);
            let (wi, wj) = (weight(&particles[i]), weight(&particles[j]));
            if wi + wj == 0.0 {
                continue;
            }
            let d = self.x[j] - self.x[i];
            let distance = d.length();
            if distance >= SELF_GAP || distance <= 1e-6 {
                continue;
            }
            let correction = d * ((SELF_GAP - distance) / (distance * (wi + wj)));
            self.x[i] -= correction * wi;
            self.x[j] += correction * wj;
        }
        for layer in layers {
            for (i, p) in particles.iter().enumerate() {
                if p.pinned {
                    continue;
                }
                if let Some(outside) = layer.push_out(self.x[i], p.radius + LAYER_GAP) {
                    self.contacts[i] = (outside - self.x[i]).normalize_or_zero();
                    let push = outside - self.x[i];
                    self.x[i] += push * params.contact;
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

    /// The render joints' world poses (rotation and position); each joint's inverse bind pose
    /// is a translation by minus its rest position ([`PieceDef::joint_rests`]).
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

/// For a wrap-around piece: how far to move `x` outward from the axis to lie outside an upright
/// bulk `capsule`'s support along its direction, over the heights `reach` either side of it
/// (`None` if it already does, or the capsule is not bulk or leans past 60°). Taken over every
/// bulk capsule, that keeps the piece outside their convex hull: it spans the gap between the
/// legs as cloth does, and a particle never hops from one side of a leg to the other. A leaning
/// capsule's slice is an ellipse, longer along its lean.
fn radial_push(
    x: Vec3A,
    capsule: &Capsule,
    radius: f32,
    origin: Vec3A,
    up: Vec3A,
    reach: f32,
) -> Option<Vec3A> {
    let along = capsule.b - capsule.a;
    let length = along.length();
    let rise = along.dot(up);
    if !capsule.bulk || length <= 1e-5 || rise.abs() < 0.5 * length {
        return None;
    }
    let rel = x - origin;
    let height = rel.dot(up);
    let flat = rel - up * height;
    let r = flat.length();
    let out = if r > 1e-4 { flat / r } else { return None };
    let lean = along - up * rise;
    let stretch = length / rise.abs();
    let (u, v) = match lean.try_normalize() {
        Some(u) => (u, u.cross(up)),
        None => (out, out.cross(up)),
    };
    let (ha, hb) = ((capsule.a - origin).dot(up), (capsule.b - origin).dot(up));
    let mut wall = f32::MIN;
    for h in [height - reach, height, height + reach] {
        let t = ((h - ha) / (hb - ha)).clamp(0.0, 1.0);
        let centre = capsule.a + along * t;
        // Past either end the slice narrows over the end cap.
        let beyond = h - (centre - origin).dot(up);
        if beyond.abs() >= capsule.radius {
            continue;
        }
        let slice = (capsule.radius * capsule.radius - beyond * beyond).sqrt();
        let c = (centre - origin) - up * (centre - origin).dot(up);
        let (along_lean, across) = (out.dot(u) * slice * stretch, out.dot(v) * slice);
        wall = wall.max(c.dot(out) + along_lean.hypot(across) + radius);
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
        contact: 0.6,
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
        // A lying capsule, or one not of the bulk, is left to the ordinary push.
        let arm = Capsule {
            bulk: true,
            ..Capsule::new(Vec3A::new(-0.3, 0.5, 0.0), Vec3A::new(0.3, 0.5, 0.0), 0.05)
        };
        assert!(radial_push(x, &arm, 0.02, origin, Vec3A::Y, 0.0).is_none());
        let item = Capsule::new(Vec3A::new(0.1, 1.0, 0.0), Vec3A::new(0.1, 0.0, 0.0), 0.07);
        assert!(radial_push(x, &item, 0.02, origin, Vec3A::Y, 0.0).is_none());
    }

    #[test]
    fn a_presser_holds_particles_on_its_inner_side() {
        let def = strand(6);
        let deforms = [Affine3A::IDENTITY];
        let mut sim = PieceSim::new(&def);
        sim.reset(&def, &deforms);
        // A vertical strap just behind the strand, pressing towards +Z; the strand is pushed
        // out towards -Z and must stay in front of the strap anyway.
        let mut strap = Capsule::new(
            Vec3A::new(0.0, 0.0, -0.05),
            Vec3A::new(0.0, -0.6, -0.05),
            0.012,
        );
        strap.press = Vec3A::Z;
        strap.width = 0.03;
        for _ in 0..120 {
            for i in 1..6 {
                sim.v[i] = Vec3A::new(0.0, 0.0, -1.0);
            }
            run(&def, &mut sim, &deforms, &[strap], 1);
        }
        for (i, x) in sim.positions().iter().enumerate().skip(1) {
            let held = -0.05 + 0.012 + PRESS_GAP;
            assert!(
                x.z > held - 2e-3,
                "particle {i} at {x:?} got past the strap"
            );
        }
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
