"""Clearance between roads of an import-roads RON, with the import's cubic handles
(road_import::knots). Pairs closer than half their widths outside a junction would fail the cook."""
import math
import re
import sys

import numpy as np

text = open(sys.argv[1]).read()
roads = {}
for m in re.finditer(r'name: "([^"]+)",\s*style: "[^"]+",\s*width: ([\d.]+),\s*traffic: [\d.]+,\s*seed: \d+,'
                     r'(?:\s*from: \("([^"]+)", (\d+)\),)?\s*points: \[([^\]]*)\]', text):
    pts = [tuple(map(float, p)) for p in re.findall(r'\(([-\d.]+), ([-\d.]+)\)', m.group(5))]
    roads[m.group(1)] = dict(width=float(m.group(2)), frm=m.group(3), idx=int(m.group(4)) if m.group(4) else None, own=pts)


def knots(name):
    r = roads[name]
    if r["frm"]:
        return [knots(r["frm"])[r["idx"]]] + r["own"]
    return r["own"]


def curve(p):
    p = np.asarray(p, float)
    out = []
    for i in range(len(p) - 1):
        prev, nxt = p[max(i - 1, 0)], p[min(i + 1, len(p) - 1)]
        d = nxt - prev; u0 = d / max(np.linalg.norm(d), 1e-9)
        prev2, nxt2 = p[i], p[min(i + 2, len(p) - 1)]
        d2 = nxt2 - prev2; u1 = d2 / max(np.linalg.norm(d2), 1e-9)
        reach = np.linalg.norm(p[i + 1] - p[i]) / 3
        a, b, c, e = p[i], p[i] + u0 * reach, p[i + 1] - u1 * reach, p[i + 1]
        t = np.linspace(0, 1, max(2, int(np.linalg.norm(e - a)) + 1))[:, None]
        out.append((1 - t) ** 3 * a + 3 * (1 - t) ** 2 * t * b + 3 * (1 - t) * t * t * c + t ** 3 * e)
    return np.concatenate(out)


curves = {n: curve(knots(n)) for n in roads}
junctions = {n: np.asarray(knots(n)[0]) for n, r in roads.items() if r["frm"]}
worst = True
names = list(roads)
for i, a in enumerate(names):
    for b in names[i + 1:]:
        ca, cb = curves[a], curves[b]
        # Leave out the neighbourhood of a junction joining these two roads.
        near = []
        for child, parent in ((a, b), (b, a)):
            if roads[child]["frm"] == parent:
                near.append(junctions[child])
        ka, kb = ca, cb
        for j in near:
            ka = ka[np.linalg.norm(ka - j, axis=1) > 30]
            kb = kb[np.linalg.norm(kb - j, axis=1) > 30]
        if not len(ka) or not len(kb):
            continue
        d = np.min(np.linalg.norm(ka[:, None, :] - kb[None, ::4, :], axis=2))
        need = (roads[a]["width"] + roads[b]["width"]) / 2
        if d < need + 5:
            print(f"CLOSE {a} / {b}: {d:.1f} m (half widths {need:.1f} m)")
            worst = False

# Junction arms: the branch's first handle against the main road's handles at its knot.
for n, r in roads.items():
    if not r["frm"]:
        continue
    main = knots(r["frm"])
    k = r["idx"]
    prev, nxt = np.asarray(main[max(k - 1, 0)]), np.asarray(main[min(k + 1, len(main) - 1)])
    road = math.atan2(*(nxt - prev)[::-1])
    arm = math.atan2(*(np.asarray(r["own"][0]) - np.asarray(main[k]))[::-1])
    off = abs((arm - road + math.pi) % (2 * math.pi) - math.pi)
    angle = math.degrees(min(off, math.pi - off)) if k < len(main) - 1 else 180.0
    if angle < 35:
        print(f"NARROW junction {n} on {r['frm']} knot {k}: {angle:.0f} degrees")
        worst = False
print("all roads clear" if worst else "fix the roads above")
