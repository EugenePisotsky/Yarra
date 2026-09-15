#!/usr/bin/env python3
"""Extract native tuft fixture captures and build a local visual comparison. No scores."""

import argparse
import csv
from pathlib import Path

PAGE = """<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<title>Grass — light between the blades</title>
<style>
*{box-sizing:border-box}body{margin:0;background:#151817;color:#e5ebe6;font:15px system-ui}
main{max-width:1500px;margin:auto;padding:24px}h1{font-size:25px;margin:0 0 8px}
p{color:#b1bcb3;line-height:1.5;max-width:950px}nav{display:flex;gap:20px;flex-wrap:wrap;align-items:center;margin:20px 0}
label{display:flex;align-items:center;gap:8px}select,button{background:#29312c;color:inherit;border:1px solid #526256;border-radius:5px;padding:9px;font:inherit}
button{cursor:pointer}#stage{aspect-ratio:16/9;position:relative;overflow:hidden;background:#000;border:1px solid #465048;touch-action:none;cursor:ew-resize}
.layer{position:absolute;inset:0;overflow:hidden}.layer img{width:100%;height:100%;object-fit:contain;transform:scale(1.4);transform-origin:52% 70%;display:block}
#left{clip-path:inset(0 50% 0 0)}#divider{position:absolute;top:0;bottom:0;left:50%;border-left:2px solid #f5fff6;pointer-events:none}
.tag{position:absolute;top:12px;background:#121714df;padding:8px 12px;border-radius:4px;pointer-events:none}.a{left:12px}.b{right:12px}
#split{width:100%;accent-color:#a9d2ae;margin:14px 0}small{display:block;color:#9bad9e;line-height:1.5}
@media(max-width:650px){main{padding:12px}nav{gap:10px}h1{font-size:21px}.tag{font-size:12px;padding:5px}}
</style>
<main><h1>Light between the blades</h1>
<p>The same four tufts in every version. Drag the divider to compare their current material with geometry-based occlusion.</p>
<nav>
<label>View <select id="view"><option value="overhead">Above the grass</option><option value="near">Near the grass</option></select></label>
<label>Right side <select id="mode"><option value="combined">AO + blade shadows</option><option value="ambient">AO only</option></select></label>
<button id="before">Show current material</button><button id="after">Show selected result</button>
<label><input id="zoom" type="checkbox" checked>Closer view</label>
</nav>
<div id="stage" aria-label="Grass comparison">
<div class="layer"><img id="result" alt="Grass with selected occlusion"></div>
<div class="layer" id="left"><img id="baseline" alt="Grass with current material"></div>
<div id="divider"></div><span class="tag a">Current material</span><span class="tag b" id="resultLabel">AO + blade shadows</span>
</div>
<input id="split" type="range" min="0" max="100" value="50" aria-label="Comparison divider">
<p id="explanation"></p>
<small>128 roots / 256 blades. Fixed wind and sun. No added blades between lighting versions.<br>
Native renderer captures. Diagnostic preview only; the game and editor were not changed by this experiment.</small>
</main><script>
const $=id=>document.getElementById(id);
function divider(value){$('split').value=value;$('left').style.clipPath=`inset(0 ${100-value}% 0 0)`;$('divider').style.left=value+'%'}
function update(){const stem=$('view').value+'-sun-0';const mode=$('mode').value;
$('baseline').src=stem+'-baseline.png';$('result').src=stem+'-'+mode+'.png';
$('resultLabel').textContent=mode==='combined'?'AO + blade shadows':'AO only';
$('explanation').textContent=mode==='combined'?'Sky occlusion plus shadows cast by the actual blades. The stronger contrast here comes mainly from direct sunlight being blocked by neighboring leaves.':'Only ambient sky light is reduced where other blades block it. Direct sunlight stays unchanged. The effect is modest in this sunlit setup.'}
$('split').oninput=e=>divider(e.target.value);$('view').onchange=update;$('mode').onchange=update;
$('before').onclick=()=>divider(100);$('after').onclick=()=>divider(0);
$('zoom').onchange=e=>document.querySelectorAll('.layer img').forEach(img=>img.style.transform=e.target.checked?'scale(1.4)':'none');
function drag(e){const r=$('stage').getBoundingClientRect();divider(Math.max(0,Math.min(100,(e.clientX-r.left)/r.width*100)))}
$('stage').onpointerdown=e=>{e.currentTarget.setPointerCapture(e.pointerId);drag(e)};
$('stage').onpointermove=e=>{if(e.buttons)drag(e)};update();
</script></html>"""


def main():
    from PIL import Image
    import numpy as np

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folder", type=Path)
    folder = parser.parse_args().folder.resolve()
    settings = dict(line.split("=", 1) for line in (folder / "parameters.txt").read_text().splitlines())
    width, height = map(int, settings["size"].split("x"))
    count = width * height
    with (folder / "frames.csv").open() as manifest:
        for row in csv.DictReader(manifest):
            stem = row["stem"]
            data = (folder / f"{stem}.bin").read_bytes()
            colors = np.frombuffer(data, np.uint8, count=count * 12).reshape(3, height, width, 4)
            for mode, color in zip(("baseline", "ambient", "combined"), colors):
                Image.fromarray(color).save(folder / f"{stem}-{mode}.png")
            visibility = np.frombuffer(data, "<f4", count=count * 4, offset=count * 12).reshape(height, width, 4)
            covered = visibility[..., 3] > 0
            assert np.isfinite(visibility).all(), "nonfinite visibility"
            # Native floating-point division can return 1 + one ULP for an unobstructed sky.
            assert ((visibility[covered, :2] >= -1e-6) & (visibility[covered, :2] <= 1 + 1e-6)).all()
            mask = np.frombuffer(data, np.uint8, count=count * 4, offset=count * 28).reshape(height, width, 4)
            Image.fromarray(mask).save(folder / f"{stem}-visibility.png")
    (folder / "comparison.html").write_text(PAGE)
    print(folder / "comparison.html")


if __name__ == "__main__":
    main()
