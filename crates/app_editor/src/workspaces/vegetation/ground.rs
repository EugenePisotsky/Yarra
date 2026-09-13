//! Uses the published world's terrain surfaces, compressed mip chains and production material.
use super::*;
use terrain_render::{
    PrepareTerrainMaterialContext, TerrainMacroVariation, TerrainMaterial, TerrainSurfaceLayer,
    prepare_terrain_material,
};
use world::{CellCoord, PageDomain, PageKey};

#[derive(Resource)]
pub(super) struct GroundDatabase(pub PathBuf);

#[derive(Resource, Default)]
pub(super) struct StudyGroundAssets {
    textures: Vec<Handle<Image>>,
    pub ready: bool,
    pub error: Option<String>,
}

#[derive(Component)]
pub(super) struct TexturedGround(GroundMode);

#[allow(clippy::too_many_arguments)]
pub(super) fn setup(
    mut commands: Commands,
    database: Res<GroundDatabase>,
    server: Res<AssetServer>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut assets: ResMut<StudyGroundAssets>,
    mut treatment: ResMut<ground_treatment::TreatmentAssets>,
    mut study_materials: ResMut<Assets<ground_treatment::StudyMaterial>>,
) {
    if let Err(error) = treatment.initialize(&mut images) {
        treatment.error = Some(error);
    }
    let prepare = || -> Result<_, String> {
        let reader =
            world_db::RuntimeReader::open_immutable(&database.0).map_err(|e| e.to_string())?;
        let space = reader.manifest().default_world_space;
        let resources = reader
            .read_terrain_resources(PageKey {
                space,
                cell: CellCoord::ZERO,
                domain: PageDomain::TerrainRender,
                lod: 0,
            })
            .map_err(|e| e.to_string())?;
        Ok(resources)
    };
    let resources = match prepare() {
        Ok(resources) => resources,
        Err(error) => {
            assets.error = Some(format!("Study ground: {error}"));
            return;
        }
    };
    let mut plane = Plane3d::default().mesh().size(256.0, 256.0).build();
    if let Err(error) = plane.generate_tangents() {
        assets.error = Some(format!("Study ground tangents: {error}"));
        return;
    }
    let mesh = meshes.add(plane);
    for (mode, key) in [
        (GroundMode::Meadow, "uncut-grass-oilpt20"),
        (GroundMode::Dried, "grass-dried-pjwhw0"),
    ] {
        let Some(surface) = resources.surfaces.iter().find(|s| s.surface.key == key) else {
            assets.error = Some(format!(
                "Study ground surface {key} is missing from the published terrain"
            ));
            return;
        };
        for z in -1..=0 {
            for x in -1..=0 {
                let prepared = prepare_terrain_material(PrepareTerrainMaterialContext {
                    asset_server: &server,
                    images: &mut images,
                    materials: &mut materials,
                    cell: CellCoord { x, z },
                    origin_cell: CellCoord::ZERO,
                    cell_size: 256.0,
                    page_surfaces: &[surface.surface.id],
                    weight_pages: &[],
                    profile: &resources.profile,
                    texture_set: &resources.texture_set,
                    surfaces: &[TerrainSurfaceLayer {
                        surface: surface.surface.clone(),
                        layer: surface.layer,
                    }],
                    macro_variation: TerrainMacroVariation::Enabled,
                });
                let prepared = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        assets.error = Some(error);
                        return;
                    }
                };
                if mode == GroundMode::Meadow && treatment.error.is_none() {
                    // All comparisons share production preparation and lighting. Later
                    // preparation updates are mirrored into every treatment together.
                    let base = materials.get(&prepared.material).unwrap().clone();
                    for test in [
                        GroundMode::OriginalStudy,
                        GroundMode::DarkenedStudy,
                        GroundMode::UnderstoryStudy,
                        GroundMode::CoverageStudy,
                    ] {
                        let material = study_materials.add(ground_treatment::StudyMaterial {
                            base: base.clone(),
                            extension: treatment.extension(test),
                        });
                        treatment
                            .links
                            .push((prepared.material.clone(), material.clone()));
                        commands.spawn((
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material),
                            Transform::from_xyz(
                                x as f32 * 256.0 + 128.0,
                                -0.005,
                                z as f32 * 256.0 + 128.0,
                            ),
                            Visibility::Hidden,
                            RenderLayers::layer(LAYER),
                            TexturedGround(test),
                            Name::new(test.label()),
                        ));
                    }
                }
                commands.spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(prepared.material),
                    Transform::from_xyz(x as f32 * 256.0 + 128.0, -0.005, z as f32 * 256.0 + 128.0),
                    Visibility::Hidden,
                    RenderLayers::layer(LAYER),
                    TexturedGround(mode),
                    Name::new(format!("Study ground · {}", mode.label())),
                ));
            }
        }
    }
    let (color, normal, variation) = resources.texture_set.runtime_uris();
    assets.textures = [color, normal, variation]
        .into_iter()
        .filter_map(|uri| server.get_handle::<Image>(uri.to_owned()))
        .collect();
}

pub(super) fn sync(
    mut state: ResMut<StudyState>,
    server: Res<AssetServer>,
    mut assets: ResMut<StudyGroundAssets>,
    treatment: Res<ground_treatment::TreatmentAssets>,
    mut ground: Query<
        (&mut Visibility, &mut Transform, Option<&TexturedGround>),
        Or<(With<StudyGround>, With<TexturedGround>)>,
    >,
) {
    if state.ground.treatment().is_some()
        && let Some(error) = &treatment.error
    {
        state.error = Some(error.clone());
    }
    let ready = assets.error.is_none()
        && assets.textures.len() == 3
        && assets
            .textures
            .iter()
            .all(|h| server.is_loaded_with_dependencies(h.id()));
    if ready != assets.ready {
        assets.ready = ready;
        state.ready_frames = 0;
    }
    for handle in &assets.textures {
        if let Some(bevy::asset::LoadState::Failed(error)) = server.get_load_state(handle.id()) {
            assets.error = Some(format!("Study ground texture failed: {error}"));
            break;
        }
    }
    for (mut visibility, mut transform, textured) in &mut ground {
        let mode = textured.map_or(GroundMode::Neutral, |g| g.0);
        let desired = if mode == state.ground {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != desired {
            *visibility = desired;
            state.ready_frames = 0;
        }
        if textured.is_none() {
            let scale = (state.field_size / PATCH_SIZE).max(1.0);
            if transform.scale != Vec3::new(scale, 1.0, scale) {
                transform.scale = Vec3::new(scale, 1.0, scale);
                state.ready_frames = 0;
            }
        }
    }
}
