"""Makes a bought hair a style the game can load: fitted to a character's skull, with its textures.

uv run --with numpy --with scipy --with pillow --with tifffile python hair_fit.py \
  PACK.npz HAIR SOURCE_DIR SKIN.json MESH.npz OUT_ROOT [--ktx PATH] [--mesh-only]

PACK is what hair_pack.py writes, HAIR a hair's name in it (hair8), SOURCE_DIR that hair's own folder
as the seller ships it (Hair_D.tga with alpha, Hair_N.tga: a card's UVs fit no other hair's), SKIN a
skin file and MESH what head_mesh.py writes. OUT_ROOT/HAIR gets style.hair (engine/src/character_hair.rs
reads it), hair_color.ktx2, hair_normal.ktx2 and hairline.ktx2; --mesh-only skips those three, the slow
part, when only the fit or the cards changed.

The pack's head is laid on ours by a scale and shift over the skull, and the hair takes the smooth
field of what is left between the two surfaces. Each hair vertex binds to the triangle of skin nearest
it, so face sliders carry the hair as they do any part.

The hairline mask is the hair's cover of the head: solid under its cards, so no skin shows between
them and their hard edge reads as hair, thinning into single strands and a haze over the skin in front
of it, along the way the hair grows from the edge. The skin paints it (shaders/lighting/skin_layers.wesl).
"""
import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import binary_closing, binary_fill_holes, distance_transform_edt, gaussian_filter, map_coordinates
from scipy.spatial import cKDTree

sys.path.insert(0, str(Path(__file__).parent))

from head_layout import TEXTURE, Layout, island_at, uv_islands
from skin_bake import bleed, compress, find_ktx, rasterize, triangles

Image.MAX_IMAGE_PIXELS = None

# The version of style.hair (engine/src/character_hair.rs).
VERSION = 1

# The skull: what lies this far below a head's top, metres. Matches this far along the worst share are
# left out of the fit, and the rounds it takes.
SKULL_DEPTH, MATCH_SHARE, ROUNDS = 0.085, 85, 30

# The field of offsets: its reach, metres, over how many of the pack's head's points.
FIELD_REACH, FIELD_POINTS = 0.025, 24

# Triangles of skin a hair vertex tries, nearest first.
BIND_TRIES = 16

# How a hair's offsets scale with the head along each axis: two base-mesh vertices and the distance
# they were at in MakeHuman's units, as its own hairs do.
SCALES = ((5399, 11998, 1.398), (791, 881, 2.2048), (962, 5320, 1.8461))

# The textures' size, and how their files are named.
COLOUR_SIZE = 2048
COLOUR_FILE, NORMAL_FILE = "Hair_D.tga", "Hair_N.tga"

# The hairline mask's size. Hair covers the skin it projects onto within this reach, metres, at this
# many samples along a card triangle's side; gaps closed over this many texels. Its strands reach this
# far over the skin and solid hair begins this far under the cards, in texels of the mask (about 6 a
# millimetre on the forehead), under a haze of fine hair at this strength and reach, which is what
# keeps the edge from reading as a comb. Strands run this many texels from seeded noise, and thin to
# nothing as they go.
HAIRLINE_SIZE = 2048
HAIR_REACH, HAIR_SAMPLES, HAIR_CLOSING = 0.016, 12, 10

# Hair covers only the scalp: skin above this height, or above the nape's behind the ears (metres over
# the feet, and from the head's mid-plane towards the back). Long hair hanging over a neck or cheek
# grows from elsewhere. On the front of the face the height rises to the forehead's top between these
# distances from the mid-line: a fringe swept over a forehead does not grow there.
SCALP_ABOVE, NAPE_ABOVE, NAPE_BEHIND = 1.645, 1.59, -0.03
FOREHEAD_TOP, FOREHEAD_SIDES, FOREHEAD_FRONT = 1.708, (0.075, 0.045), -0.08

HAIR_OUTSIDE, HAIR_INSIDE = 24.0, 8.0
HAZE_STRENGTH, HAZE_REACH = 0.5, 34.0
STRAND_STEPS, STRAND_SEED = 16, 11


def smooth(low, high, value):
    t = np.clip((value - low) / (high - low), 0, 1)
    return t * t * (3 - 2 * t)


def to_gltf(points):
    """Blender's Z-up axes to glTF's Y-up."""
    return points[:, [0, 2, 1]] * np.array([1.0, 1.0, -1.0])


def fit_skull(pack, ours):
    """The scale and shift that lays the pack's skull on ours."""
    a = pack[pack[:, 2] > pack[:, 2].max() - SKULL_DEPTH]
    b = ours[ours[:, 2] > ours[:, 2].max() - SKULL_DEPTH]
    tree = cKDTree(b)
    scale, shift = 1.0, np.array([0.0, 0.0, b[:, 2].max() - a[:, 2].max()])

    for _ in range(ROUNDS):
        distances, nearest = tree.query(a * scale + shift)
        kept = distances < np.percentile(distances, MATCH_SHARE)
        p, q = a[kept], b[nearest[kept]]
        mean_p, mean_q = p.mean(0), q.mean(0)
        scale = ((p - mean_p) * (q - mean_q)).sum() / ((p - mean_p) ** 2).sum()
        shift = mean_q - scale * mean_p

    print(f"HAIR skull fit: scale {scale:.3f}, {distances.mean() * 1000:.1f} mm apart on average")
    return scale, shift


def bind(points, positions, tris):
    """Each point's nearest triangle of skin: its three vertices and their weights."""
    a, b, c = positions[tris[:, 0]], positions[tris[:, 1]], positions[tris[:, 2]]
    _, tries = cKDTree((a + b + c) / 3).query(points, k=BIND_TRIES)
    ab, ac = b[tries] - a[tries], c[tries] - a[tries]
    to_point = points[:, None] - a[tries]
    normal = np.cross(ab, ac)
    area = np.maximum((normal * normal).sum(-1), 1e-18)
    v = np.clip((np.cross(to_point, ac) * normal).sum(-1) / area, 0, 1)
    w = np.clip((np.cross(ab, to_point) * normal).sum(-1) / area, 0, 1)
    total = np.where(v + w > 1, v + w, 1.0)
    v, w = v / total, w / total
    closest = a[tries] + ab * v[..., None] + ac * w[..., None]
    best = np.argmin(((closest - points[:, None]) ** 2).sum(-1), axis=1)
    rows = np.arange(len(points))
    weights = np.stack([1 - v[rows, best] - w[rows, best], v[rows, best], w[rows, best]], -1)
    return tris[tries[rows, best]], weights


def textures(source, out, ktx):
    """Writes the colour and normal textures; returns the colour's alpha. The colour's cut-out parts
    are filled from what is around them, so that the edges of its cards do not pick up its background."""
    image = Image.open(source / COLOUR_FILE).convert("RGBA").resize((COLOUR_SIZE, COLOUR_SIZE), Image.LANCZOS)
    pixels = np.asarray(image, dtype=np.float32) / 255
    filled = np.dstack([bleed(pixels[..., :3], pixels[..., 3] > 0.5), pixels[..., 3]])
    colour = out / "hair_color.png"
    Image.fromarray((np.clip(filled, 0, 1) * 255 + 0.5).astype(np.uint8)).save(colour)
    compress(ktx, colour, True, alpha=True)
    normal = out / "hair_normal.png"
    Image.open(source / NORMAL_FILE).convert("RGB").resize((COLOUR_SIZE, COLOUR_SIZE), Image.LANCZOS).save(normal)
    compress(ktx, normal, False)
    colour.unlink()
    normal.unlink()
    return pixels[..., 3]


def hair_samples(points, uv, starts, totals, alpha):
    """Points of the hair's cards (per loop) where its texture is not cut out."""
    tris = triangles(starts, totals)
    i, j = np.meshgrid(np.arange(HAIR_SAMPLES + 1), np.arange(HAIR_SAMPLES + 1))
    keep = i + j <= HAIR_SAMPLES
    a, b = (i[keep] / HAIR_SAMPLES)[None, :, None], (j[keep] / HAIR_SAMPLES)[None, :, None]

    def blend(values):
        return values[tris[:, 0]][:, None] * (1 - a - b) + values[tris[:, 1]][:, None] * a + values[tris[:, 2]][:, None] * b

    at, texture_at = blend(points).reshape(-1, 3), blend(uv).reshape(-1, 2)
    height, width = alpha.shape
    rows = np.clip(((1 - texture_at[:, 1]) % 1.0 * height).astype(int), 0, height - 1)
    columns = np.clip((texture_at[:, 0] % 1.0 * width).astype(int), 0, width - 1)
    return at[alpha[rows, columns] > 0.5]


def strands_from(field):
    """Noise drawn out into streaks along the field's gradient: where hair runs from its edge."""
    noise = np.random.default_rng(STRAND_SEED).random(field.shape, np.float32)
    grow_y, grow_x = np.gradient(gaussian_filter(field, 9.0))
    length = np.maximum(np.hypot(grow_x, grow_y), 1e-5)
    grow_x, grow_y = grow_x / length, grow_y / length
    y, x = np.mgrid[0 : field.shape[0], 0 : field.shape[1]].astype(np.float32)
    total = noise.copy()

    for sign in (1.0, -1.0):
        ay, ax = y.copy(), x.copy()

        for _ in range(STRAND_STEPS):
            turn_x = map_coordinates(grow_x, [ay, ax], order=1, mode="nearest")
            turn_y = map_coordinates(grow_y, [ay, ax], order=1, mode="nearest")
            ax, ay = ax + sign * turn_x, ay + sign * turn_y
            total += map_coordinates(noise, [ay, ax], order=0, mode="nearest")

    streaks = gaussian_filter(total, 0.6) / (2 * STRAND_STEPS + 1)
    return np.clip((streaks - streaks.mean()) / (streaks.std() * 3.2) + 0.5, 0, 1)


def outside_ears(skin, layout, scale):
    """Zero over the ears: cards above an ear project onto its top, where no hair grows."""
    y, x = np.mgrid[0:HAIRLINE_SIZE, 0:HAIRLINE_SIZE].astype(np.float32) / scale
    scan_x, scan_y = (x - layout.origin[0]) / layout.scale, (y - layout.origin[1]) / layout.scale
    out = np.ones_like(x)

    for cx, cy, rx, ry in skin["ears"]:
        out *= smooth(0.85, 1.1, ((scan_x - cx) / rx) ** 2 + ((scan_y - cy) / ry) ** 2)

    return out


def hairline(samples, skin, layout, positions, tris, texels, out, ktx):
    """Writes the hair's cover of the head."""
    scale = HAIRLINE_SIZE / TEXTURE
    filled, hit = rasterize(texels * scale, positions, tris, HAIRLINE_SIZE)
    front = smooth(*FOREHEAD_SIDES, np.abs(filled[..., 0])) * (filled[..., 1] < FOREHEAD_FRONT)
    above = SCALP_ABOVE + (FOREHEAD_TOP - SCALP_ABOVE) * front
    scalp = (filled[..., 2] > above) | ((filled[..., 2] > NAPE_ABOVE) & (filled[..., 1] > NAPE_BEHIND))
    rows, columns = np.nonzero(hit & scalp)
    distances, nearest = cKDTree(filled[rows, columns]).query(samples)
    cover = np.zeros_like(hit)
    near = nearest[distances < HAIR_REACH]
    cover[rows[near], columns[near]] = True
    closed = binary_closing(cover, iterations=HAIR_CLOSING)
    closed = binary_fill_holes(closed) & hit
    signed = distance_transform_edt(closed) - distance_transform_edt(~closed)
    base = smooth(-HAIR_OUTSIDE, HAIR_INSIDE, signed)
    # Solid well under the cards; before that a strand shows where its streak beats how far out it is,
    # fainter the further out, over the haze.
    strands = smooth(0.0, 0.3, base - (1 - strands_from(signed)) * 0.9) * smooth(0.0, 0.7, base)
    haze = smooth(-HAZE_REACH, HAIR_INSIDE, signed) * HAZE_STRENGTH
    mask = gaussian_filter(np.maximum(strands, haze), 0.7) * hit * outside_ears(skin, layout, scale)
    path = out / "hairline.png"
    Image.fromarray((np.clip(np.dstack([mask] * 3), 0, 1) * 255 + 0.5).astype(np.uint8)).save(path)
    compress(ktx, path, False)
    path.unlink()
    print(f"HAIR hairline: covers {closed.mean():.1%} of the mask")


def smooth_normals(points, indices):
    """Each point's normal, area weighted over the triangles around it."""
    faces = np.cross(points[indices[:, 1]] - points[indices[:, 0]], points[indices[:, 2]] - points[indices[:, 0]])
    sums = np.zeros_like(points)

    for corner in range(3):
        np.add.at(sums, indices[:, corner], faces)

    return sums / np.maximum(np.linalg.norm(sums, axis=1, keepdims=True), 1e-12)


def write_style(path, points, loops, uv, tris, triples, shares, base):
    """Writes style.hair: in the game's axes, a vertex per source vertex and UV, little-endian."""
    gltf = to_gltf(points)
    gltf_base = to_gltf(base)
    size = np.array([abs(gltf_base[a, axis] - gltf_base[b, axis]) / length for axis, (a, b, length) in enumerate(SCALES)])
    anchored = (shares[:, :, None] * gltf_base[triples]).sum(1)
    offsets = (gltf - anchored) / size
    keys = np.round(uv * 100000).astype(np.int64)
    _, unique, inverse = np.unique(np.c_[loops, keys], axis=0, return_index=True, return_inverse=True)
    inverse = inverse.ravel()
    sources = loops[unique]
    indices = inverse[tris]
    vertex_normals = smooth_normals(gltf, loops[tris])[sources]
    texture_uv = uv[unique] * np.array([1.0, -1.0]) + np.array([0.0, 1.0])

    with path.open("wb") as file:
        file.write(struct.pack("<4I", VERSION, len(points), len(sources), indices.size))

        for a, b, length in SCALES:
            file.write(struct.pack("<2If", a, b, length))

        file.write(triples.astype("<u4").tobytes())
        file.write(shares.astype("<f4").tobytes())
        file.write(offsets.astype("<f4").tobytes())
        file.write(gltf[sources].astype("<f4").tobytes())
        file.write(vertex_normals.astype("<f4").tobytes())
        file.write(texture_uv.astype("<f4").tobytes())
        file.write(sources.astype("<u4").tobytes())
        file.write(indices.astype("<u4").ravel().tobytes())

    print(f"HAIR wrote {path}: {len(sources)} vertices, {len(indices)} triangles")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pack", type=Path)
    parser.add_argument("hair")
    parser.add_argument("source", type=Path)
    parser.add_argument("skin", type=Path)
    parser.add_argument("mesh", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--ktx")
    parser.add_argument("--mesh-only", action="store_true")
    args = parser.parse_args()
    pack = np.load(args.pack)
    skin = json.loads(args.skin.read_text())
    mesh = np.load(args.mesh)
    uv, starts, totals, vertices, positions = (mesh[k] for k in ("uv", "starts", "totals", "vertices", "positions"))
    head = island_at(uv, uv_islands(uv, starts, totals), skin["head_uv_point"])
    ours = positions[np.unique(vertices[head])]
    scale, shift = fit_skull(pack["head"], ours)
    placed = pack["head"] * scale + shift
    _, nearest = cKDTree(ours).query(placed)
    offsets = ours[nearest] - placed
    points = pack[f"{args.hair}_points"] * scale + shift
    distances, around = cKDTree(placed).query(points, k=FIELD_POINTS)
    weights = np.exp(-((distances / FIELD_REACH) ** 2))
    weights /= np.maximum(weights.sum(1, keepdims=True), 1e-9)
    points = points + (offsets[around] * weights[..., None]).sum(1)
    head_tris = triangles(starts[head[starts]], totals[head[starts]])
    triples, shares = bind(points, positions, vertices[head_tris])
    out = args.out / args.hair
    out.mkdir(parents=True, exist_ok=True)
    hair_starts, hair_totals = pack[f"{args.hair}_starts"], pack[f"{args.hair}_totals"]
    hair_loops, hair_uv = pack[f"{args.hair}_loops"], pack[f"{args.hair}_uv"]

    if not args.mesh_only:
        ktx = find_ktx(args.ktx)
        alpha = textures(args.source, out, ktx)
        samples = hair_samples(points[hair_loops], hair_uv, hair_starts, hair_totals, alpha)
        layout = Layout(skin)
        hairline(samples, skin, layout, positions[vertices], head_tris, layout.texels(layout.scan(uv)), out, ktx)

    write_style(out / "style.hair", points, hair_loops, hair_uv, triangles(hair_starts, hair_totals), triples, shares, positions)


main()
