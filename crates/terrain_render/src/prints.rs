//! Ground prints: where feet push the soft layer (snow) down. The application lists the feet
//! within its reach each frame ([`GroundPresses`]); presses are kept in a log, and a compute pass
//! draws them into two toroidal windows centred on the player, 41 m at 2 cm and 164 m at 8 cm
//! (`shaders/terrain/press.wesl`). Like Rise of the Tomb Raider's deformation map, a window holds
//! not the snow but how high over the firm ground it may stand: a foot passing through the layer
//! lowers it to its sole, with walls rising steeply beside it, and the lowest sole near. Over the
//! same tiles and those beside them a second pass resolves that, with the soft layer's depth ([`PrintGround`],
//! `world::soft_layer`), into the relief the terrain draws: how far feet lowered the ground, rims
//! risen beside trenches, and its slope, filtered level by level so vertices take it at their own
//! spacing (`shaders/terrain/print_relief.wesl`, `shaders/terrain/prints.wesl`). Shallow snow takes
//! prints and deep snow trenches.
//!
//! The windows hold nothing the log does not: ground a window moves onto starts undisturbed and
//! takes again every logged press that reaches it, so a trail lasts as long as the log holds it,
//! however far the player walks away and back. Between its logged presses a moving foot's sweep
//! since its last is drawn every frame, so a trench grows with the foot; the next logged sweep
//! takes it in.
//!
//! Falling snow fills prints: each press keeps how much snow had fallen when it was made
//! ([`GroundPresses::snowfall`]), and is drawn as much shallower as has fallen since. While snow
//! falls the tiles holding prints are brought up to date a few at a time, and a press the snow has
//! buried leaves the log.
mod gpu;

use bevy::{math::DVec2, prelude::*};

use std::collections::{HashMap, HashSet, VecDeque};
use world::WorldSpaceId;

pub(crate) use gpu::{relief_handle, windows_handle};

/// Texels per side of a window.
const SIDE: u32 = if cfg!(target_os = "ios") { 1024 } else { 2048 };

/// The fine window and the coarse one around it.
const LEVELS: usize = 2;

/// Metres per texel of each window.
const TEXEL_METRES: [f64; LEVELS] = [0.02, 0.08];

/// Texels per side of a tile: windows move by whole tiles, and the pass updates tiles.
const TILE: i64 = 8;

/// Presses the log keeps: a walker through deep snow logs about thirty a second, so half an hour
/// of it.
const LOG_CAPACITY: usize = 65536;

/// A foot is logged again only once it moves this far (metres, under half a sole) or reaches this
/// much lower. A moving foot logs its sweep since its last press, so its way through deep snow
/// is one smooth trench, not a row of soles.
const LOG_MOVED: f64 = 0.1;

const LOG_LOWER: f32 = 0.01;

/// A foot moved this far from its last press (metres) has its sweep drawn until it is logged, and
/// one moving less than the second in a frame has stopped: its sweep is logged then, however
/// short. A planted foot sliding a little draws nothing.
const DRAW_MOVED: f64 = 0.01;

const STOPPED: f64 = 0.002;

/// Most presses taken in a frame.
const MAX_PRESSES: usize = 64;

/// Feet higher than this over the firm ground press nothing: no soft layer is deeper.
const SOFT_REACH: f32 = 0.6;

/// Widest a press's circles may be, metres: the legs' channel.
const MAX_RADIUS: f32 = 1.0;

/// While snow falls, a tile holding prints is filled once this much more has fallen on it
/// (metres), at most this many tiles a frame.
const FILL_STEP: f64 = 0.003;

const FILL_BUDGET: usize = 256;

/// Snow this deep fallen on a press since it was made buries it, and the rims beside its trench:
/// deeper than any soft layer it could press.
const BURIED: f64 = SOFT_REACH as f64 + 0.2;

/// How far past its outline a sole's trench wall still matters (its shape is
/// `terrain/print_map.wesl`'s), metres: past the rim and the crumbs thrown beside a trench.
const WALL_REACH_METRES: f32 = 0.3;

/// A foot pressing the ground this frame, or the legs wading. Its outline is two circles, at the
/// heel and the ball, joined along their tangents and drawn in at the middle by its arch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundPress {
    /// Stable for one foot, so a foot held still is logged once.
    pub source: u64,
    /// Render-space centres of the heel and ball circles (x, z).
    pub heel: Vec2,
    pub ball: Vec2,
    /// Radii of the heel and ball circles, metres.
    pub radii: [f32; 2],
    /// How high over the firm ground the heel's and the ball's undersides are, metres: the soft
    /// layer is pushed down to them where it stands higher.
    pub floor: [f32; 2],
    /// How far the outline draws in at its middle, as a share of its width: a sole's arch, none
    /// for the legs' channel, whose sweeps must join smoothly.
    pub arch: f32,
}

/// What presses the ground this frame, listed by the application.
#[derive(Resource, Clone, Debug, Default)]
pub struct GroundPresses {
    /// The world space pressed; another one starts the prints over.
    pub space: Option<WorldSpaceId>,
    /// Where the windows centre, in render space: the player.
    pub focus: Option<Vec3>,
    /// The render origin's world XZ, metres.
    pub origin: DVec2,
    pub presses: Vec<GroundPress>,
    /// Snow fallen on the soft layer since the game began, metres: it fills prints made before.
    pub snowfall: f64,
}

/// Systems listing [`GroundPresses`] run in this set; the prints take them after it.
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub struct GroundPressSystems;

/// The soft layer the prints lower: the drawn world's leaf depths
/// (`ClipmapTextures::print_ground`). Without it nothing is lowered.
#[derive(Resource, Clone, Debug)]
pub struct PrintGround {
    pub(crate) soft: Handle<Image>,
    /// Metres between the leaves' samples, and the texture's texels per side.
    pub(crate) spacing: f64,
    pub(crate) side: u32,
}

impl PrintGround {
    /// Where the depths lie for the resolve pass: the render origin's texel, texels per metre,
    /// texels per side.
    fn placement(&self, origin: DVec2) -> [f32; 4] {
        let side = i64::from(self.side);
        let texel = |v: f64| ((v / self.spacing).round() as i64).rem_euclid(side) as f32;
        [
            texel(origin.x),
            texel(origin.y),
            (1.0 / self.spacing) as f32,
            self.side as f32,
        ]
    }
}

/// A logged press in world coordinates, and the snow fallen when it was made.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Logged {
    heel: DVec2,
    ball: DVec2,
    radii: [f32; 2],
    floor: [f32; 2],
    arch: f32,
    snowfall: f64,
}

impl Logged {
    fn moved_from(&self, last: &Self, distance: f64) -> bool {
        self.heel.distance(last.heel) > distance || self.ball.distance(last.ball) > distance
    }

    fn lower_than(&self, last: &Self) -> bool {
        (0..2).any(|i| self.floor[i] < last.floor[i] - LOG_LOWER)
    }

    /// The ground a foot swept from `last` to here: its ball's way, which its heel follows, as low
    /// as its sole reached at either end and with no waist. Sweep after sweep joins end to end at
    /// the same floor, so a foot through deep snow lays one smooth lane.
    fn swept_from(&self, last: &Self) -> Self {
        let lowest = |l: &Self| l.floor[0].min(l.floor[1]);
        Self {
            heel: last.ball,
            ball: self.ball,
            radii: [last.radii[1], self.radii[1]],
            floor: [lowest(last), lowest(self)],
            arch: 0.0,
            snowfall: self.snowfall,
        }
    }

    /// The world tiles of a window's level the press reaches, as [x0, z0, x1, z1).
    fn tiles(&self, level: usize) -> [i64; 4] {
        let reach = f64::from(self.radii[0].max(self.radii[1]) + WALL_REACH_METRES);
        let min = self.heel.min(self.ball) - reach;
        let max = self.heel.max(self.ball) + reach;
        world_tiles([min.x, min.y, max.x, max.y], level)
    }

    /// Heel and ball in render space, their radii and floors, the arch and the snow fallen on it
    /// by `snowfall`.
    fn stamp(&self, origin: DVec2, snowfall: f64) -> [f32; 12] {
        let heel = (self.heel - origin).as_vec2();
        let ball = (self.ball - origin).as_vec2();
        [
            heel.x,
            heel.y,
            ball.x,
            ball.y,
            self.radii[0],
            self.radii[1],
            self.floor[0],
            self.floor[1],
            self.arch,
            (snowfall - self.snowfall).max(0.0) as f32,
            0.0,
            0.0,
        ]
    }
}

/// A tile holding prints: the snow fallen when its values were last brought up to date, and when
/// its newest press was made.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Filled {
    at: f64,
    pressed: f64,
}

/// The log, the windows and the work the render world takes each frame.
#[derive(Resource, Default)]
pub(crate) struct GroundPrints {
    space: Option<WorldSpaceId>,
    /// Each window's first texel (a world texel index, a whole tile), once there is a focus.
    windows: Option<[[i64; 2]; LEVELS]>,
    log: VecDeque<Logged>,
    /// Each foot in reach's last logged press.
    last: HashMap<u64, Logged>,
    /// Each foot in reach that moved since its last logged press, as it was last frame.
    moving: HashMap<u64, Logged>,
    /// The windows' tiles holding prints, by level and world tile.
    filled: HashMap<(usize, i64, i64), Filled>,
    pub(crate) frame: PrintFrame,
}

/// What the render world draws this frame.
#[derive(Clone, Debug, Default)]
pub(crate) struct PrintFrame {
    /// Per window, the two texels `prints.wesl` reads: the render-space corner of its first
    /// texel (f32 bits) and the toroidal texel holding it; metres per texel (f32 bits), texels per
    /// side, 1 while drawn, and samples per texel side taken when pressing it.
    pub(crate) windows: [[u32; 8]; LEVELS],
    /// Per window, the world's fine texel holding its first texel (x, z), which the snow's
    /// unevenness is drawn by.
    pub(crate) cells: [i32; 4],
    /// Per tile: its x and z within its window, its first entry in `indices`, and its level
    /// (bit 0), whether it starts undisturbed (bit 1) and how many stamps it takes (from bit 2).
    pub(crate) tiles: Vec<[u32; 4]>,
    /// Stamps by tile, oldest first.
    pub(crate) indices: Vec<u32>,
    /// Render-space heel and ball, radii, floors and arch, as [`Logged::stamp`] lays them out.
    pub(crate) stamps: Vec<[f32; 12]>,
    /// Per tile, snow fallen on it since its values were last brought up to date, metres.
    pub(crate) fills: Vec<f32>,
    /// The soft layer's depths, and where they lie ([`PrintGround::placement`]; no side, none).
    pub(crate) soft: Option<AssetId<Image>>,
    pub(crate) ground: [f32; 4],
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GroundPresses>()
        .init_resource::<GroundPrints>()
        .add_systems(Startup, gpu::init_images)
        .add_systems(PostUpdate, update.after(GroundPressSystems));
    gpu::install(app);
}

fn update(
    presses: Res<GroundPresses>,
    ground: Option<Res<PrintGround>>,
    mut uploads: ResMut<crate::clipmap::ClipmapUploads>,
    mut prints: ResMut<GroundPrints>,
) {
    let changed = uploads.take_soft_changes();
    let ground = ground.as_deref();
    let changed: Vec<_> = changed
        .into_iter()
        .filter(|(soft, _)| ground.is_some_and(|g| g.soft.id() == *soft))
        .map(|(_, rect)| rect)
        .collect();
    prints.update(&presses, ground, &changed);
}

impl GroundPrints {
    /// Takes this frame's presses and lists the work: tiles the windows entered or presses
    /// reached, and those under `changed` (world rects, metres) where the soft layer changed.
    fn update(
        &mut self,
        presses: &GroundPresses,
        ground: Option<&PrintGround>,
        changed: &[[f64; 4]],
    ) {
        if presses.space != self.space {
            *self = Self {
                space: presses.space,
                ..default()
            };
        }

        let (fresh, drawn) = self.take(presses);
        let snowfall = presses.snowfall;

        while self
            .log
            .front()
            .is_some_and(|press| snowfall - press.snowfall > BURIED)
        {
            self.log.pop_front();
        }

        let fresh = fresh.min(self.log.len());
        let Some(focus) = presses.focus else {
            self.windows = None;
            self.filled.clear();
            self.frame = PrintFrame::default();
            return;
        };
        let focus = presses.origin + DVec2::new(f64::from(focus.x), f64::from(focus.z));
        let windows = TEXEL_METRES.map(|texel| window_first(focus, texel));
        let previous = self.windows.replace(windows);
        let mut work = Work::default();

        for level in 0..LEVELS {
            let to_tiles = |first: [i64; 2]| first.map(|t| t.div_euclid(TILE));
            let window = to_tiles(windows[level]);
            let entered = entered(previous.map(|p| to_tiles(p[level])), window);
            work.press(level, window, &entered, &self.log, fresh);
            work.draw(level, window, self.log.len(), &drawn);
            work.resolve(level, window, changed);

            for tile in self.stale(level, window, snowfall) {
                work.tile(level, window, tile, false);
            }

            work.resolve_beside(level, window);
        }

        self.frame = work.finish(windows, presses.origin, snowfall, &mut self.filled);
        let n = i64::from(SIDE) / TILE;
        self.filled.retain(|&(level, x, z), filled| {
            let window = windows[level].map(|t| t.div_euclid(TILE));
            snowfall - filled.pressed <= BURIED
                && inside([window[0], window[1], window[0] + n, window[1] + n], x, z)
        });

        if let Some(ground) = ground {
            self.frame.soft = Some(ground.soft.id());
            self.frame.ground = ground.placement(presses.origin);
        }
    }

    /// Up to [`FILL_BUDGET`] tiles of a window holding prints that [`FILL_STEP`] more snow has
    /// fallen on since they were last brought up to date.
    fn stale(&self, level: usize, window: [i64; 2], snowfall: f64) -> Vec<[i64; 2]> {
        let n = i64::from(SIDE) / TILE;
        let whole = [window[0], window[1], window[0] + n, window[1] + n];
        self.filled
            .iter()
            .filter(|&(&(l, x, z), filled)| {
                l == level && inside(whole, x, z) && snowfall - filled.at >= FILL_STEP
            })
            .map(|(&(_, x, z), _)| [x, z])
            .take(FILL_BUDGET)
            .collect()
    }

    /// Logs this frame's presses that add to their foot's last; returns how many it logged and
    /// the sweeps of feet moving short of their next logged press, drawn this frame alone.
    fn take(&mut self, presses: &GroundPresses) -> (usize, Vec<Logged>) {
        let mut touching = HashSet::new();
        let mut drawn = Vec::new();
        let before = self.log.len();

        for press in presses.presses.iter().take(MAX_PRESSES) {
            let finite = press.heel.is_finite()
                && press.ball.is_finite()
                && press
                    .radii
                    .iter()
                    .chain(&press.floor)
                    .chain([&press.arch])
                    .all(|v| v.is_finite());

            if !finite || press.floor.iter().all(|&f| f >= SOFT_REACH) {
                continue;
            }

            touching.insert(press.source);
            let at = |p: Vec2| presses.origin + p.as_dvec2();
            let logged = Logged {
                heel: at(press.heel),
                ball: at(press.ball),
                radii: press.radii.map(|r| r.clamp(0.005, MAX_RADIUS)),
                floor: press.floor.map(|f| f.clamp(0.0, SOFT_REACH)),
                arch: press.arch.clamp(0.0, 0.5),
                snowfall: presses.snowfall,
            };
            let previous = self.moving.remove(&press.source);
            let Some(last) = self.last.insert(press.source, logged) else {
                self.log.push_back(logged);
                continue;
            };

            let moved = logged.moved_from(&last, DRAW_MOVED);
            let stopped = previous.is_some_and(|p| !logged.moved_from(&p, STOPPED));
            let lower = logged.lower_than(&last);

            if logged.moved_from(&last, LOG_MOVED) || lower || (moved && stopped) {
                if moved {
                    self.log.push_back(logged.swept_from(&last));
                }

                if lower {
                    self.log.push_back(logged);
                }
            } else {
                self.last.insert(press.source, last);

                if moved {
                    drawn.push(logged.swept_from(&last));
                    self.moving.insert(press.source, logged);
                }
            }
        }

        // A foot leaving reach logs how far it got; one out of reach is logged whatever it was
        // before when it comes back.
        for (source, moved) in std::mem::take(&mut self.moving) {
            if touching.contains(&source) {
                self.moving.insert(source, moved);
            } else if let Some(last) = self.last.get(&source) {
                self.log.push_back(moved.swept_from(last));
            }
        }

        self.last.retain(|source, _| touching.contains(source));
        let fresh = self.log.len() - before;

        while self.log.len() > LOG_CAPACITY {
            self.log.pop_front();
        }

        (fresh.min(self.log.len()), drawn)
    }
}

/// The first texel of a window centred on `focus` (world metres), on a whole tile.
fn window_first(focus: DVec2, texel: f64) -> [i64; 2] {
    let half = i64::from(SIDE / 2);
    [focus.x, focus.y].map(|v| ((v / texel).floor() as i64).div_euclid(TILE) * TILE - half)
}

/// The tiles of a window first at `window` (world tiles) that one first at `previous` did not
/// hold: at most two disjoint rectangles [x0, z0, x1, z1), the whole window when nothing is held.
fn entered(previous: Option<[i64; 2]>, window: [i64; 2]) -> Vec<[i64; 4]> {
    let n = i64::from(SIDE) / TILE;
    let whole = vec![[window[0], window[1], window[0] + n, window[1] + n]];
    let Some(previous) = previous else {
        return whole;
    };
    let [dx, dz] = [window[0] - previous[0], window[1] - previous[1]];

    if dx.abs() >= n || dz.abs() >= n {
        return whole;
    }

    let mut rects = Vec::new();

    // Columns the previous window did not cover, the window's whole depth.
    if dx > 0 {
        rects.push([previous[0] + n, window[1], window[0] + n, window[1] + n]);
    } else if dx < 0 {
        rects.push([window[0], window[1], previous[0], window[1] + n]);
    }

    // Rows it did not cover, within the columns it did.
    let (x0, x1) = (
        window[0].max(previous[0]),
        (window[0] + n).min(previous[0] + n),
    );

    if dz > 0 {
        rects.push([x0, previous[1] + n, x1, window[1] + n]);
    } else if dz < 0 {
        rects.push([x0, window[1], x1, previous[1]]);
    }

    rects
}

/// The world tiles of a window's level covering a world rect (metres), as [x0, z0, x1, z1).
fn world_tiles(rect: [f64; 4], level: usize) -> [i64; 4] {
    let tile = TEXEL_METRES[level] * TILE as f64;
    [
        (rect[0] / tile).floor() as i64,
        (rect[1] / tile).floor() as i64,
        (rect[2] / tile).floor() as i64 + 1,
        (rect[3] / tile).floor() as i64 + 1,
    ]
}

fn overlap(a: [i64; 4], b: [i64; 4]) -> Option<[i64; 4]> {
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

fn inside(rect: [i64; 4], x: i64, z: i64) -> bool {
    (rect[0]..rect[2]).contains(&x) && (rect[1]..rect[3]).contains(&z)
}

/// A frame's tiles, each listed once, with the stamps each takes in log order.
#[derive(Default)]
struct Work {
    tiles: Vec<(usize, [i64; 2], [i64; 2], bool)>,
    lists: Vec<Vec<u32>>,
    by_tile: HashMap<(usize, i64, i64), usize>,
    stamps: Vec<Logged>,
    /// Each logged press's stamp this frame, by its place in the log.
    stamp_of: HashMap<usize, u32>,
}

impl Work {
    /// A window's work: tiles it entered start undisturbed and take every logged press reaching
    /// them; elsewhere it takes the `fresh` presses at the end of the log.
    fn press(
        &mut self,
        level: usize,
        window: [i64; 2],
        entered: &[[i64; 4]],
        log: &VecDeque<Logged>,
        fresh: usize,
    ) {
        let n = i64::from(SIDE) / TILE;
        let whole = [window[0], window[1], window[0] + n, window[1] + n];

        for rect in entered {
            for z in rect[1]..rect[3] {
                for x in rect[0]..rect[2] {
                    self.tile(level, window, [x, z], true);
                }
            }
        }

        if !entered.is_empty() {
            for (at, press) in log.iter().enumerate() {
                for rect in entered
                    .iter()
                    .filter_map(|r| overlap(press.tiles(level), *r))
                {
                    self.add(level, window, rect, at, press, |_, _| true);
                }
            }
        }

        let first_fresh = log.len() - fresh;

        for (at, press) in log.iter().enumerate().skip(first_fresh) {
            if let Some(rect) = overlap(press.tiles(level), whole) {
                self.add(level, window, rect, at, press, |x, z| {
                    !entered.iter().any(|r| inside(*r, x, z))
                });
            }
        }
    }

    /// The sweeps drawn this frame alone, after the `logged` presses of the log, on every tile
    /// they reach: those the window entered too, which the log does not give them.
    fn draw(&mut self, level: usize, window: [i64; 2], logged: usize, drawn: &[Logged]) {
        let n = i64::from(SIDE) / TILE;
        let whole = [window[0], window[1], window[0] + n, window[1] + n];

        for (i, press) in drawn.iter().enumerate() {
            if let Some(rect) = overlap(press.tiles(level), whole) {
                self.add(level, window, rect, logged + i, press, |_, _| true);
            }
        }
    }

    /// A window's tiles under `changed` (world rects, metres) are resolved again, pressing
    /// nothing new.
    fn resolve(&mut self, level: usize, window: [i64; 2], changed: &[[f64; 4]]) {
        let n = i64::from(SIDE) / TILE;
        let whole = [window[0], window[1], window[0] + n, window[1] + n];

        for rect in changed
            .iter()
            .filter_map(|r| overlap(world_tiles(*r, level), whole))
        {
            for z in rect[1]..rect[3] {
                for x in rect[0]..rect[2] {
                    self.tile(level, window, [x, z], false);
                }
            }
        }
    }

    /// The tiles beside a window's listed ones are resolved again, pressing nothing: a texel's
    /// slope reads the texels on either side of it, so a tile's edge changes with its
    /// neighbour's. Without this a tile entering at the window's edge would keep the slope it took
    /// beside ground the window had yet to reach, a line of a trail from across it.
    fn resolve_beside(&mut self, level: usize, window: [i64; 2]) {
        let n = i64::from(SIDE) / TILE;
        let whole = [window[0], window[1], window[0] + n, window[1] + n];
        let listed: Vec<[i64; 2]> = self
            .tiles
            .iter()
            .filter(|t| t.0 == level)
            .map(|t| t.2)
            .collect();

        for [x, z] in listed {
            for [dx, dz] in [[1, 0], [-1, 0], [0, 1], [0, -1]] {
                if inside(whole, x + dx, z + dz) {
                    self.tile(level, window, [x + dx, z + dz], false);
                }
            }
        }
    }

    fn tile(&mut self, level: usize, window: [i64; 2], tile: [i64; 2], reset: bool) -> usize {
        *self
            .by_tile
            .entry((level, tile[0], tile[1]))
            .or_insert_with(|| {
                self.tiles.push((level, window, tile, reset));
                self.lists.push(Vec::new());
                self.tiles.len() - 1
            })
    }

    fn add(
        &mut self,
        level: usize,
        window: [i64; 2],
        rect: [i64; 4],
        at: usize,
        press: &Logged,
        wanted: impl Fn(i64, i64) -> bool,
    ) {
        let stamps = &mut self.stamps;
        let stamp = *self.stamp_of.entry(at).or_insert_with(|| {
            stamps.push(*press);
            stamps.len() as u32 - 1
        });

        for z in rect[1]..rect[3] {
            for x in rect[0]..rect[2] {
                if wanted(x, z) {
                    let tile = self.tile(level, window, [x, z], false);
                    self.lists[tile].push(stamp);
                }
            }
        }
    }

    /// The frame's work, each tile brought up to date with the snow fallen on it since `filled`
    /// last saw it, which then holds the tiles holding prints as this frame leaves them.
    fn finish(
        self,
        windows: [[i64; 2]; LEVELS],
        origin: DVec2,
        snowfall: f64,
        filled: &mut HashMap<(usize, i64, i64), Filled>,
    ) -> PrintFrame {
        let mut frame = PrintFrame {
            stamps: self
                .stamps
                .iter()
                .map(|s| s.stamp(origin, snowfall))
                .collect(),
            ..default()
        };

        for level in 0..LEVELS {
            let texel = TEXEL_METRES[level];
            let first = windows[level];
            let corner = [0, 1].map(|i| (first[i] as f64 * texel - origin[i]) as f32);
            let wrap = first.map(|t| t.rem_euclid(i64::from(SIDE)) as u32);
            let samples = (texel / TEXEL_METRES[0]).round() as u32;
            frame.windows[level] = [
                corner[0].to_bits(),
                corner[1].to_bits(),
                wrap[0],
                wrap[1],
                (texel as f32).to_bits(),
                SIDE,
                1,
                samples,
            ];

            // World texels wrap the noise past 40,000 km.
            frame.cells[2 * level] = (first[0] * i64::from(samples)) as i32;
            frame.cells[2 * level + 1] = (first[1] * i64::from(samples)) as i32;
        }

        for ((level, window, tile, reset), list) in self.tiles.into_iter().zip(self.lists) {
            let key = (level, tile[0], tile[1]);
            let held = filled.get(&key).filter(|_| !reset).copied();
            frame
                .fills
                .push(held.map_or(0.0, |h| (snowfall - h.at).max(0.0) as f32));
            let newest = list
                .iter()
                .map(|&i| self.stamps[i as usize].snowfall)
                .reduce(f64::max);

            match (newest, held) {
                (Some(newest), _) => {
                    let pressed = held.map_or(newest, |h| h.pressed.max(newest));
                    filled.insert(
                        key,
                        Filled {
                            at: snowfall,
                            pressed,
                        },
                    );
                }
                (None, Some(held)) => {
                    filled.insert(
                        key,
                        Filled {
                            at: snowfall,
                            ..held
                        },
                    );
                }
                (None, None) => {
                    filled.remove(&key);
                }
            }

            let local = [tile[0] - window[0], tile[1] - window[1]].map(|t| t as u32);
            let flags = level as u32 | u32::from(reset) << 1 | (list.len() as u32) << 2;
            frame
                .tiles
                .push([local[0], local[1], frame.indices.len() as u32, flags]);
            frame.indices.extend(list);
        }

        frame
    }
}

/// The sole's reach, as the pass reads it.
fn shape() -> [f32; 4] {
    [WALL_REACH_METRES, 0.0, 0.0, 0.0]
}

#[cfg(test)]
mod tests;
