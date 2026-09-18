//! cargo run --offline -p yarra-environment-compile --example cart_track > cart-track.svg
//! Options: --breakup 0..1 --spacing metres --track-width metres --road-width metres
//!          --center-retention 0..1 --shoulder-retention 0..1 --seed integer --straight
#[path = "../tests/support/roads.rs"]
mod road_support;
#[path = "../tests/support/mod.rs"]
mod support;
use road_support::*;
use std::fmt::Write;
use support::*;
use yarra_environment_compile::{CompilePlan, CompiledCell};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (definition, library, plants, coverage, mut roads) = cart_tracks();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--straight" {
            straight(&mut roads);
            continue;
        }
        let value = args.next().ok_or("expected an option value")?;
        match arg.as_str() {
            "--breakup" => roads.profiles[0].breakup = value.parse()?,
            "--spacing" => roads.profiles[0].track_spacing = value.parse()?,
            "--track-width" => roads.profiles[0].track_width = value.parse()?,
            "--road-width" => {
                let width = value.parse()?;
                roads.spans[0].start.width = width;
                roads.spans[0].end.width = width;
            }
            "--center-retention" => roads.profiles[0].center_retention = value.parse()?,
            "--shoulder-retention" => roads.profiles[0].shoulder_retention = value.parse()?,
            "--seed" => roads.roads[0].seed = value.parse()?,
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    let plan = CompilePlan::new(&definition, &plants, &library, Default::default())?;
    let baseline = plan.compile_cells(&CELLS, &coverage)?;
    let cells = plan.compile_cells_with_roads(&CELLS, &coverage, &roads, Default::default())?;
    let placements = roots(&plan, &cells)?;
    let before = roots(&plan, &baseline)?;
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="1050" viewBox="0 0 1280 1050">
<rect width="1280" height="1050" fill="#101a20"/><g font-family="system-ui,sans-serif" fill="#e8efe9">
<text x="40" y="48" font-size="28" font-weight="650">Cart track · source and compiler fixture</text>
<text x="40" y="78" font-size="15" fill="#a9bcb5">Two 8 m cells · fixed seed · separate surface and vegetation effects · existing meadow roots retained</text>
<text x="40" y="120" font-size="18">Compiled ground + CPU reference grass roots</text>
<text x="40" y="685" font-size="18">Ground influence</text><text x="660" y="685" font-size="18">Remaining meadow coverage</text>
"##,
    );
    paint(&mut svg, &cells, 40.0, 138.0, 1200.0, false)?;
    for root in &placements {
        let x = 40.0 + f64::from(root.root[0]) * 75.0;
        let y = 138.0 + f64::from(root.root[2]) * 60.0;
        writeln!(
            svg,
            r##"<path d="M {x:.2} {y:.2} l -1.2 -3.2" stroke="#a3c775" stroke-width="1.1" opacity=".85"/>"##
        )?;
    }
    writeln!(
        svg,
        r##"<path d="M 640 138 V 618" stroke="#fff" stroke-dasharray="6 6" opacity=".65"/>
<text x="40" y="645" font-size="14" fill="#a9bcb5">{} / {} reference roots retained. The pale marks are root locations, not rendered blades or a performance estimate.</text>"##,
        placements.len(),
        before.len()
    )?;
    paint(&mut svg, &cells, 40.0, 703.0, 580.0, false)?;
    paint(&mut svg, &cells, 660.0, 703.0, 580.0, true)?;
    for x in [330, 950] {
        writeln!(
            svg,
            r##"<path d="M {x} 703 V 935" stroke="#fff" stroke-dasharray="5 5" opacity=".6"/>"##
        )?;
    }
    let p = &roads.profiles[0];
    writeln!(
        svg,
        r##"<text x="40" y="969" font-size="15">Wheel spacing {:.2} m · track width {:.2} m · corridor {:.1}–{:.1} m · breakup {:.0}% · center retention {:.0}%</text>
<text x="40" y="997" font-size="14" fill="#a9bcb5">Surface: brown = road, green = meadow. Coverage: dark = cleared, bright = retained. Dashes mark the cell boundary.</text>
<text x="40" y="1025" font-size="14" fill="#a9bcb5">Compiler proof only. Road persistence, editor handles, paving joints, elevation and navigation are not implemented.</text></g></svg>"##,
        p.track_spacing,
        p.track_width,
        roads.spans[0].start.width,
        roads.spans[0].end.width,
        p.breakup * 100.0,
        p.center_retention * 100.0
    )?;
    println!("{svg}");
    eprintln!(
        "Compiled two cells; retained {} of {} CPU reference roots; {} stable population bindings.",
        placements.len(),
        before.len(),
        plan.bindings().len()
    );
    Ok(())
}
fn roots(
    plan: &CompilePlan,
    cells: &[CompiledCell],
) -> Result<Vec<vegetation_compile::DebugPlacement>, vegetation_compile::PlacementError> {
    vegetation_compile::generate_scene_debug_placements(&vegetation::VegetationScene {
        catalog: plan.catalog().clone(),
        pages: cells
            .iter()
            .map(|c| vegetation::VegetationFieldPage {
                origin_xz: [c.cell.x as f32 * 8.0, c.cell.z as f32 * 8.0],
                size: 8.0,
                surface: vegetation::VegetationSurfaceField::flat(2, 0.0, [0.0, 1.0, 0.0]),
                fields: c.vegetation.fields.clone(),
            })
            .collect(),
    })
}
fn paint(
    svg: &mut String,
    cells: &[CompiledCell],
    left: f64,
    top: f64,
    width: f64,
    grass: bool,
) -> Result<(), std::fmt::Error> {
    let step = width / 128.0;
    let height_step = step * 0.8;
    for (i, c) in cells.iter().enumerate() {
        for z in 0..64 {
            for x in 0..64 {
                // Ground has endpoint samples; vegetation has cell-center samples.
                // Bilinear ground at each center aligns the two displayed fields.
                let soil = [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)]
                    .into_iter()
                    .map(|(gx, gz)| {
                        ground_weights(&c.ground, gx, gz)
                            .into_iter()
                            .find(|(surface, _)| *surface == SOIL)
                            .map_or(0.0, |(_, weight)| f64::from(weight))
                    })
                    .sum::<f64>()
                    / (4.0 * 255.0);
                let cover = f64::from(
                    c.vegetation
                        .fields
                        .iter()
                        .map(|f| f.coverage[z * 64 + x])
                        .max()
                        .unwrap_or(0),
                ) / 255.0;
                let color = if grass {
                    mix([21, 35, 39], [168, 212, 117], cover)
                } else {
                    mix([65, 94, 55], [165, 135, 94], soil)
                };
                writeln!(
                    svg,
                    r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="rgb({},{},{})"/>"#,
                    left + (i * 64 + x) as f64 * step,
                    top + z as f64 * height_step,
                    step + 0.05,
                    height_step + 0.05,
                    color[0],
                    color[1],
                    color[2]
                )?;
            }
        }
    }
    Ok(())
}
fn mix(a: [u8; 3], b: [u8; 3], w: f64) -> [u8; 3] {
    std::array::from_fn(|i| (f64::from(a[i]) * (1.0 - w) + f64::from(b[i]) * w).round() as u8)
}
