//! Offline, inspectable output from the same public compiler API used by integration tests.
//! cargo run -p yarra-environment-compile --example layered_meadow > meadow.svg

#[path = "../tests/support/mod.rs"]
mod support;

use std::{collections::BTreeMap, fmt::Write};

use support::*;
use yarra_environment_compile::CompilePlan;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (definition, library, plants, source) = fixture();
    let plan = CompilePlan::new(&definition, &plants, &library, profile())?;
    let cells = plan.compile_cells(&CELLS, &source)?;
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1120" height="480" viewBox="0 0 1120 480">
<rect width="1120" height="480" fill="#141c24"/>
<g font-family="system-ui, sans-serif" fill="#ecf0f3">
<text x="32" y="42" font-size="24" font-weight="600">Environment compiler: two meadow layers + a clearing</text>
<text x="32" y="72" font-size="14" fill="#a9b9c6">Two adjacent 8 m cells. Each square shows one compiled sample. Dashed lines mark the cell boundary.</text>
<text x="32" y="111" font-size="18">Ground surface weights</text>
<text x="576" y="111" font-size="18">Vegetation density estimate</text>
"##,
    );
    for (cell_index, cell) in cells.iter().enumerate() {
        for z in 0..16 {
            for x in 0..16 {
                let weights = ground_weights(&cell.ground, x, z);
                let green = weights
                    .iter()
                    .find(|(id, _)| *id == GREEN_SOIL)
                    .map_or(0.0, |(_, v)| f64::from(*v) / 255.0);
                let ground = mix([151, 111, 67], [66, 118, 72], green);
                // Competition spends the strongest requested density within each group.
                // Spatial clump thinning, geometry LOD and render appearance are not evaluated.
                let mut groups = BTreeMap::<u16, f64>::new();
                let mut density = 0.0;
                for field in &cell.vegetation.fields {
                    let population = plan.catalog().population(field.population).unwrap();
                    let value = f64::from(population.density_per_square_meter)
                        * f64::from(field.coverage[z * 16 + x])
                        / 255.0;
                    if let Some(group) = population.competition_group {
                        let requested = groups.entry(group).or_default();
                        *requested = requested.max(value);
                    } else {
                        density += value;
                    }
                }
                density += groups.values().sum::<f64>();
                let grass = mix(
                    [25, 37, 45],
                    [166, 214, 109],
                    (density / 24.0).clamp(0.0, 1.0),
                );
                for (left, color) in [(32, ground), (576, grass)] {
                    writeln!(
                        svg,
                        r#"<rect x="{}" y="{}" width="16" height="16" fill="rgb({},{},{})"/>"#,
                        left + (cell_index * 16 + x) * 16,
                        128 + z * 16,
                        color[0],
                        color[1],
                        color[2]
                    )?;
                }
            }
        }
    }
    for left in [32, 576] {
        writeln!(
            svg,
            r##"<rect x="{left}" y="128" width="512" height="256" fill="none" stroke="#a9b9c6"/>
<path d="M {} 128 V 384" stroke="#ffffff" stroke-dasharray="5 5" opacity="0.7"/>
<text x="{left}" y="407" font-size="13" fill="#a9b9c6">0 m</text>
<text x="{}" y="407" font-size="13" fill="#a9b9c6">8 m</text>
<text x="{}" y="407" font-size="13" fill="#a9b9c6">16 m</text>"##,
            left + 256,
            left + 244,
            left + 480
        )?;
    }
    svg.push_str(r##"<text x="32" y="440" font-size="14" fill="#a9b9c6">Brown: dry ground. Green: meadow ground. The clearing changes both ground and vegetation.</text>
<text x="32" y="462" font-size="14" fill="#a9b9c6">Dark: no plants. Bright: up to 24 roots/m² before clump thinning. The upper-left hole is unpainted coverage.</text>
</g></svg>"##);
    println!("{svg}");
    eprintln!(
        "Compiled {} cells, {} population bindings; ground and vegetation came from one layer stack.",
        cells.len(),
        plan.bindings().len()
    );
    Ok(())
}

fn mix(a: [u8; 3], b: [u8; 3], weight: f64) -> [u8; 3] {
    std::array::from_fn(|i| {
        (f64::from(a[i]) * (1.0 - weight) + f64::from(b[i]) * weight).round() as u8
    })
}
