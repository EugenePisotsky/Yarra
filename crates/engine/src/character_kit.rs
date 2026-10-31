//! Character kits: what reshapes a character's face and fits its parts to it again, after
//! MakeHuman's model (`content/characters/scripts/kit.py` writes a kit beside the glb).
//!
//! A face is the base mesh plus each face slider's offsets times its value. A part (eyes, brows,
//! lashes, teeth, hair, garments) follows by its fitting: each of its vertices is three base-mesh
//! vertices, weighted, plus an offset scaled by the base mesh's size along each axis. Every
//! exported vertex names its base-mesh or part vertex in [`SOURCE_VERTEX`], since glTF splits
//! vertices along seams. A character's meshes are built once its scene spawns and again when its
//! [`CharacterAppearance`] changes, never per frame.

use std::collections::{BTreeMap, BTreeSet};

use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    gltf::{GltfMaterialName, GltfMeshName},
    light::NotShadowCaster,
    mesh::{MeshVertexAttribute, VertexAttributeValues, VertexFormat},
    prelude::*,
};
use serde::Deserialize;

use crate::{
    character_face::CharacterAppearance,
    character_hair::{HairStyle, HairStyleLoader, wear_hair},
    character_skin::CharacterSkinPlugin,
};

const VERSION: u32 = 4;

/// Each exported vertex's base-mesh or part vertex. The files name it `_SOURCE_VERTEX`.
pub(crate) const SOURCE_VERTEX: MeshVertexAttribute = MeshVertexAttribute::new(
    "Source_Vertex",
    0x5952_5241_4348_0001,
    VertexFormat::Float32,
);

/// The material of the eye's wet layer (`content/characters/scripts/fit_parts.py`).
const EYE_WET_MATERIAL: &str = "eye_wet";

/// Blended materials, which cast no shadow: the wet layer, and the lashes, too fine for shadow maps.
const UNSHADOWING_MATERIALS: [&str; 2] = [EYE_WET_MATERIAL, "eye_lash"];

/// From a lash strand's middle to this vertex's edge (`shaders/lighting/lash_strand.wesl`).
pub(crate) const STRAND_ACROSS: MeshVertexAttribute = MeshVertexAttribute::new(
    "Strand_Across",
    0x5952_5241_4348_0002,
    VertexFormat::Float32x3,
);

/// The parts made of lash strands: pairs of source vertices across each strand.
const STRAND_PARTS: &str = "eyelashes/";

pub(crate) struct CharacterKitPlugin;

impl Plugin for CharacterKitPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(CharacterSkinPlugin)
            .init_asset::<CharacterKit>()
            .init_asset_loader::<CharacterKitLoader>()
            .init_asset::<HairStyle>()
            .init_asset_loader::<HairStyleLoader>()
            .add_systems(
                Update,
                (
                    bind_kit_meshes,
                    wear_hair,
                    mark_changed_appearances,
                    build_kit_meshes,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                prepare_eye_wet_layers.before(atmosphere::environment::EnvironmentMaterialSystems),
            );
    }
}

/// A face slider: one end's offsets per base-mesh vertex for a value of -1, the other's for +1.
#[derive(Clone, Debug)]
pub struct FaceSlider {
    pub label: String,
    pub group: String,
    /// The values players may give it.
    pub range: [f32; 2],
    /// Sliders sharing a choice exclude each other: a creator offers one of them.
    pub choice: Option<String>,
    /// Hard to see at a face's full framing.
    pub detail: bool,
    low: Option<SliderEnd>,
    high: Option<SliderEnd>,
}

#[derive(Clone, Debug, Deserialize)]
struct SliderEnd {
    vertices: Vec<u32>,
    offsets: Vec<[f32; 3]>,
}

/// How a part follows the base mesh, per part vertex (MakeHuman's MHCLO fitting).
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PartFitting {
    pub(crate) vertices: Vec<[u32; 3]>,
    pub(crate) weights: Vec<[f32; 3]>,
    pub(crate) offsets: Vec<[f32; 3]>,
    /// Per axis, two base-mesh vertices and the distance between them the offsets were made at.
    pub(crate) scales: [(u32, u32, f32); 3],
}

impl PartFitting {
    fn fit(&self, base: &[Vec3]) -> Vec<Vec3> {
        let size = Vec3::from_array(std::array::from_fn(|axis| {
            let (a, b, length) = self.scales[axis];
            (base[a as usize][axis] - base[b as usize][axis]).abs() / length
        }));

        self.vertices
            .iter()
            .zip(&self.weights)
            .zip(&self.offsets)
            .map(|((vertices, weights), offset)| {
                let mut point = Vec3::from_array(*offset) * size;

                for (&vertex, &weight) in vertices.iter().zip(weights) {
                    point += base[vertex as usize] * weight;
                }

                point
            })
            .collect()
    }
}

/// A brow its skin can paint.
#[derive(Clone, Debug, Deserialize)]
pub struct BrowShape {
    pub label: String,
    pub(crate) mask: String,
}

/// A hair style it can wear: its label and folder from the asset root (`character_hair`).
#[derive(Clone, Debug, Deserialize)]
pub struct HairShape {
    pub label: String,
    pub(crate) folder: String,
}

impl HairShape {
    pub(crate) fn hairline_path(&self) -> String {
        format!("{}/hairline.ktx2", self.folder)
    }
}

/// The masks a character's skin paints with (`character_skin`), by their paths from the asset root.
#[derive(Clone, Debug, Deserialize)]
pub struct SkinLayers {
    /// The part of the head's texture the brow masks cover: its corner and size in UVs.
    pub(crate) brow_rect: [f32; 4],
    pub brows: Vec<BrowShape>,
    pub(crate) makeup: String,
    pub hairs: Vec<HairShape>,
}

/// A character's kit: its base mesh as exported, its face sliders, its parts' fittings and its
/// skin's masks.
#[derive(Asset, TypePath, Debug)]
pub struct CharacterKit {
    body: String,
    base: Vec<Vec3>,
    parts: BTreeMap<String, PartFitting>,
    sliders: Vec<FaceSlider>,
    skin_layers: Option<SkinLayers>,
}

impl CharacterKit {
    pub fn sliders(&self) -> &[FaceSlider] {
        &self.sliders
    }

    pub(crate) fn base_len(&self) -> usize {
        self.base.len()
    }

    pub fn skin_layers(&self) -> Option<&SkinLayers> {
        self.skin_layers.as_ref()
    }

    /// The top of the base mesh's head over its feet, metres.
    pub fn height(&self) -> f32 {
        self.base.iter().map(|p| p.y).fold(f32::MIN, f32::max)
    }

    /// The base mesh with the sliders at these values.
    fn shape(&self, values: &BTreeMap<String, f32>) -> Vec<Vec3> {
        let mut shaped = self.base.clone();

        for slider in &self.sliders {
            let Some(&value) = values.get(&slider.label) else {
                continue;
            };
            let value = value.clamp(slider.range[0], slider.range[1]);
            let end = if value < 0.0 {
                &slider.low
            } else {
                &slider.high
            };

            if let Some(end) = end {
                for (&vertex, offset) in end.vertices.iter().zip(&end.offsets) {
                    shaped[vertex as usize] += Vec3::from_array(*offset) * value.abs();
                }
            }
        }

        shaped
    }

    /// Where a mesh's source vertices go on the shaped base mesh: the body's are its vertices,
    /// a part's are fitted to it. None for a mesh the kit does not know.
    fn sources(&self, mesh: &str, shaped: &[Vec3]) -> Option<Vec<Vec3>> {
        if mesh == self.body {
            Some(shaped.to_vec())
        } else {
            self.parts.get(mesh).map(|part| part.fit(shaped))
        }
    }
}

#[derive(Deserialize)]
struct KitFile {
    version: u32,
    body: String,
    base: Vec<[f32; 3]>,
    parts: BTreeMap<String, PartFitting>,
    sliders: Vec<SliderFile>,
    skin_layers: Option<SkinLayers>,
}

#[derive(Deserialize)]
struct SliderFile {
    label: String,
    group: String,
    range: [f32; 2],
    choice: Option<String>,
    #[serde(default)]
    detail: bool,
    low: Option<SliderEnd>,
    high: Option<SliderEnd>,
}

fn parse_kit(bytes: &[u8]) -> Result<CharacterKit, String> {
    let file: KitFile = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;

    if file.version != VERSION {
        return Err(format!("kit version {} (expected {VERSION})", file.version));
    }

    let count = file.base.len();
    let base = file
        .base
        .iter()
        .map(|p| Vec3::from_array(*p))
        .collect::<Vec<_>>();

    if count == 0 || base.iter().any(|p| !p.is_finite()) {
        return Err("the base mesh is empty or not finite".into());
    }

    if file.body.is_empty() || file.parts.contains_key(&file.body) {
        return Err(format!(
            "the body's mesh name {:?} is empty or a part's",
            file.body
        ));
    }

    for (name, part) in &file.parts {
        validate_part(part, count).map_err(|e| format!("part {name:?}: {e}"))?;
    }

    let mut labels = BTreeSet::new();
    let mut sliders = Vec::with_capacity(file.sliders.len());

    for slider in file.sliders {
        if !labels.insert(slider.label.clone()) {
            return Err(format!("slider {:?} appears twice", slider.label));
        }

        validate_slider(&slider, count).map_err(|e| format!("slider {:?}: {e}", slider.label))?;
        sliders.push(FaceSlider {
            label: slider.label,
            group: slider.group,
            range: slider.range,
            choice: slider.choice,
            detail: slider.detail,
            low: slider.low,
            high: slider.high,
        });
    }

    Ok(CharacterKit {
        body: file.body,
        base,
        parts: file.parts,
        sliders,
        skin_layers: file.skin_layers,
    })
}

pub(crate) fn validate_part(part: &PartFitting, count: usize) -> Result<(), String> {
    let vertices = part.vertices.len();

    if part.weights.len() != vertices || part.offsets.len() != vertices {
        return Err("vertices, weights and offsets differ in number".into());
    }

    if part.vertices.iter().flatten().any(|&v| v as usize >= count) {
        return Err("a vertex beyond the base mesh".into());
    }

    let finite = |values: &[[f32; 3]]| values.iter().flatten().all(|v| v.is_finite());

    if !finite(&part.weights) || !finite(&part.offsets) {
        return Err("a weight or offset is not finite".into());
    }

    if part.scales.iter().any(|&(a, b, length)| {
        a as usize >= count || b as usize >= count || !length.is_finite() || length <= 0.0
    }) {
        return Err("a scale's vertex beyond the base mesh or a length not positive".into());
    }

    Ok(())
}

fn validate_slider(slider: &SliderFile, count: usize) -> Result<(), String> {
    let [low, high] = slider.range;

    if !(-1.0..=0.0).contains(&low) || !(0.0..=1.0).contains(&high) || low == high {
        return Err(format!("range {low}..{high} (within -1..1, spanning 0)"));
    }

    for (end, wanted) in [(&slider.low, low < 0.0), (&slider.high, high > 0.0)] {
        match end {
            Some(end) if !wanted => return Err("offsets for an end its range never reaches".into()),
            None if wanted => return Err("no offsets for an end its range reaches".into()),
            Some(end) => validate_end(end, count)?,
            None => {}
        }
    }

    Ok(())
}

fn validate_end(end: &SliderEnd, count: usize) -> Result<(), String> {
    if end.vertices.len() != end.offsets.len() {
        return Err("vertices and offsets differ in number".into());
    }

    if end.vertices.iter().any(|&v| v as usize >= count) {
        return Err("a vertex beyond the base mesh".into());
    }

    if !end.offsets.iter().flatten().all(|v| v.is_finite()) {
        return Err("an offset is not finite".into());
    }

    Ok(())
}

#[derive(Default, TypePath)]
struct CharacterKitLoader;

impl AssetLoader for CharacterKitLoader {
    type Asset = CharacterKit;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _: &(),
        _: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        parse_kit(&bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn extensions(&self) -> &[&str] {
        &["kit.json"]
    }
}

/// On a character visual (a child of its actor) whose meshes its kit builds.
#[derive(Component)]
pub struct CharacterKitRef(pub(crate) Handle<CharacterKit>);

impl CharacterKitRef {
    pub fn kit(&self) -> &Handle<CharacterKit> {
        &self.0
    }
}

/// What a kit mesh is built from.
pub(crate) enum KitMeshSource {
    /// A part of the glb: the mesh as exported, fitted by the kit.
    Part(Handle<Mesh>),
    /// A hair style, which holds its own cards and fit.
    Hair(Handle<HairStyle>),
}

/// A mesh of a kit-built visual: its name in the kit, what it is built from, the one built from it.
#[derive(Component)]
pub(crate) struct KitMesh {
    pub(crate) visual: Entity,
    pub(crate) name: String,
    pub(crate) source: KitMeshSource,
    pub(crate) built: Option<Handle<Mesh>>,
}

/// On a kit-built visual: the slider values its meshes were built for. Colours and paint change no
/// mesh, so they do not rebuild one.
#[derive(Component)]
struct BuiltSliders(BTreeMap<String, f32>);

/// On a kit-built visual whose meshes are to be built again.
#[derive(Component)]
pub(crate) struct KitStale;

/// The wet layer shades what is behind it by its alpha and adds its reflections at full strength.
/// glTF's blending would scale them by the alpha, so there would be no highlight where it is clear:
/// over the iris. Runs before materials are converted for the deferred lighting, which keep this.
fn prepare_eye_wet_layers(
    layers: Query<(&GltfMaterialName, &MeshMaterial3d<StandardMaterial>), Added<GltfMaterialName>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    let Some(mut materials) = materials else {
        return;
    };

    for (name, material) in &layers {
        if name.0 == EYE_WET_MATERIAL
            && let Some(mut material) = materials.get_mut(&material.0)
        {
            material.alpha_mode = AlphaMode::Premultiplied;
        }
    }
}

fn bind_kit_meshes(
    mut commands: Commands,
    meshes: Query<(Entity, &GltfMeshName, &Mesh3d, Option<&GltfMaterialName>), Added<GltfMeshName>>,
    parents: Query<&ChildOf>,
    visuals: Query<(), With<CharacterKitRef>>,
) {
    for (entity, name, mesh, material) in &meshes {
        let Some(visual) = kit_visual(entity, &parents, &visuals) else {
            continue;
        };

        if material.is_some_and(|material| UNSHADOWING_MATERIALS.contains(&material.0.as_str())) {
            commands.entity(entity).insert(NotShadowCaster);
        }

        commands.entity(entity).insert(KitMesh {
            visual,
            name: name.0.clone(),
            source: KitMeshSource::Part(mesh.0.clone()),
            built: None,
        });
        commands.entity(visual).insert(KitStale);
    }
}

fn kit_visual(
    mut entity: Entity,
    parents: &Query<&ChildOf>,
    visuals: &Query<(), With<CharacterKitRef>>,
) -> Option<Entity> {
    loop {
        entity = parents.get(entity).ok()?.parent();

        if visuals.contains(entity) {
            return Some(entity);
        }
    }
}

fn mark_changed_appearances(
    mut commands: Commands,
    actors: Query<(Entity, &CharacterAppearance), Changed<CharacterAppearance>>,
    visuals: Query<(Entity, &ChildOf, Option<&BuiltSliders>), With<CharacterKitRef>>,
) {
    for (actor, appearance) in &actors {
        for (visual, parent, built) in &visuals {
            if parent.parent() == actor && built.is_none_or(|b| b.0 != appearance.sliders) {
                commands.entity(visual).insert(KitStale);
            }
        }
    }
}

/// Skipped in apps without meshes (headless gameplay).
fn build_kit_meshes(
    mut commands: Commands,
    stale: Query<(Entity, &CharacterKitRef, &ChildOf), With<KitStale>>,
    appearances: Query<&CharacterAppearance>,
    kits: Res<Assets<CharacterKit>>,
    styles: Res<Assets<HairStyle>>,
    If(mut meshes): If<ResMut<Assets<Mesh>>>,
    mut kit_meshes: Query<(&mut KitMesh, &mut Mesh3d)>,
) {
    for (visual, kit, parent) in &stale {
        let Some(kit) = kits.get(&kit.0) else {
            continue;
        };
        let values = appearances
            .get(parent.parent())
            .map(|appearance| appearance.sliders.clone())
            .unwrap_or_default();
        let shaped = kit.shape(&values);
        let mut pending = false;
        let mut built = 0;

        for (mut kit_mesh, mut mesh) in &mut kit_meshes {
            if kit_mesh.visual != visual {
                continue;
            }

            let (template, sources) = match &kit_mesh.source {
                KitMeshSource::Part(template) => {
                    let Some(template) = meshes.get(template) else {
                        pending = true;
                        continue;
                    };
                    let Some(sources) = kit.sources(&kit_mesh.name, &shaped) else {
                        warn_once!("character kit has no mesh {:?}", kit_mesh.name);
                        continue;
                    };
                    (template, sources)
                }
                KitMeshSource::Hair(style) => {
                    let Some(style) = styles.get(style) else {
                        pending = true;
                        continue;
                    };
                    (&style.template, style.fit.fit(&shaped))
                }
            };
            let shaped_mesh = reshape(template, &sources).and_then(|mut mesh| {
                if kit_mesh.name.starts_with(STRAND_PARTS) {
                    add_strand_attributes(&mut mesh, &sources)?;
                }

                Ok(mesh)
            });
            let reshaped = match shaped_mesh {
                Ok(reshaped) => reshaped,
                Err(error) => {
                    error!("character mesh {:?}: {error}", kit_mesh.name);
                    continue;
                }
            };

            match &kit_mesh.built {
                Some(handle) => {
                    if meshes.insert(handle, reshaped).is_err() {
                        error!("character mesh {:?} lost its built mesh", kit_mesh.name);
                    }
                }
                None => {
                    let handle = meshes.add(reshaped);
                    mesh.0 = handle.clone();
                    kit_mesh.built = Some(handle);
                }
            }

            built += 1;
        }

        if !pending {
            debug!("built {built} meshes of character visual {visual} from its kit");
            commands
                .entity(visual)
                .remove::<KitStale>()
                .insert(BuiltSliders(values));
        }
    }
}

/// The template with each vertex moved to its source's place, and its normals and tangents
/// turned as the surface turned around that source: unchanged where the surface is.
fn reshape(template: &Mesh, sources: &[Vec3]) -> Result<Mesh, String> {
    let source = template
        .attribute(SOURCE_VERTEX)
        .and_then(|values| match values {
            VertexAttributeValues::Float32(values) => Some(values),
            _ => None,
        })
        .ok_or("no source vertices")?
        .iter()
        .map(|&s| s as usize)
        .collect::<Vec<_>>();
    let before = template
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(VertexAttributeValues::as_float3)
        .ok_or("no positions")?;

    if source.iter().any(|&s| s >= sources.len()) {
        return Err(format!(
            "a source vertex beyond the kit's {}",
            sources.len()
        ));
    }

    let triangles = template
        .indices()
        .ok_or("no indices")?
        .iter()
        .collect::<Vec<_>>();
    let after = source.iter().map(|&s| sources[s]).collect::<Vec<_>>();
    let before = before
        .iter()
        .map(|p| Vec3::from_array(*p))
        .collect::<Vec<_>>();
    let turns = surface_turns(&before, &after, &source, &triangles, sources.len());
    let mut mesh = template.clone();
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        after.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
    );

    if let Some(VertexAttributeValues::Float32x3(normals)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_NORMAL)
    {
        for (normal, &s) in normals.iter_mut().zip(&source) {
            *normal = (turns[s] * Vec3::from_array(*normal)).to_array();
        }
    }

    let normals = mesh
        .attribute(Mesh::ATTRIBUTE_NORMAL)
        .and_then(VertexAttributeValues::as_float3)
        .map(<[_]>::to_vec);

    if let Some(VertexAttributeValues::Float32x4(tangents)) =
        mesh.attribute_mut(Mesh::ATTRIBUTE_TANGENT)
    {
        for (index, (tangent, &s)) in tangents.iter_mut().zip(&source).enumerate() {
            let turned = turns[s] * Vec3::new(tangent[0], tangent[1], tangent[2]);
            let normal = normals
                .as_ref()
                .map_or(Vec3::Z, |n| Vec3::from_array(n[index]));
            *tangent = usable_tangent(turned, normal).extend(tangent[3]).to_array();
        }
    }

    Ok(mesh)
}

/// A vertex whose UV triangles have no area (a face layout folds the lids' and lips' insides away)
/// gets no tangent from the mesh's tangent space, and a normal map's shader would divide by its
/// zero length: any direction across the normal serves there, with nothing to show.
fn usable_tangent(tangent: Vec3, normal: Vec3) -> Vec3 {
    if tangent.is_finite() && tangent.length_squared() > 1e-8 {
        tangent
    } else {
        normal.any_orthonormal_vector()
    }
}

/// What a strand's vertex shader reads: each vertex's way across its strand, its source's partner
/// being the other edge, and a colour for the shader to fade it by.
fn add_strand_attributes(mesh: &mut Mesh, sources: &[Vec3]) -> Result<(), String> {
    let VertexAttributeValues::Float32(source) =
        mesh.attribute(SOURCE_VERTEX).ok_or("no source vertices")?
    else {
        return Err("source vertices are not floats".into());
    };
    let across = source
        .iter()
        .map(|&s| {
            let s = s as usize;
            let partner = sources
                .get(s ^ 1)
                .ok_or("a strand vertex without its partner")?;
            Ok(((sources[s] - *partner) / 2.0).to_array())
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let count = across.len();
    mesh.insert_attribute(STRAND_ACROSS, across);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0f32; 4]; count]);
    Ok(())
}

/// Per source vertex, the turn from its smooth normal before to its smooth normal after: area
/// weighted over the triangles around it, its split copies counted as one vertex.
fn surface_turns(
    before: &[Vec3],
    after: &[Vec3],
    source: &[usize],
    triangles: &[usize],
    count: usize,
) -> Vec<Quat> {
    let normals = |positions: &[Vec3]| {
        let mut sums = vec![Vec3::ZERO; count];

        for triangle in triangles.as_chunks::<3>().0 {
            let [a, b, c] = *triangle;
            let face = (positions[b] - positions[a]).cross(positions[c] - positions[a]);

            for &corner in triangle {
                sums[source[corner]] += face;
            }
        }

        sums
    };
    let (old, new) = (normals(before), normals(after));

    old.iter()
        .zip(&new)
        .map(
            |(old, new)| match (old.try_normalize(), new.try_normalize()) {
                (Some(old), Some(new)) => Quat::from_rotation_arc(old, new),
                _ => Quat::IDENTITY,
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        asset::RenderAssetUsages,
        mesh::{Indices, PrimitiveTopology},
    };

    const KIT: &str = r#"{
        "version": 4,
        "body": "body",
        "base": [[0,0,0], [1,0,0], [0,1,0], [0,0,1]],
        "parts": {"hat": {
            "vertices": [[0,1,2]],
            "weights": [[0.5,0.25,0.25]],
            "offsets": [[0,0,10]],
            "scales": [[0,1,10], [0,2,10], [0,3,20]]
        }},
        "sliders": [
            {"label": "lift", "group": "g", "range": [-0.5, 1.0],
             "low": {"vertices": [2], "offsets": [[0,-1,0]]},
             "high": {"vertices": [2], "offsets": [[0,2,0]]}},
            {"label": "point", "group": "g", "range": [0.0, 0.7], "choice": "shape", "detail": true,
             "high": {"vertices": [3], "offsets": [[0,0,1]]}}
        ]
    }"#;

    fn kit() -> CharacterKit {
        parse_kit(KIT.as_bytes()).expect("the test kit is valid")
    }

    fn values(pairs: &[(&str, f32)]) -> BTreeMap<String, f32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn sliders_move_their_vertices_by_the_end_their_sign_picks() {
        let kit = kit();
        let raised = kit.shape(&values(&[("lift", 0.5)]));
        assert_eq!(raised[2], Vec3::new(0.0, 2.0, 0.0));
        assert_eq!(raised[0], Vec3::ZERO);

        let lowered = kit.shape(&values(&[("lift", -0.5)]));
        assert_eq!(lowered[2], Vec3::new(0.0, 0.5, 0.0));
    }

    #[test]
    fn slider_values_are_held_to_their_range() {
        let kit = kit();
        let held = kit.shape(&values(&[("lift", -1.0), ("point", 1.0), ("unknown", 1.0)]));
        assert_eq!(held[2], Vec3::new(0.0, 0.5, 0.0));
        assert!((held[3] - Vec3::new(0.0, 0.0, 1.7)).length() < 1e-6);
    }

    #[test]
    fn parts_fit_to_weighted_vertices_plus_offsets_scaled_by_the_base() {
        let kit = kit();
        let base = kit.shape(&BTreeMap::new());

        // Sizes: x 1/10, y 1/10, z 1/20; offset z 10 scales to 0.5.
        let hat = kit.sources("hat", &base).expect("the kit fits its hat");
        assert!((hat[0] - Vec3::new(0.25, 0.25, 0.5)).length() < 1e-6);

        // Stretching the base along z stretches the offset with it.
        let stretched = kit.shape(&values(&[("point", 0.5)]));
        let hat = kit
            .sources("hat", &stretched)
            .expect("the kit fits its hat");
        assert!((hat[0].z - 0.75).abs() < 1e-6);
        assert!(kit.sources("cape", &base).is_none());
    }

    #[test]
    fn kits_with_out_of_range_or_missing_data_are_refused() {
        let beyond = KIT.replace(
            r#""vertices": [2], "offsets": [[0,2,0]]"#,
            r#""vertices": [9], "offsets": [[0,2,0]]"#,
        );
        assert!(parse_kit(beyond.as_bytes()).is_err());

        let no_low = KIT.replace(r#""low": {"vertices": [2], "offsets": [[0,-1,0]]},"#, "");
        assert!(parse_kit(no_low.as_bytes()).is_err());

        let version = KIT.replace(r#""version": 4"#, r#""version": 3"#);
        assert!(parse_kit(version.as_bytes()).is_err());
    }

    fn template() -> Mesh {
        // Two split copies of source 0 (a seam), one each of sources 1 and 2.
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(
            Mesh::ATTRIBUTE_POSITION,
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
            ],
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4])
        .with_inserted_attribute(SOURCE_VERTEX, vec![0.0f32, 1.0, 2.0, 0.0])
        .with_inserted_indices(Indices::U32(vec![0, 1, 2]))
    }

    #[test]
    fn reshaping_moves_split_copies_together_and_keeps_unturned_normals() {
        let moved = [Vec3::new(0.0, 0.0, 0.5), Vec3::X, Vec3::Y];
        let mesh = reshape(&template(), &moved).expect("the template reshapes");
        let positions = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(VertexAttributeValues::as_float3)
            .expect("positions");
        assert_eq!(positions[0], [0.0, 0.0, 0.5]);
        assert_eq!(positions[3], [0.0, 0.0, 0.5]);

        let still = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let mesh = reshape(&template(), &still).expect("the template reshapes");
        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(VertexAttributeValues::as_float3)
            .expect("normals");
        assert!(normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
    }

    #[test]
    fn a_strands_vertices_point_away_from_each_other() {
        let mut mesh = template();
        let sources = [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::Y, Vec3::Y];
        add_strand_attributes(&mut mesh, &sources).expect("the strands have partners");
        let Some(VertexAttributeValues::Float32x3(across)) = mesh.attribute(STRAND_ACROSS) else {
            panic!("no way across");
        };
        assert_eq!(across[0], [-0.5, 0.0, 0.0]);
        assert_eq!(across[1], [0.5, 0.0, 0.0]);
    }

    #[test]
    fn a_vertex_without_a_tangent_gets_one_across_its_normal() {
        let made = usable_tangent(Vec3::ZERO, Vec3::Y);
        assert!((made.length() - 1.0).abs() < 1e-5 && made.dot(Vec3::Y).abs() < 1e-5);
        assert_eq!(usable_tangent(Vec3::X, Vec3::Y), Vec3::X);
    }

    #[test]
    fn reshaping_turns_normals_with_the_surface() {
        // Tipping the triangle up about the x axis tips its normal the same way.
        let tipped = [Vec3::ZERO, Vec3::X, Vec3::new(0.0, 0.0, 1.0)];
        let mesh = reshape(&template(), &tipped).expect("the template reshapes");
        let normals = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .and_then(VertexAttributeValues::as_float3)
            .expect("normals");
        assert!((Vec3::from_array(normals[0]) - Vec3::NEG_Y).length() < 1e-5);
    }
}
