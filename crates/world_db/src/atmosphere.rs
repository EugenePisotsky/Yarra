use crate::{ProjectWriter, WorldDbError};
use rusqlite::params;
use std::collections::HashSet;
use world::{WorldSpaceId, atmosphere::AtmosphereProfile};

#[derive(Debug, Clone)]
pub struct AtmosphereWrite {
    pub space: WorldSpaceId,
    pub expected_revision: i64,
    pub profile: AtmosphereProfile,
}

#[derive(Debug, Clone)]
pub enum AtmosphereWriteResult {
    Committed(Vec<(WorldSpaceId, i64, AtmosphereProfile)>),
    Conflict {
        space: WorldSpaceId,
        revision: i64,
        actual: AtmosphereProfile,
    },
}

// Binary compatibility: keep the schema-24/20 core unchanged and append a tagged extension.
// Older readers reject the trailing bytes rather than misreading a newer profile.
#[derive(serde::Serialize, serde::Deserialize)]
struct CoreProfile {
    outdoor: bool,
    initial_phase: f32,
    day_seconds: f32,
    azimuth_degrees: f32,
    maximum_elevation_degrees: f32,
    sun_diameter_degrees: f32,
    exposure_ev100: f32,
    bloom_intensity: f32,
    visibility_metres: f32,
    haze_srgb: [f32; 3],
    molecular_density: f32,
    phases: [world::atmosphere::LightingPhase; 4],
    night: world::atmosphere::NightLighting,
}
impl From<&AtmosphereProfile> for CoreProfile {
    fn from(p: &AtmosphereProfile) -> Self {
        Self {
            outdoor: p.outdoor,
            initial_phase: p.initial_phase,
            day_seconds: p.day_seconds,
            azimuth_degrees: p.azimuth_degrees,
            maximum_elevation_degrees: p.maximum_elevation_degrees,
            sun_diameter_degrees: p.sun_diameter_degrees,
            exposure_ev100: p.exposure_ev100,
            bloom_intensity: p.bloom_intensity,
            visibility_metres: p.visibility_metres,
            haze_srgb: p.haze_srgb,
            molecular_density: p.molecular_density,
            phases: p.phases.clone(),
            night: p.night.clone(),
        }
    }
}
impl From<CoreProfile> for AtmosphereProfile {
    fn from(p: CoreProfile) -> Self {
        Self {
            outdoor: p.outdoor,
            initial_phase: p.initial_phase,
            day_seconds: p.day_seconds,
            azimuth_degrees: p.azimuth_degrees,
            maximum_elevation_degrees: p.maximum_elevation_degrees,
            sun_diameter_degrees: p.sun_diameter_degrees,
            exposure_ev100: p.exposure_ev100,
            bloom_intensity: p.bloom_intensity,
            visibility_metres: p.visibility_metres,
            haze_srgb: p.haze_srgb,
            molecular_density: p.molecular_density,
            phases: p.phases,
            night: p.night,
            clouds: Default::default(),
            weather: Default::default(),
        }
    }
}
const CLOUD_EXTENSION: &[u8; 4] = b"CLD1";
const WEATHER_EXTENSION: &[u8; 4] = b"WTH1";
pub(super) fn encode(profile: &AtmosphereProfile) -> Result<Vec<u8>, WorldDbError> {
    profile
        .validate()
        .map_err(|e| WorldDbError::Cook(e.into()))?;
    let mut bytes =
        bincode::serde::encode_to_vec(CoreProfile::from(profile), bincode::config::standard())?;
    // Preserve the exact checkpoint encoding when the extension is unused.
    if profile.clouds != Default::default() {
        bytes.extend_from_slice(CLOUD_EXTENSION);
        bytes.extend(bincode::serde::encode_to_vec(
            &profile.clouds,
            bincode::config::standard(),
        )?);
    }
    if profile.weather != Default::default() {
        bytes.extend_from_slice(WEATHER_EXTENSION);
        bytes.extend(bincode::serde::encode_to_vec(
            &profile.weather,
            bincode::config::standard(),
        )?);
    }
    if bytes.len() > 4096 {
        return Err(WorldDbError::Cook("atmosphere exceeds 4096 bytes".into()));
    }
    Ok(bytes)
}
pub(super) fn decode(bytes: &[u8]) -> Result<AtmosphereProfile, WorldDbError> {
    if bytes.len() > 4096 {
        return Err(WorldDbError::Cook("atmosphere exceeds 4096 bytes".into()));
    }
    let (core, consumed): (CoreProfile, _) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard().with_limit::<4096>())?;
    let mut p = AtmosphereProfile::from(core);
    // Tagged extensions follow the core in a fixed order, each at most once.
    let mut rest = &bytes[consumed..];
    let limit = bincode::config::standard().with_limit::<4096>();
    if let Some(extension) = rest.strip_prefix(CLOUD_EXTENSION) {
        let (clouds, used) = bincode::serde::decode_from_slice(extension, limit)?;
        p.clouds = clouds;
        rest = &extension[used..];
    }
    if let Some(extension) = rest.strip_prefix(WEATHER_EXTENSION) {
        let (weather, used) = bincode::serde::decode_from_slice(extension, limit)?;
        p.weather = weather;
        rest = &extension[used..];
    }
    if !rest.is_empty() {
        return Err(WorldDbError::Cook(
            "unknown or trailing atmosphere extension".into(),
        ));
    }
    p.validate().map_err(|e| WorldDbError::Cook(e.into()))?;
    Ok(p)
}
impl ProjectWriter {
    /// All worlds in the bounded batch commit together, or no source record changes.
    pub fn write_atmospheres(
        &mut self,
        writes: &[AtmosphereWrite],
    ) -> Result<AtmosphereWriteResult, WorldDbError> {
        if writes.is_empty() || writes.len() > 32 {
            return Err(WorldDbError::Cook(
                "invalid atmosphere transaction size".into(),
            ));
        }
        let mut ids = HashSet::new();
        let payloads = writes
            .iter()
            .map(|w| {
                if !ids.insert(w.space)
                    || w.expected_revision <= 0
                    || w.expected_revision == i64::MAX
                {
                    return Err(WorldDbError::Cook(
                        "invalid atmosphere revision or duplicate world".into(),
                    ));
                }
                encode(&w.profile)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for w in writes {
            let (revision, bytes): (i64, Vec<u8>) = tx.query_row(
                "SELECT atmosphere_revision, atmosphere FROM world_spaces WHERE id=?1",
                [w.space.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if revision != w.expected_revision {
                return Ok(AtmosphereWriteResult::Conflict {
                    space: w.space,
                    revision,
                    actual: decode(&bytes)?,
                });
            }
        }
        for (w, payload) in writes.iter().zip(payloads) {
            tx.execute(
                "UPDATE world_spaces SET atmosphere=?1, atmosphere_revision=?2 WHERE id=?3",
                params![payload, w.expected_revision + 1, w.space.0],
            )?;
        }
        tx.commit()?;
        Ok(AtmosphereWriteResult::Committed(
            writes
                .iter()
                .map(|w| (w.space, w.expected_revision + 1, w.profile.clone()))
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog::query_world_spaces, schema};
    use rusqlite::Connection;
    #[test]
    fn checkpoint_and_tagged_cloud_profiles_round_trip_without_accepting_corruption() {
        let p = AtmosphereProfile::default();
        let legacy =
            bincode::serde::encode_to_vec(CoreProfile::from(&p), bincode::config::standard())
                .unwrap();
        assert_eq!(decode(&legacy).unwrap(), p);
        assert_eq!(encode(&p).unwrap(), legacy);
        let mut clouds = p.clone();
        clouds.clouds = world::clouds::CloudSettings::overcast();
        clouds.clouds.seed = u32::MAX;
        let bytes = encode(&clouds).unwrap();
        assert_eq!(decode(&bytes).unwrap(), clouds);
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        let mut unknown = bytes.clone();
        unknown[legacy.len() + 3] = b'2';
        assert!(decode(&unknown).is_err());
        let mut trailing = bytes;
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        clouds.clouds.density = f32::INFINITY;
        assert!(encode(&clouds).is_err());
    }
    #[test]
    fn authored_weather_round_trips_alone_and_after_clouds() {
        let mut weather = AtmosphereProfile::default();
        weather.weather.presets[3].cloud_coverage = 0.7;
        weather.weather.schedule.transition_seconds = [120., 300.];
        let bytes = encode(&weather).unwrap();
        assert_eq!(decode(&bytes).unwrap(), weather);
        let mut both = weather.clone();
        both.clouds = world::clouds::CloudSettings::overcast();
        let bytes = encode(&both).unwrap();
        assert!(bytes.len() <= 4096);
        assert_eq!(decode(&bytes).unwrap(), both);
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
        let mut invalid = both;
        invalid.weather.presets[0].precipitation = 2.;
        assert!(encode(&invalid).is_err());
    }
    #[test]
    fn conflict_rolls_back_the_entire_batch_and_round_trips_colors_and_clouds() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(schema::PROJECT_SCHEMA).unwrap();
        let p = AtmosphereProfile::default();
        for id in [1, 2] {
            connection
                .execute(
                    "INSERT INTO world_spaces VALUES (?1,?2,32,0,10,?3,1)",
                    params![id, format!("world{id}"), encode(&p).unwrap()],
                )
                .unwrap();
        }
        let mut writer = ProjectWriter { connection };
        let mut changed = p.clone();
        changed.phases[1].sun_srgb = [0.8, 0.5, 0.2];
        changed.clouds = world::clouds::CloudSettings::scattered();
        let writes = [
            AtmosphereWrite {
                space: WorldSpaceId(1),
                expected_revision: 1,
                profile: changed.clone(),
            },
            AtmosphereWrite {
                space: WorldSpaceId(2),
                expected_revision: 2,
                profile: changed.clone(),
            },
        ];
        assert!(matches!(
            writer.write_atmospheres(&writes).unwrap(),
            AtmosphereWriteResult::Conflict { .. }
        ));
        assert_eq!(
            query_world_spaces(&writer.connection).unwrap()[0].atmosphere,
            p
        );
        assert!(matches!(
            writer.write_atmospheres(&writes[..1]).unwrap(),
            AtmosphereWriteResult::Committed(_)
        ));
        assert_eq!(
            query_world_spaces(&writer.connection).unwrap()[0].atmosphere,
            changed
        );
        let mut bytes = encode(&p).unwrap();
        bytes.push(0);
        assert!(decode(&bytes).is_err());
    }
}
