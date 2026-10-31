"""Bakes the masks a character's skin paints with: brows and makeup, in the head's texture layout.

uv run --with numpy --with scipy --with pillow --with tifffile python skin_layers_bake.py \
  SKIN.json BROW_PACK MESH.npz OUT_DIR [--ktx PATH]

SKIN is a skin file, BROW_PACK an eyebrow pack as Lambrador3D lays one out (Maps/<n>/*alpha.bmp and
OBJ/Eyebrows_Low.obj), MESH what head_mesh.py writes. OUT_DIR is the head's skin folder: it needs
skin_bake.py's head_color.png, and gets brow_<n>.ktx2, makeup.ktx2 and skin_layers.json, which the
export copies into the kit. The game mixes the masks over the skin as it draws it
(shaders/lighting/skin_layers.wesl), so a character's colours cost no texture of their own.

Each brow's map is packed through its plane's UVs, bent to fill the square: it is unbent through
the plane first. A brow is then its centre line and its thickness, taken apart and laid on this
head: along the brow by arc length over the forehead, inner end to the nose, the line on the crest
of the head's own brow ridge with some of its drawn arch, both sides from the one brow. Being in the
head's layout a brow follows every face slider, and its hairs resolve into soft coverage in the
mipmaps instead of shimmering as cut-outs do.

- brow_<n>: red the hairs, green a soft halo under them: skin between brow hairs is shaded by them.
- makeup: red eyeshadow, green liner, blue blush, alpha lipstick.
"""
import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import gaussian_filter, map_coordinates
from scipy.spatial import cKDTree

sys.path.insert(0, str(Path(__file__).parent))

from head_layout import TEXTURE, Layout, island_at, uv_islands
from skin_bake import compress, find_ktx, rasterize, triangles

Image.MAX_IMAGE_PIXELS = None

# The pack's brows by number, as a character creator offers them: the first is a kit's default.
BROWS = {
    2: "Feathered",
    1: "Soft arch",
    3: "Slim arch",
    4: "Upswept",
    5: "Wispy",
    6: "Unruly",
    7: "Full arch",
    8: "Layered",
    9: "Thin",
    10: "Bold",
}

# Texels of a flattened brow per unit of its plane.
FLAT_DENSITY = 500.0

# On the face, metres: where the brow's inner and outer ends lie from the mid-line (the outer past
# the eye's corner, as a brow's tail is), how far its line stands off the crest of the ridge, how much
# of its drawn arch it keeps over the ridge's own, how much of its drawn thickness, and the band of the
# face the masks may cover. The ridge is a curve through its crest over this stretch of the head.
# Hair thinner than a mask texel loses coverage in the sampling, which the gamma gives back.
BROW_INNER, BROW_OUTER, BROW_LIFT = 0.0125, 0.048, -0.0012
BROW_ARCH, BROW_THICKNESS = 0.41, 0.74
BROW_GAMMA = 0.62

# The ridge is level from the nose, and a brow on it alone reads as worried: its inner end lies this
# far below the line, rising to it at this share of its length, where a brow's peak is.
BROW_RISE, BROW_PEAK = 0.0045, 0.62
RIDGE_ACROSS, RIDGE_HEIGHTS = (0.010, 0.062), (1.640, 1.680)
BAND = (1.622, 1.685)
BAND_X = 0.072

# The brow masks' width, and the halo's reach, metres. Compressed textures come in blocks of 4.
BROW_WIDTH = 3072
BLOCK = 4
HALO = 0.0016

# The makeup mask's size and shapes, metres. Eyeshadow fades over its reach above the upper lid,
# liner over its own from the lid's edge, thinner under the eye; blush is an ellipse on the cheek,
# rising outwards, on the front of the face. The layout folds the lids' edges into the scan's
# closed eyes, so a liner needs more reach on the mesh than it shows on the skin.
MAKEUP_SIZE = 2048
SHADOW_REACH = 0.013
LINER_REACH = (0.0016, 0.0042)
LOWER_LINER = 0.4
BLUSH_CENTRE, BLUSH_RADII, BLUSH_RISE, BLUSH_PEAK = (0.043, 1.603), (0.021, 0.0135), 0.35, 0.6
BLUSH_FRONT = (-0.06, -0.09)

# Lipstick covers what is redder than the skin inside the lips' ellipse (the skin file's gloss area).
LIP_REDNESS = (1.17, 1.26)


def smooth(low, high, value):
    t = np.clip((value - low) / (high - low), 0, 1)
    return t * t * (3 - 2 * t)


def read_planes(path):
    """Each object of the OBJ: its faces as lists of (vertex, UV) indices, with all the vertices."""
    points, uvs, objects = [], [], []

    for line in path.read_text().splitlines():
        if line.startswith("v "):
            points.append([float(value) for value in line.split()[1:4]])
        elif line.startswith("vt "):
            uvs.append([float(value) for value in line.split()[1:3]])
        elif line.startswith("o "):
            objects.append([])
        elif line.startswith("f "):
            objects[-1].append([tuple(int(part) - 1 for part in corner.split("/")[:2]) for corner in line.split()[1:]])

    return np.array(points)[:, :2], np.array(uvs), objects


def flatten(alpha, points, uvs, faces):
    """The map unbent through its plane: coverage, its rows running up, and the plane's size."""
    used = [vertex for face in faces for vertex, _ in face]
    low, high = points[used].min(0), points[used].max(0)
    width, height = ((high - low) * FLAT_DENSITY).astype(int) + 2
    canvas = np.full((height, width, 2), -1.0, np.float32)

    for face in faces:
        for i in range(1, len(face) - 1):
            corners = [face[0], face[i], face[i + 1]]
            p = (points[[c[0] for c in corners]] - low) * FLAT_DENSITY
            q = uvs[[c[1] for c in corners]]
            area = (p[1, 0] - p[0, 0]) * (p[2, 1] - p[0, 1]) - (p[1, 1] - p[0, 1]) * (p[2, 0] - p[0, 0])

            if abs(area) < 1e-9:
                continue

            first, last = np.floor(p.min(0)).astype(int), np.ceil(p.max(0)).astype(int) + 1
            y, x = np.mgrid[max(first[1], 0) : min(last[1], height), max(first[0], 0) : min(last[0], width)]
            b = ((x - p[0, 0]) * (p[2, 1] - p[0, 1]) - (y - p[0, 1]) * (p[2, 0] - p[0, 0])) / area
            c = ((p[1, 0] - p[0, 0]) * (y - p[0, 1]) - (p[1, 1] - p[0, 1]) * (x - p[0, 0])) / area
            inside = (b >= -0.01) & (c >= -0.01) & (b + c <= 1.01)
            canvas[y[inside], x[inside], 0] = (q[0, 0] * (1 - b - c) + q[1, 0] * b + q[2, 0] * c)[inside]
            canvas[y[inside], x[inside], 1] = (q[0, 1] * (1 - b - c) + q[1, 1] * b + q[2, 1] * c)[inside]

    size = alpha.shape[0]
    flat = map_coordinates(alpha, [(1 - canvas[..., 1]) * size, canvas[..., 0] * size], order=1)
    flat[canvas[..., 0] < 0] = 0
    return flat


def brow_ridge(skin_points):
    """The crest of the head's brow ridge: its height as a function of the distance across."""
    across = np.abs(skin_points[:, 0])
    band = (skin_points[:, 2] > RIDGE_HEIGHTS[0]) & (skin_points[:, 2] < RIDGE_HEIGHTS[1])
    bins = np.arange(RIDGE_ACROSS[0], RIDGE_ACROSS[1], 0.002)
    crest = []

    for x in bins:
        near = np.flatnonzero(band & (np.abs(across - x) < 0.0035))
        crest.append(skin_points[near[np.argmin(skin_points[near, 1])], 2])

    fit = np.poly1d(np.polyfit(bins, crest, 3))
    return lambda x: fit(np.clip(x, *RIDGE_ACROSS))


def arc_lengths(skin_points, ridge):
    """Distance over the forehead from the mid-line as a function of the distance across, along the
    ridge. Skin only: MakeHuman's helpers stand in front of it."""
    across = np.abs(skin_points[:, 0])
    near = np.abs(skin_points[:, 2] - ridge(across)) < 0.006
    front = skin_points[near & (skin_points[:, 1] < -0.02)]
    across = np.abs(front[:, 0])
    bins = np.arange(0.0, BAND_X + 0.004, 0.004)
    depth = np.array([front[np.abs(across - x) < 0.004, 1].min() for x in bins])
    arc = np.r_[0.0, np.cumsum(np.hypot(np.diff(bins), np.diff(gaussian_filter(depth, 1.0))))]
    return bins, arc


def centre_line(flat):
    """A flattened brow's centre line: the mean row of its hairs in each column, smoothed."""
    rows = np.arange(flat.shape[0], dtype=np.float32)
    weight = flat.sum(0)
    used = weight > 0.5
    line = np.interp(np.arange(flat.shape[1]), np.flatnonzero(used), ((flat * rows[:, None]).sum(0) / np.maximum(weight, 1e-6))[used])
    return gaussian_filter(line, used.sum() * 0.05, mode="nearest")


def brow_mask(flat, across, height, arcs, ridge, filled):
    """A flattened brow on the face: its hairs and their halo. The flat image's left is the tail."""
    bins, arc = arcs
    columns = np.nonzero(flat > 0.5)[1]
    left, right = columns.min(), columns.max()
    inner = np.interp(BROW_INNER, bins, arc)
    scale = (np.interp(BROW_OUTER, bins, arc) - inner) / (right - left)
    column = right - (np.interp(across, bins, arc) - inner) / scale
    line = centre_line(flat)
    drawn = np.interp(column, np.arange(len(line)), line)
    # The line on the ridge, with its rise and some of its arch, and the hairs about it at their new
    # thickness.
    along = np.clip((right - column) / (right - left) / BROW_PEAK, 0, 1)
    rise = BROW_RISE * (1 - along * along * (3 - 2 * along))
    placed = ridge(across) + BROW_LIFT - rise + (drawn - line[left : right + 1].mean()) * scale * BROW_ARCH
    row = drawn + (height - placed) / (scale * BROW_THICKNESS)
    # The brow's texels per mask texel, so the sample takes in what one mask texel covers.
    texels = np.median(np.abs(np.gradient(column, axis=1))[filled])
    soft = gaussian_filter(flat, max(texels * 0.5, 0.01))
    hairs = np.clip(map_coordinates(soft, [row, column], order=1, mode="constant"), 0, 1) ** BROW_GAMMA * filled
    per_metre = np.nanmedian(np.abs(np.gradient(across, axis=1))[filled])
    halo = gaussian_filter(hairs, HALO / max(per_metre, 1e-7))
    return hairs, np.clip(halo * 1.6, 0, 1)


def brows(args, positions, tris, texels, outdir, ktx):
    """Writes each brow's mask; returns the manifest's brow entries and the mask's rectangle."""
    skin_points = positions[np.unique(tris)]
    ridge = brow_ridge(skin_points)
    print("LAYERS ridge heights at 15, 30, 45, 60 mm:", np.round(ridge(np.array([0.015, 0.03, 0.045, 0.06])), 4))
    band = (positions[tris, 2].min(1) > BAND[0]) & (positions[tris, 2].max(1) < BAND[1])
    band &= (np.abs(positions[tris, 0]).max(1) < BAND_X) & (positions[tris, 1].max(1) < -0.03)
    tris = tris[band]
    used = texels[np.unique(tris)]
    low, high = np.floor(used.min(0)) - 8, np.ceil(used.max(0)) + 8
    scale = BROW_WIDTH / (high[0] - low[0])
    height = int(np.ceil((high[1] - low[1]) * scale / BLOCK)) * BLOCK
    filled_values, filled = rasterize((texels - low) * scale, positions, tris, BROW_WIDTH)
    filled_values, filled = filled_values[:height], filled[:height]
    across, up = np.abs(filled_values[..., 0]), filled_values[..., 2]
    arcs = arc_lengths(skin_points, ridge)
    points, uvs, objects = read_planes(args.pack / "OBJ/Eyebrows_Low.obj")
    entries = []

    for number, label in BROWS.items():
        path = next((args.pack / "Maps" / str(number)).glob("*alpha.bmp"))
        alpha = np.asarray(Image.open(path).convert("L"), dtype=np.float32) / 255
        flat = flatten(alpha, points, uvs, objects[number - 1])
        hairs, halo = brow_mask(flat, across, up, arcs, ridge, filled)
        image = np.dstack([hairs, halo, hairs])
        name = f"brow_{number:02}"
        Image.fromarray((np.clip(image, 0, 1) * 255 + 0.5).astype(np.uint8)).save(outdir / f"{name}.png")
        compress(ktx, outdir / f"{name}.png", False)
        entries.append({"label": label, "mask": f"{name}.ktx2"})

    rect = [low[0] / TEXTURE, low[1] / TEXTURE, (high[0] - low[0]) / TEXTURE, height / scale / TEXTURE]
    print(f"LAYERS {len(entries)} brows, {BROW_WIDTH}x{height}, {scale:.2f} mask texels per head texel")
    return entries, [round(value, 6) for value in rect]


def lid_edges(skin, mesh, head, islands):
    """Each eye's lid edge: the vertices where the head's skin joins its socket's inside, and where
    the upper lid's lie. The first of the cavity points is the mouth's. Positions are per loop."""
    vertices, positions, uv = mesh["vertices"], mesh["positions"], mesh["uv"]
    edges = []

    for point in skin["cavity_uv_points"][1:]:
        socket = island_at(uv, islands, point)
        shared = np.intersect1d(vertices[head], vertices[socket])
        edges.append(positions[np.isin(vertices, shared)])

    return edges


def makeup(skin, layout, mesh, head, islands, tris, texels, outdir, ktx):
    """Writes the makeup mask."""
    scale = MAKEUP_SIZE / TEXTURE
    filled, hit = rasterize(texels * scale, mesh["positions"], tris, MAKEUP_SIZE)
    x, up = np.abs(filled[..., 0]), filled[..., 2]
    shadow, liner = np.zeros_like(x), np.zeros_like(x)
    edges = lid_edges(skin, mesh, head, islands)

    for edge in edges:
        tree = cKDTree(edge)
        distances, nearest = tree.query(filled.reshape(-1, 3))
        distances = distances.reshape(x.shape)
        margin = edge[nearest.reshape(x.shape)]
        eye_height = edge[:, 2].mean()
        # Which lid a point is on, by the height of the edge it is nearest: the rim rolls forward
        # from the edge at about the edge's own height.
        above = smooth(-0.001, 0.0015, margin[..., 2] - eye_height) * smooth(-0.0005, 0.001, up - eye_height)
        fade = smooth(LINER_REACH[1], LINER_REACH[0], distances)
        liner = np.maximum(liner, fade * (above + (1 - above) * LOWER_LINER))
        shadow = np.maximum(shadow, above * smooth(SHADOW_REACH, 0.0015, distances))

    outward = (x - BLUSH_CENTRE[0]) / BLUSH_RADII[0]
    rising = (up - BLUSH_CENTRE[1] - (x - BLUSH_CENTRE[0]) * BLUSH_RISE) / BLUSH_RADII[1]
    blush = np.exp(-(outward * outward + rising * rising) * 1.4) * BLUSH_PEAK * smooth(*BLUSH_FRONT, filled[..., 1])
    colour = np.asarray(Image.open(outdir / "head_color.png").convert("RGB").resize((MAKEUP_SIZE,) * 2), np.float32)
    ratio = colour[..., 0] / np.maximum(colour[..., 1], 1.0)
    cx, cy, rx, ry, _ = next(area for area in skin["gloss"] if area["name"] == "lips")["ellipse"]
    centre = layout.origin + np.array([cx, cy]) * layout.scale
    grid_y, grid_x = np.mgrid[0:MAKEUP_SIZE, 0:MAKEUP_SIZE].astype(np.float32) / scale
    inside = ((grid_x - centre[0]) / (rx * layout.scale)) ** 2 + ((grid_y - centre[1]) / (ry * layout.scale)) ** 2
    lips = smooth(*LIP_REDNESS, gaussian_filter(ratio, 1.5)) * smooth(1.0, 0.8, inside)
    image = np.dstack([shadow * hit, liner * hit, blush * hit, lips])
    Image.fromarray((np.clip(image, 0, 1) * 255 + 0.5).astype(np.uint8)).save(outdir / "makeup.png")
    compress(ktx, outdir / "makeup.png", False, alpha=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("skin", type=Path)
    parser.add_argument("pack", type=Path)
    parser.add_argument("mesh", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--ktx")
    args = parser.parse_args()
    skin = json.loads(args.skin.read_text())
    ktx = find_ktx(args.ktx)
    mesh = dict(np.load(args.mesh))
    uv, starts, totals = mesh["uv"], mesh["starts"], mesh["totals"]
    # Per loop, as the bakes rasterize.
    mesh["positions"] = mesh["positions"][mesh["vertices"]]
    layout = Layout(skin)
    islands = uv_islands(uv, starts, totals)
    head = island_at(uv, islands, skin["head_uv_point"])
    tris = triangles(starts[head[starts]], totals[head[starts]])
    texels = layout.texels(layout.scan(uv))
    entries, rect = brows(args, mesh["positions"], tris, texels, args.out, ktx)
    makeup(skin, layout, mesh, head, islands, tris, texels, args.out, ktx)
    manifest = {"brow_rect": rect, "brows": entries, "makeup": "makeup.ktx2"}
    (args.out / "skin_layers.json").write_text(json.dumps(manifest, indent=1))
    print("LAYERS_DONE")


main()
