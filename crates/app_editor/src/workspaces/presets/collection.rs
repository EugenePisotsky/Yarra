use crate::navigation::ProjectNavigationStore;
use bevy_egui::egui;
use environment::{AssetCollection, CollectionAsset};

pub(crate) fn editor(
    ui: &mut egui::Ui,
    collection: &mut AssetCollection,
    navigation: &mut ProjectNavigationStore,
    assets: &[world_db::CollectionAssetView],
) {
    ui.label("Spaced scatter");
    ui.small("Stable random positions. Spacing applies within this collection use; separate uses do not compete.");
    ui.add(
        egui::Slider::new(&mut collection.spacing, 0.5..=64.0)
            .logarithmic(true)
            .suffix(" m")
            .text("Minimum spacing"),
    );
    ui.add(egui::Slider::new(&mut collection.density, 0.0..=1.0).text("Default density"));
    ui.horizontal(|ui| {
        ui.label("Default seed");
        ui.add(egui::DragValue::new(&mut collection.seed));
    });
    ui.add(
        egui::Slider::new(&mut collection.max_slope_degrees, 0.0..=85.0)
            .suffix("°")
            .text("Maximum slope"),
    );
    ui.add(
        egui::Slider::new(&mut collection.road_clearance, 0.0..=4.0)
            .suffix(" m")
            .text("Road edge clearance"),
    );
    ui.small("Keeps roots outside the whole road and junction area. Trees stay upright.");
    ui.separator();
    ui.label("Collection assets");
    let mut remove = None;
    for (index, a) in collection.assets.iter_mut().enumerate() {
        ui.push_id(a.asset.0, |ui| {
            let label = assets
                .iter()
                .find(|v| v.id == a.asset)
                .map(|v| v.name.clone())
                .or_else(|| {
                    navigation
                        .palette_records()
                        .iter()
                        .find(|v| v.definition.visual_asset == Some(a.asset))
                        .map(|v| v.definition.display_name.clone())
                })
                .unwrap_or_else(|| {
                    format!(
                        "Asset {:02x}{:02x}{:02x}{:02x}…",
                        a.asset.0[0], a.asset.0[1], a.asset.0[2], a.asset.0[3]
                    )
                });
            ui.horizontal_wrapped(|ui| {
                ui.strong(label);
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
            ui.add(
                egui::DragValue::new(&mut a.weight)
                    .range(0.001..=1000.0)
                    .speed(0.05)
                    .prefix("Weight "),
            );
            ui.horizontal(|ui| {
                ui.label("Scale");
                ui.add(
                    egui::DragValue::new(&mut a.scale_min)
                        .range(0.01..=a.scale_max)
                        .speed(0.01),
                );
                ui.label("to");
                ui.add(
                    egui::DragValue::new(&mut a.scale_max)
                        .range(a.scale_min..=100.0)
                        .speed(0.01),
                );
            });
        });
    }
    if let Some(i) = remove {
        collection.assets.remove(i);
    }
    ui.small("Weights control the mix over many placements, not exact counts in a small patch.");
    ui.collapsing("Add from asset library", |ui| {
        let mut search = navigation.palette_search().to_owned();
        if ui
            .add(egui::TextEdit::singleline(&mut search).hint_text("Find tree, bush, rock…"))
            .changed()
        {
            navigation.search_palette(search);
        }
        egui::ScrollArea::vertical()
            .id_salt("collection_assets")
            .max_height(220.0)
            .show(ui, |ui| {
                for record in navigation.palette_records() {
                    let Some(asset) = record.definition.visual_asset else {
                        continue;
                    };
                    if record.visual_uri.is_none() {
                        continue;
                    }
                    let included = collection.assets.iter().any(|a| a.asset == asset);
                    if ui
                        .add_enabled(
                            !included
                                && collection.assets.len() < environment::MAX_COLLECTION_ASSETS,
                            egui::Button::new(format!(
                                "{}{}",
                                if included { "✓ " } else { "+ " },
                                record.definition.display_name
                            )),
                        )
                        .clicked()
                    {
                        collection.assets.push(CollectionAsset {
                            asset,
                            weight: 1.0,
                            scale_min: 0.85,
                            scale_max: 1.15,
                        });
                    }
                }
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    navigation.palette_has_previous(),
                    egui::Button::new("Previous"),
                )
                .clicked()
            {
                navigation.palette_previous();
            }
            if ui
                .add_enabled(navigation.palette_has_next(), egui::Button::new("Next"))
                .clicked()
            {
                navigation.palette_next();
            }
        });
        ui.weak(navigation.status());
    });
}
