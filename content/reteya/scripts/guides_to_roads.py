"""Turn reteya_starter_v06 route guides into an `import-roads` RON file.

The guides are graded polylines. Corners are rounded within a small radius and the result is
resampled, so the import's smooth spans stay on the guide. The journey stops at both bridge pads
(no bridge mesh yet) and long routes continue as chained roads of at most 30 knots.
"""
import json
import math
import re
import sys
from pathlib import Path

design = json.loads(Path(sys.argv[1]).read_text())
styles_source = Path(sys.argv[2]).read_text()
out = Path(sys.argv[3])
MAX_KNOTS = 30
SPACING = 40.0
CORNER = 30.0

g = {k: [(p[0], p[2]) for p in v["points"]] for k, v in design["guides"].items()}


def rounded(points):
    """Cut each interior corner by up to CORNER metres and join the cuts with a quadratic arc."""
    out = [points[0]]
    for a, b, c in zip(points, points[1:], points[2:]):
        ab, bc = math.dist(a, b), math.dist(b, c)
        cut = min(CORNER, 0.4 * ab, 0.4 * bc)
        p0 = (b[0] + (a[0] - b[0]) * cut / ab, b[1] + (a[1] - b[1]) * cut / ab)
        p2 = (b[0] + (c[0] - b[0]) * cut / bc, b[1] + (c[1] - b[1]) * cut / bc)
        for t in (0.0, 0.25, 0.5, 0.75, 1.0):
            out.append(((1 - t) ** 2 * p0[0] + 2 * t * (1 - t) * b[0] + t * t * p2[0],
                        (1 - t) ** 2 * p0[1] + 2 * t * (1 - t) * b[1] + t * t * p2[1]))
    out.append(points[-1])
    return out


def resample(points, spacing=SPACING):
    lengths = [0.0]
    for a, b in zip(points, points[1:]):
        lengths.append(lengths[-1] + math.dist(a, b))
    total = lengths[-1]
    count = max(1, round(total / spacing))
    result, j = [], 0
    for i in range(count + 1):
        d = total * i / count
        while j + 1 < len(lengths) - 1 and lengths[j + 1] < d:
            j += 1
        seg = lengths[j + 1] - lengths[j]
        t = 0.0 if seg == 0 else (d - lengths[j]) / seg
        a, b = points[j], points[j + 1]
        result.append((a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t))
    return result, total


def chained(name, style, width, traffic, seed, points, from_=None):
    """Split a dense polyline into roads of at most MAX_KNOTS knots, each leaving the previous."""
    roads, first, part = [], 0, 1
    while first < len(points) - 1:
        # A road that leaves another gets that point inserted as its first knot.
        room = MAX_KNOTS - (1 if (from_ or roads) else 0)
        last = min(len(points) - 1, first + room - (0 if (from_ or roads) else 1))
        take = points[first + (1 if (from_ or roads) else 0): last + 1]
        label = name if part == 1 else f"{name} {part}"
        start = from_ if not roads else (roads[-1]["name"], roads[-1]["knots"] - 1)
        roads.append({"name": label, "style": style, "width": width, "traffic": traffic,
                      "seed": seed + part, "from": start, "points": take,
                      "knots": len(take) + (1 if start else 0)})
        first, part = last, part + 1
    return roads


def nearest_index(road, point):
    """Knot index on `road` nearest `point`, counting an inserted junction point as knot 0."""
    offset = 1 if road["from"] else 0
    best = min(range(len(road["points"])), key=lambda i: math.dist(road["points"][i], point))
    return best + offset, road["points"][best]


bridge = design["bridge"]
cx, cz = bridge["center"]; tx, tz = bridge["tangent_xz"]; half = bridge["span_m"] / 2
south_pad = (cx - tx * half, cz - tz * half)
north_pad = (cx + tx * half, cz + tz * half)

journey = g["journey"]
landing = rounded(journey[:6]) + [south_pad]
pass_part = [north_pad] + journey[6:]
landing_pts, landing_len = resample(rounded(journey[:6] + [south_pad]))
pass_pts, pass_len = resample(rounded(pass_part))

roads = []
roads += chained("Landing road", "Baltic cart track", 4.0, 0.8, 101, landing_pts)
roads += chained("Pass road", "Baltic cart track", 3.6, 0.6, 111, pass_pts)


def trimmed(points, metres):
    """The polyline without its last `metres`."""
    keep = sum(math.dist(a, b) for a, b in zip(points, points[1:])) - metres
    out = [points[0]]
    for a, b in zip(points, points[1:]):
        step = math.dist(a, b)
        if step >= keep:
            out.append((a[0] + (b[0] - a[0]) * keep / step, a[1] + (b[1] - a[1]) * keep / step))
            return out
        keep -= step
        out.append(b)
    return out


def knots_of(road):
    """All knots of a road as the import builds them, its junction point first if it branches."""
    if not road["from"]:
        return road["points"]
    parent = next(r for r in roads if r["name"] == road["from"][0])
    return [knots_of(parent)[road["from"][1]]] + road["points"]


def divergence(knots, k, toward):
    """Smallest angle in degrees between a branch leaving knot k toward `toward` and the road's
    arms there, whose handles lie along the direction from the previous knot to the next."""
    prev, nxt = knots[max(k - 1, 0)], knots[min(k + 1, len(knots) - 1)]
    road = math.atan2(nxt[1] - prev[1], nxt[0] - prev[0])
    arm = math.atan2(toward[1] - knots[k][1], toward[0] - knots[k][0])
    off = abs((arm - road + math.pi) % (2 * math.pi) - math.pi)
    return math.degrees(min(off, math.pi - off))


def branch(name, style, width, traffic, seed, guide_points, parents, trim=0.0):
    pts, length = resample(trimmed(rounded(guide_points), trim))
    # The import refuses junction arms closer than 30 degrees: join at the knot near the guide's
    # start whose arms diverge most, preferring the nearest that clears 50 degrees.
    candidates = []
    for parent in parents:
        knots = knots_of(parent)
        for k in range(1, len(knots) - 1):
            d = math.dist(knots[k], pts[0])
            if d <= 90:
                candidates.append((divergence(knots, k, pts[1]), d, parent["name"], k))
    clear = [c for c in candidates if c[0] >= 50]
    angle, _, parent_name, index = min(clear, key=lambda c: c[1]) if clear else max(candidates)
    # chained() drops the first point: the junction on the parent takes its place.
    return chained(name, style, width, traffic, seed, pts, from_=(parent_name, index)), length


# An import joins a road only at its start, so each loop guide becomes two branches that leave
# the main road and meet at their hamlet, the second stopping short of the first.
lowland = [r for r in roads if r["name"].startswith("Landing road")]
upland = [r for r in roads if r["name"].startswith("Pass road")]
loop, spur = g["village_loop"], g["hamlet_spur"]
village, village_len = branch("Farm loop", "Baltic cart track", 3.2, 0.45, 121, loop[:5], lowland)
village_back, back_len = branch("Farm loop 2", "Baltic cart track", 3.2, 0.35, 125, loop[4:][::-1], lowland, trim=25)
woodland, woodland_len = branch("Woodland spur", "Baltic cart track", 3.0, 0.35, 131, g["east_loop"], lowland)
hamlet, hamlet_len = branch("Mountain hamlet track", "Forest footpath", 1.6, 0.45, 141, spur[:3], upland)
hamlet_back, hamlet_back_len = branch("Mountain hamlet track 2", "Forest footpath", 1.6, 0.35, 145, spur[2:][::-1], upland, trim=25)
village_len += back_len
hamlet_len += hamlet_back_len
# The return branch goes first: an earlier import chained it to the Farm loop, and the import
# rewrites a road only once no stale junction hangs off it.
roads += village_back + village + woodland + hamlet + hamlet_back

styles = re.search(r"\n    styles: \[\n.*?\n    \],\n", styles_source, re.S).group(0)
lines = [
    "// Roads of reteya_starter_v06, generated from its route guides (design.json: journey,",
    "// village_loop, east_loop, hamlet_spur) by guides_to_roads.py. Corners are rounded within",
    f"// {CORNER:g} m and resampled every {SPACING:g} m so the smooth spans stay on the graded guides.",
    "// The journey stops at both pads of the planned 52 m gorge bridge (no bridge mesh yet).",
    "// Styles are copied from content/roads/island.ron.",
    "(" + styles.rstrip("\n"),
    "    roads: [",
]
for r in roads:
    pts = ", ".join(f"({x:.1f}, {z:.1f})" for x, z in r["points"])
    frm = f'\n            from: ("{r["from"][0]}", {r["from"][1]}),' if r["from"] else ""
    lines.append(f"""        (
            name: "{r['name']}",
            style: "{r['style']}",
            width: {r['width']},
            traffic: {r['traffic']},
            seed: {r['seed']},{frm}
            points: [{pts}],
        ),""")
lines += ["    ],", ")", ""]
out.write_text("\n".join(lines))
print(json.dumps({
    "landing_road_m": round(landing_len), "pass_road_m": round(pass_len),
    "farm_loop_m": round(village_len), "woodland_spur_m": round(woodland_len),
    "hamlet_track_m": round(hamlet_len),
    "roads": [(r["name"], r["knots"], r["from"]) for r in roads],
}, indent=1))
