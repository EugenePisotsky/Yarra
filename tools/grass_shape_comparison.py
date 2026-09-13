#!/usr/bin/env python3
"""Capture fixed grass shapes in the native editor and build a local linked comparison.

The comparison deliberately uses high topology for every retained root. Its counters are
inspection cost, not a proposal to increase the production budget. No catalog is published.
"""
import argparse
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
VIEWS = [
    ("current", "Current shape", "Production morph on high topology; preserves retained roots and density fade."),
    ("full", "Full shape", "Same retained roots and density fade; shape morph fixed at 1."),
    ("low", "Low shape", "Same retained roots and density fade; shape morph fixed at 0."),
    ("morph", "Morph weight", "Green: full shape. Yellow: intermediate. Red: low shape."),
    ("cause", "Simplification cause", "Green: full shape. Orange: budget limited. Blue: screen-size limited."),
    ("full-no-opening", "Full shape / opening off", "Full shape with view opening disabled; all authored settings remain intact."),
    ("production", "Production draw", "Actual production topology bins; use to check the current-shape comparison."),
]


def array(text, field):
    match = re.search(rf"\b{field}:\s*\[([^]]+)\]", text)
    if match is None:
        raise ValueError(f"Missing {field} in diagnostics")
    return [int(v) for v in re.findall(r"\d+", match[1])]


def scalar(text, field):
    return int(re.search(rf"\b{field}:\s*(\d+)", text)[1])


def normalize_study(text):
    # These are the only permitted differences in a captured comparison.
    return re.sub(r"^\s*(shape_inspection|inspection_disable_opening|build):.*$", "", text, flags=re.M)


def report(folder, prefix=""):
    views = []
    baseline = None
    count = None
    for name, label, description in VIEWS:
        capture = folder / (prefix + name)
        study = normalize_study((capture / "study.ron").read_text())
        if baseline is None:
            baseline = study
        if study != baseline:
            raise ValueError(f"{capture}: camera/catalog/wind/stage differ from current shape")
        text = (capture / "diagnostics.txt").read_text()
        emitted = sum(array(text, "emitted_instances"))
        if any(array(text, "capacity_dropped_instances")):
            raise ValueError(f"{capture}: capacity drops invalidate the comparison")
        if count is None:
            count = emitted
        if emitted != count:
            raise ValueError(f"{capture}: retained population differs")
        path = (capture / "viewport.png").relative_to(folder).as_posix()
        if not (folder / path).is_file():
            raise ValueError(f"Missing {path}")
        views.append(dict(name=name, label=label, description=description, path=path,
                          replay=(capture / "study.ron").relative_to(folder).as_posix(),
                          roots=emitted, vertices=scalar(text, "topology_vertex_inputs"),
                          indices=scalar(text, "submitted_indices")))
    payload = json.dumps(views).replace("<", "\\u003c")
    page = '''<!doctype html><html lang="en"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Grass shape comparison</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#141719;color:#e5e8e7;font:15px system-ui}
header{padding:20px 24px 12px}h1{font-size:24px;margin:0 0 8px}p{margin:7px 0;color:#acb5b4;max-width:1050px;line-height:1.45}
nav{display:flex;align-items:center;gap:12px;flex-wrap:wrap;margin-top:16px}button,select{background:#283033;color:#f0f3f1;border:1px solid #596364;border-radius:5px;padding:7px;font:inherit}button{cursor:pointer}
main{display:grid;grid-template-columns:1fr 1fr;gap:12px;padding:12px 24px}section{min-width:0}select{width:100%}.note{min-height:44px;font-size:13px}.viewport{width:100%;aspect-ratio:16/9;overflow:auto;background:#0e1214;cursor:grab;border:1px solid #424a4c}.viewport:active{cursor:grabbing}.viewport img{display:block;width:100%;max-width:none;pointer-events:none;user-select:none}.counts{font:12px ui-monospace;margin-top:8px;color:#abb6b6}footer{padding:0 24px 24px;color:#9ea9a7;font-size:13px}a{color:#96cccb}input{accent-color:#96cccb}
</style><header><h1>Grass shape comparison</h1>
<p>Fixed camera, wind, catalog and retained roots. Compare silhouette changes on the same blades. Full shape uses the existing 5-section main blade and 4-section companion; it is not an ideal smooth curve.</p>
<p>All shape views use high topology to isolate the shape from density and bin changes. Production draw uses the actual bins. These captures do not measure GPU time.</p>
<nav><label>Linked zoom <input id="zoom" type="range" min="1" max="5" step=".1" value="1"></label><output id="factor">1×</output><button id="reset">Fit</button><button id="front">Inspect foreground</button><button id="swap">Swap views</button><span>Drag or scroll either image to pan both.</span></nav></header>
<main><section><select id="left" aria-label="Left view"></select><p class="note" id="left-note"></p><div class="viewport" id="a"><img alt="Left grass comparison" draggable="false"></div><div class="counts" id="left-counts"></div></section>
<section><select id="right" aria-label="Right view"></select><p class="note" id="right-note"></p><div class="viewport" id="b"><img alt="Right grass comparison" draggable="false"></div><div class="counts" id="right-counts"></div></section></main>
<footer>Green in the cause view means morph ≥ 0.999; orange and blue identify the smaller of the budget and projected-extent limits. A high topology blade can already be morphed almost to the low shape. Report validation checks equal saved inputs, equal retained counts and zero capacity drops; exact root identity is covered by the native GPU test.</footer>
<script>
const views=PAYLOAD; const q=id=>document.getElementById(id);const panes=[q('a'),q('b')];
for(const [n,id] of ['left','right'].entries()) { const select=q(id); for(const v of views){const o=document.createElement('option');o.value=v.name;o.textContent=v.label;select.append(o)}select.value=n?'full':'current';select.onchange=()=>{const v=views.find(v=>v.name===select.value);panes[n].querySelector('img').src=v.path;q(id+'-note').textContent=v.description;const c=q(id+'-counts');c.textContent=`${v.roots.toLocaleString()} retained roots · ${v.vertices.toLocaleString()} vertex inputs · ${v.indices.toLocaleString()} indices · `;const a=document.createElement('a');a.href=v.replay;a.textContent='Saved study';c.append(a)};select.onchange()}
let zoom=1;function setZoom(value,center){const p=panes[0];center??=[(p.scrollLeft+p.clientWidth/2)/(p.clientWidth*zoom),(p.scrollTop+p.clientHeight/2)/(p.clientHeight*zoom)];zoom=value;q('zoom').value=value;q('factor').textContent=value.toFixed(1)+'×';for(const p of panes){p.querySelector('img').style.width=(value*100)+'%';p.scrollLeft=center[0]*p.clientWidth*value-p.clientWidth/2;p.scrollTop=center[1]*p.clientHeight*value-p.clientHeight/2}}
q('zoom').oninput=e=>setZoom(+e.target.value);q('reset').onclick=()=>setZoom(1,[.5,.5]);q('front').onclick=()=>setZoom(3,[.66,.79]);q('swap').onclick=()=>{const a=q('left'),b=q('right');[a.value,b.value]=[b.value,a.value];a.onchange();b.onchange()};
for(const [i,p] of panes.entries()){p.onscroll=()=>{const other=panes[1-i];if(Math.abs(other.scrollLeft-p.scrollLeft)>.5)other.scrollLeft=p.scrollLeft;if(Math.abs(other.scrollTop-p.scrollTop)>.5)other.scrollTop=p.scrollTop};let drag;p.onpointerdown=e=>{if(e.button!==0)return;drag=[e.clientX,e.clientY,p.scrollLeft,p.scrollTop];p.setPointerCapture(e.pointerId)};p.onpointermove=e=>{if(drag){p.scrollLeft=drag[2]+drag[0]-e.clientX;p.scrollTop=drag[3]+drag[1]-e.clientY}};p.onpointerup=p.onpointercancel=()=>drag=null}
</script></html>'''.replace("PAYLOAD", payload)
    path = folder / "comparison.html"
    path.write_text(page)
    (folder / "comparison.json").write_text(json.dumps(views, indent=2) + "\n")
    print(path)
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--load", type=Path, help="Saved study; source for camera, catalog and wind")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--camera", choices=["game-close", "overhead", "top"], default="game-close")
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--report-only", action="store_true", help="Validate existing captures and rebuild the HTML")
    parser.add_argument("--prefix", default="", help="Capture folder prefix, e.g. close-")
    args = parser.parse_args()
    if not re.fullmatch(r"[a-zA-Z0-9_-]*", args.prefix):
        parser.error("Prefix must contain only letters, numbers, underscores or hyphens")
    folder = args.output.resolve()
    if not args.report_only:
        if args.load is None:
            parser.error("--load is required for capture")
        source = args.load.resolve(strict=True)
        # Keep the opening-on baseline explicit even when reusing an opening-off study.
        if re.search(r"inspection_disable_opening:\s*true", source.read_text()):
            parser.error("Use a study with view opening enabled as the baseline")
        for index, (name, _, _) in enumerate(VIEWS):
            command = ["python3", str(ROOT / "tools/vegetation_study.py"), "capture", "--load", str(source),
                       "--camera", args.camera, "--field", "16", "--no-edge", "--no-wind",
                       "--shape", "full" if name == "full-no-opening" else name,
                       "--output", str(folder / (args.prefix + name))]
            if args.no_build or index:
                command.append("--no-build")
            if name == "full-no-opening":
                command.append("--no-opening")
            subprocess.run(command, cwd=ROOT, check=True)
    report(folder, args.prefix)


if __name__ == "__main__":
    main()
