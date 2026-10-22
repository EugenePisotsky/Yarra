//! Forms: each object that ends in an impostor, by impostor, with its mesh LODs' primitives
//! and materials. One hidden template scene per mesh LOD lets the trees' material conversion
//! (`super::super::material`) make the same tree materials it makes for every tree scene; the
//! templates stay for as long as the game runs, so their meshes and materials stay loaded. A
//! snowy form's nearest LOD also gives its limbs, whose snow characters shake off
//! (`super::shed`).
use super::super::impostor::{
    ImpostorBatch, ImpostorDescriptor, ImpostorMaterial, ImpostorMaterialInputs, ImpostorMaterials,
    TreeFormSpec, instanced_quad, quad_reach,
};

use super::super::material::{TreeWindMaterial, WindBoundsPending, WindChecked};
use super::super::wind::WindBuffer;
use super::MAX_REPRESENTATIONS;
use super::shed::{Limb, limb_table};
use atmosphere::environment::EnvironmentAssets;

use bevy::{
    asset::RecursiveDependencyLoadState, camera::primitives::Aabb, gltf::GltfAssetLabel, prelude::*,
};

use std::{collections::HashMap, sync::Arc};

/// One primitive of a mesh LOD, with its bounds in object space as the material conversion
/// grew them for wind and turning cards (the bounds its entities were culled with).
#[derive(Clone, Debug)]
pub(super) struct Part {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: Handle<TreeWindMaterial>,
    pub(super) bounds: (Vec3, Vec3),
}

pub(super) struct Form {
    pub(super) spec: Arc<TreeFormSpec>,
    descriptor: Handle<ImpostorDescriptor>,
    /// One template scene per mesh LOD.
    templates: Vec<Entity>,
    /// Each mesh LOD's primitives, once its template converted.
    pub(super) lods: Vec<Option<Vec<Part>>>,
    /// The impostor's material, once its descriptor and textures loaded.
    pub(super) impostor: Option<Handle<ImpostorMaterial>>,
    /// The impostor quad's bounds at unit scale in object space, wind included: centre and
    /// radius. (Each primitive's are its part's.)
    pub(super) impostor_sphere: Vec4,
    /// Its views' texels per metre, which place its hand-off.
    pub(super) texels_per_metre: f32,
    /// Its limbs, for forms holding snow.
    limbs: Option<Arc<Vec<Limb>>>,
    pub(super) ready: bool,
    failed: bool,
}

/// Every form met so far, in the order the pool numbers them.
#[derive(Resource, Default)]
pub(crate) struct TreeForms {
    by_impostor: HashMap<String, u32>,
    pub(super) forms: Vec<Form>,
    /// The impostors' quad, shared.
    pub(super) quad: Handle<Mesh>,
    /// Counts forms becoming ready, so the draw layout follows.
    pub(super) revision: u64,
}

impl TreeForms {
    pub(super) fn index(&self, impostor: &str) -> Option<u32> {
        self.by_impostor.get(impostor).copied()
    }

    /// Whether the form of `impostor` draws: its templates converted and its impostor loaded.
    pub(crate) fn ready(&self, impostor: &str) -> bool {
        self.index(impostor)
            .is_some_and(|index| self.forms[index as usize].ready)
    }

    pub(super) fn len(&self) -> usize {
        self.forms.len()
    }

    pub(super) fn is_ready(&self, form: u32) -> bool {
        self.forms.get(form as usize).is_some_and(|f| f.ready)
    }

    /// The limbs of form `form`, once it is ready, if it holds snow.
    pub(super) fn limbs(&self, form: u32) -> Option<&Arc<Vec<Limb>>> {
        self.forms.get(form as usize)?.limbs.as_ref()
    }
}

/// A form's mesh LOD template, hidden: its meshes are drawn only through the GPU trees.
#[derive(Component)]
pub(super) struct TreeTemplate;

/// Registers the form of every new batch, spawning its templates.
pub(super) fn register(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut forms: ResMut<TreeForms>,
    mut meshes: ResMut<Assets<Mesh>>,
    batches: Query<&ImpostorBatch, Added<ImpostorBatch>>,
) {
    for batch in &batches {
        let spec = &batch.form;

        if forms.by_impostor.contains_key(&spec.impostor) {
            continue;
        }

        if spec.meshes.is_empty() || spec.meshes.len() + 1 > MAX_REPRESENTATIONS {
            warn_once!(
                "GPU trees: {} has {} mesh LODs; 1 to {} are drawn",
                spec.impostor,
                spec.meshes.len(),
                MAX_REPRESENTATIONS - 1
            );
            continue;
        }

        if forms.quad == Handle::default() {
            forms.quad = meshes.add(instanced_quad());
        }

        let templates = spec
            .meshes
            .iter()
            .enumerate()
            .map(|(lod, (uri, _))| {
                commands
                    .spawn((
                        WorldAssetRoot(
                            server.load(GltfAssetLabel::Scene(0).from_asset(uri.clone())),
                        ),
                        Transform::IDENTITY,
                        Visibility::Hidden,
                        TreeTemplate,
                        Name::new(format!("Tree template {} LOD{lod}", spec.impostor)),
                    ))
                    .id()
            })
            .collect();
        let index = forms.forms.len() as u32;
        forms.by_impostor.insert(spec.impostor.clone(), index);
        forms.forms.push(Form {
            spec: spec.clone(),
            descriptor: batch.descriptor.clone(),
            lods: vec![None; spec.meshes.len()],
            templates,
            impostor: None,
            impostor_sphere: Vec4::ZERO,
            texels_per_metre: 0.0,
            limbs: None,
            ready: false,
            failed: false,
        });
    }
}

/// Completes forms whose templates converted and whose impostor loaded.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn harvest(
    mut forms: ResMut<TreeForms>,
    server: Res<AssetServer>,
    descriptors: Res<Assets<ImpostorDescriptor>>,
    children: Query<&Children>,
    parts: Query<(
        &Mesh3d,
        Option<&MeshMaterial3d<TreeWindMaterial>>,
        Has<WindChecked>,
        Option<&Aabb>,
        Has<WindBoundsPending>,
    )>,
    (environment, wind, mut cache, mut materials): (
        Option<Res<EnvironmentAssets>>,
        Option<Res<WindBuffer>>,
        Option<ResMut<ImpostorMaterials>>,
        Option<ResMut<Assets<ImpostorMaterial>>>,
    ),
    (meshes, tree_materials): (Res<Assets<Mesh>>, Res<Assets<TreeWindMaterial>>),
) {
    let forms = forms.as_mut();
    let mut readied = false;

    for form in forms.forms.iter_mut().filter(|f| !f.ready && !f.failed) {
        for (lod, &template) in form.templates.iter().enumerate() {
            if form.lods[lod].is_some() {
                continue;
            }

            // Every mesh of the spawned scene has been seen by the material conversion, and
            // its bounds grown (the template stands at the origin, unturned and unscaled).
            let mut found = Vec::new();
            let mut pending = !children.contains(template);

            for entity in children.iter_descendants(template) {
                let Ok((mesh, material, checked, aabb, growing)) = parts.get(entity) else {
                    continue;
                };

                if !checked || (material.is_some() && (aabb.is_none() || growing)) {
                    pending = true;
                    break;
                }

                match (material, aabb) {
                    (Some(material), Some(aabb)) => found.push(Part {
                        mesh: mesh.0.clone(),
                        material: material.0.clone(),
                        bounds: (
                            Vec3::from(aabb.center - aabb.half_extents),
                            Vec3::from(aabb.center + aabb.half_extents),
                        ),
                    }),
                    _ => warn_once!(
                        "GPU trees: a primitive of {} LOD{lod} has no tree material and is not drawn",
                        form.spec.impostor
                    ),
                }
            }

            if !pending && !found.is_empty() {
                form.lods[lod] = Some(found);
            }
        }

        if let RecursiveDependencyLoadState::Failed(error) =
            server.recursive_dependency_load_state(&form.descriptor)
        {
            warn!(
                "GPU trees: impostor {} failed to load: {error}",
                form.spec.impostor
            );
            form.failed = true;
            continue;
        }

        let (Some(descriptor), true) = (
            descriptors.get(&form.descriptor),
            server.is_loaded_with_dependencies(&form.descriptor),
        ) else {
            continue;
        };

        if form.lods.iter().any(Option::is_none) {
            continue;
        }

        let (Some(environment), Some(wind), Some(cache), Some(materials)) = (
            environment.as_deref(),
            wind.as_deref(),
            cache.as_deref_mut(),
            materials.as_deref_mut(),
        ) else {
            continue;
        };

        form.impostor_sphere = impostor_sphere(descriptor);
        form.texels_per_metre = descriptor.texels_per_metre;
        form.limbs = snowy_limbs(form, &meshes, &tree_materials);
        debug!(
            "TREE_GPU_FORM {} height={:.1} texels_per_metre={:.2} snowy_limbs={} parts={:?} impostor_sphere={:?}",
            form.spec.impostor,
            form.spec.height,
            form.texels_per_metre,
            form.limbs.as_ref().map_or(0, |l| l.len()),
            form.lods
                .iter()
                .map(|parts| parts.iter().flatten().map(|p| p.bounds).collect::<Vec<_>>())
                .collect::<Vec<_>>(),
            form.impostor_sphere,
        );
        form.impostor = Some(cache.get_or_add(
            materials,
            &form.descriptor,
            descriptor,
            &ImpostorMaterialInputs { environment, wind },
        ));
        form.ready = true;
        readied = true;
    }

    if readied {
        forms.revision += 1;
    }
}

/// The limbs of a form's nearest LOD, from its primitives that hold snow.
fn snowy_limbs(
    form: &Form,
    meshes: &Assets<Mesh>,
    materials: &Assets<TreeWindMaterial>,
) -> Option<Arc<Vec<Limb>>> {
    let parts = form.lods.first()?.as_ref()?;
    let snowy = parts.iter().filter_map(|part| {
        let holder = materials.get(&part.material)?.extension.snow_holder()?;
        Some((meshes.get(&part.mesh)?, holder))
    });
    limb_table(snowy).map(Arc::new)
}

/// A sphere round the impostor's quad facing any way, leaning in any wind.
fn impostor_sphere(descriptor: &ImpostorDescriptor) -> Vec4 {
    descriptor
        .centre
        .extend(quad_reach(descriptor) - descriptor.centre.length())
}
