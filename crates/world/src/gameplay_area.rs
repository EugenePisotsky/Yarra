//! Named places the game's rules react to: a ground polygon with an optional height range.
//!
//! The world stores and ships the shapes. What happens there belongs to gameplay content,
//! which knows a place only by its name.
use crate::WorldSpaceId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub const MAX_GAMEPLAY_AREAS: usize = 4096;
pub const MAX_GAMEPLAY_AREA_POINTS: usize = 64;
/// How far past an edge an actor already inside still counts as inside, so standing on a
/// boundary does not enter and leave every frame.
pub const GAMEPLAY_AREA_EXIT_MARGIN: f64 = 0.25;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameplayArea {
    /// The name gameplay content uses, such as `guard/gate_post`. Unique in the project.
    pub name: String,
    pub space: WorldSpaceId,
    /// World-space X/Z corners in order; the last joins the first.
    pub points: Vec<[f64; 2]>,
    /// Lowest and highest world Y that count as inside. `None` is any height.
    pub height: Option<[f32; 2]>,
}

/// The rule gameplay names follow, so every painted area can be referred to by content.
pub fn valid_gameplay_area_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-/.".contains(&b))
}

impl GameplayArea {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_gameplay_area_name(&self.name) {
            return Err("an area name is 1-96 lowercase letters, digits or _ - / . characters");
        }
        if !(3..=MAX_GAMEPLAY_AREA_POINTS).contains(&self.points.len()) {
            return Err("an area has between 3 and 64 corners");
        }
        if self
            .points
            .iter()
            .flatten()
            .any(|v| !v.is_finite() || v.abs() > 1_000_000.)
        {
            return Err("an area corner is outside the world");
        }
        if self.height.is_some_and(|[low, high]| {
            !low.is_finite() || !high.is_finite() || low > high || low.abs().max(high.abs()) > 1e6
        }) {
            return Err("an area's height range is invalid");
        }
        Ok(())
    }

    /// Smallest and largest X/Z of the corners.
    pub fn bounds(&self) -> [[f64; 2]; 2] {
        self.points.iter().fold(
            [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]],
            |[low, high], p| {
                [
                    [low[0].min(p[0]), low[1].min(p[1])],
                    [high[0].max(p[0]), high[1].max(p[1])],
                ]
            },
        )
    }

    fn edges(&self) -> impl Iterator<Item = ([f64; 2], [f64; 2])> + '_ {
        let last = self.points.last().copied();
        self.points.iter().scan(last, |previous, &point| {
            let edge = ((*previous)?, point);
            *previous = Some(point);
            Some(edge)
        })
    }

    /// Whether the ground position is inside the polygon (even-odd rule).
    pub fn covers(&self, x: f64, z: f64) -> bool {
        self.edges()
            .filter(|(a, b)| {
                (a[1] > z) != (b[1] > z) && x < (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0]
            })
            .count()
            % 2
            == 1
    }

    /// Distance from the ground position to the nearest edge.
    pub fn edge_distance(&self, x: f64, z: f64) -> f64 {
        self.edges()
            .map(|(a, b)| {
                let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
                let length = dx * dx + dz * dz;
                let t = if length > 0. {
                    (((x - a[0]) * dx + (z - a[1]) * dz) / length).clamp(0., 1.)
                } else {
                    0.
                };
                (x - a[0] - t * dx).hypot(z - a[1] - t * dz)
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// Whether a world position is inside; `margin` widens the polygon and the height range.
    pub fn contains(&self, position: [f64; 3], margin: f64) -> bool {
        self.height.is_none_or(|[low, high]| {
            (f64::from(low) - margin..=f64::from(high) + margin).contains(&position[1])
        }) && (self.covers(position[0], position[2])
            || margin > 0. && self.edge_distance(position[0], position[2]) <= margin)
    }

    /// A ground position inside the polygon, for walking there: the middle of the corners
    /// when that is inside, otherwise the middle of the widest stretch on the line through it.
    pub fn interior_point(&self) -> [f64; 2] {
        let count = self.points.len().max(1) as f64;
        let middle = self
            .points
            .iter()
            .fold([0.; 2], |sum, p| [sum[0] + p[0], sum[1] + p[1]])
            .map(|v| v / count);
        if self.covers(middle[0], middle[1]) {
            return middle;
        }
        let z = middle[1];
        let mut crossings: Vec<f64> = self
            .edges()
            .filter(|(a, b)| (a[1] > z) != (b[1] > z))
            .map(|(a, b)| (b[0] - a[0]) * (z - a[1]) / (b[1] - a[1]) + a[0])
            .collect();
        crossings.sort_by(f64::total_cmp);
        crossings
            .as_chunks::<2>()
            .0
            .iter()
            .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
            .map_or(middle, |span| [(span[0] + span[1]) / 2., z])
    }
}

/// Checks a project's whole set: every area, the count, and that names are not shared.
pub fn validate_gameplay_areas(areas: &[GameplayArea]) -> Result<(), &'static str> {
    if areas.len() > MAX_GAMEPLAY_AREAS {
        return Err("too many gameplay areas");
    }
    let mut names = std::collections::HashSet::new();
    for area in areas {
        area.validate()?;
        if !names.insert(area.name.as_str()) {
            return Err("two gameplay areas share a name");
        }
    }
    Ok(())
}

/// A world's areas with their bounds, for asking where a position is. Checking a position
/// against every area's bounds costs about a microsecond per thousand areas.
#[derive(Debug, Clone, Default)]
pub struct GameplayAreaIndex {
    areas: Arc<[GameplayArea]>,
    bounds: Vec<[[f64; 2]; 2]>,
}

impl GameplayAreaIndex {
    pub fn new(areas: Arc<[GameplayArea]>) -> Self {
        let bounds = areas.iter().map(GameplayArea::bounds).collect();
        Self { areas, bounds }
    }

    pub fn areas(&self) -> &[GameplayArea] {
        &self.areas
    }

    pub fn find(&self, name: &str) -> Option<&GameplayArea> {
        self.areas.iter().find(|area| area.name == name)
    }

    /// The areas holding a position. One for which `inside` answers true is kept until the
    /// position is [`GAMEPLAY_AREA_EXIT_MARGIN`] beyond it.
    pub fn at<'a>(
        &'a self,
        space: WorldSpaceId,
        position: [f64; 3],
        inside: impl Fn(&str) -> bool + 'a,
    ) -> impl Iterator<Item = &'a GameplayArea> + 'a {
        let m = GAMEPLAY_AREA_EXIT_MARGIN;
        self.areas
            .iter()
            .zip(&self.bounds)
            .filter(move |(area, [low, high])| {
                area.space == space
                    && position[0] >= low[0] - m
                    && position[0] <= high[0] + m
                    && position[2] >= low[1] - m
                    && position[2] <= high[1] + m
            })
            .filter(move |(area, _)| {
                area.contains(position, if inside(&area.name) { m } else { 0. })
            })
            .map(|(area, _)| area)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(name: &str, points: &[[f64; 2]]) -> GameplayArea {
        GameplayArea {
            name: name.into(),
            space: WorldSpaceId(1),
            points: points.to_vec(),
            height: None,
        }
    }

    #[test]
    fn a_position_is_inside_by_ground_polygon_and_height() {
        let mut yard = area("yard", &[[0., 0.], [10., 0.], [10., 10.], [0., 10.]]);
        assert_eq!(yard.validate(), Ok(()));
        assert!(yard.contains([5., 900., 5.], 0.));
        assert!(!yard.contains([10.1, 0., 5.], 0.));
        assert!(yard.contains([10.1, 0., 5.], 0.25));
        assert!(!yard.contains([10.3, 0., 5.], 0.25));
        assert!((yard.edge_distance(13., 14.) - 5.).abs() < 1e-9);
        yard.height = Some([2., 4.]);
        assert!(yard.contains([5., 3., 5.], 0.));
        assert!(!yard.contains([5., 4.2, 5.], 0.));
        assert!(yard.contains([5., 4.2, 5.], 0.25));
        assert_eq!(yard.bounds(), [[0., 0.], [10., 10.]]);
    }

    #[test]
    fn a_walk_target_is_inside_even_when_the_middle_is_not() {
        let square = area("square", &[[0., 0.], [4., 0.], [4., 4.], [0., 4.]]);
        assert_eq!(square.interior_point(), [2., 2.]);
        // A U shape: the middle of its corners lies in the gap between the arms.
        let u = area(
            "u",
            &[
                [0., 0.],
                [9., 0.],
                [9., 9.],
                [7., 9.],
                [7., 1.],
                [2., 1.],
                [2., 9.],
                [0., 9.],
            ],
        );
        let middle = [4.5, 4.75];
        assert!(!u.covers(middle[0], middle[1]));
        let [x, z] = u.interior_point();
        assert!(u.covers(x, z), "{x} {z}");
    }

    #[test]
    fn sets_reject_bad_shapes_names_and_duplicates() {
        let good = area("camp/fire", &[[0., 0.], [1., 0.], [0., 1.]]);
        assert_eq!(validate_gameplay_areas(std::slice::from_ref(&good)), Ok(()));
        assert!(validate_gameplay_areas(&[good.clone(), good.clone()]).is_err());
        for broken in [
            area("Camp Fire", &good.points),
            area("", &good.points),
            area("camp/fire", &good.points[..2]),
            area("camp/fire", &[[0., 0.], [f64::NAN, 0.], [0., 1.]]),
            area("camp/fire", &vec![[0., 0.]; MAX_GAMEPLAY_AREA_POINTS + 1]),
            GameplayArea {
                height: Some([3., 1.]),
                ..good.clone()
            },
        ] {
            assert!(broken.validate().is_err(), "{broken:?}");
        }
    }

    #[test]
    fn the_index_answers_by_space_and_keeps_an_occupant_on_the_edge() {
        let mut far = area("far", &[[100., 100.], [101., 100.], [100., 101.]]);
        far.space = WorldSpaceId(2);
        let index = GameplayAreaIndex::new(Arc::from(vec![
            area("yard", &[[0., 0.], [10., 0.], [10., 10.], [0., 10.]]),
            area("gate", &[[8., 4.], [12., 4.], [12., 6.], [8., 6.]]),
            far,
        ]));
        let names = |position, inside: &[&str]| -> Vec<String> {
            index
                .at(WorldSpaceId(1), position, |name| inside.contains(&name))
                .map(|area| area.name.clone())
                .collect()
        };
        assert_eq!(names([9., 0., 5.], &[]), ["yard", "gate"]);
        assert_eq!(names([10.1, 0., 5.], &[]), ["gate"]);
        assert_eq!(names([10.1, 0., 5.], &["yard"]), ["yard", "gate"]);
        assert_eq!(names([100.2, 0., 100.2], &[]), [""; 0]);
        assert!(index.find("gate").is_some() && index.find("moat").is_none());
    }
}
