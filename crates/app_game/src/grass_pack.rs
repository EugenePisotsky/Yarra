//! Live grass tuning: while the game runs, edits to the grass pack's types
//! (`assets/packs/grass/baltic.ron`) replace the published looks of the types with the same keys.
//! Which types grow where stays as published; `yarra-world-cook import-grass` publishes that.
use bevy::prelude::*;
use std::{path::PathBuf, time::SystemTime};

/// Seconds between checks of the file's modification time.
const CHECK_INTERVAL: f64 = 0.5;

pub(crate) struct GrassPackPlugin {
    pub path: PathBuf,
}

#[derive(Resource)]
struct Watch {
    path: PathBuf,
    modified: Option<SystemTime>,
    next_check: f64,
}

impl Plugin for GrassPackPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Watch {
            path: self.path.clone(),
            modified: None,
            next_check: 0.0,
        })
        .add_systems(Update, reload.before(engine::WorldVegetationSystems));
    }
}

fn reload(time: Res<Time<Real>>, mut watch: ResMut<Watch>, mut looks: ResMut<engine::GrassLooks>) {
    let now = time.elapsed_secs_f64();
    if now < watch.next_check {
        return;
    }
    watch.next_check = now + CHECK_INTERVAL;
    let Ok(modified) = std::fs::metadata(&watch.path).and_then(|m| m.modified()) else {
        return;
    };
    if watch.modified == Some(modified) {
        return;
    }
    let first = watch.modified.is_none();
    watch.modified = Some(modified);
    let pack = std::fs::read_to_string(&watch.path)
        .map_err(|e| e.to_string())
        .and_then(|text| vegetation::GrassPack::parse(&text));
    match pack {
        Ok(pack) => {
            let catalog = vegetation::VegetationCatalog {
                grass: pack.types,
                surfaces: Vec::new(),
            };
            if let Err(error) = catalog.validate() {
                warn!("Grass pack {}: {error}", watch.path.display());
                return;
            }
            if !first {
                info!("Grass pack reloaded from {}", watch.path.display());
            }
            looks.0 = Some(catalog);
        }
        Err(error) => warn!("Grass pack {}: {error}", watch.path.display()),
    }
}
