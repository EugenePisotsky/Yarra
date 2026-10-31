"""Bakes a scanned face into a character's skin textures.

uv run --with numpy --with scipy --with pillow --with tifffile python skin_bake.py \
  SKIN.json SCAN_DIR ATLAS.png MESH.npz OUT_DIR [--ktx PATH] [--cache DIR]

SKIN is a skin file (content/characters/skins/), SCAN_DIR a Texturing.XYZ multichannel face,
ATLAS MakeHuman's skin atlas for the body, MESH what head_mesh.py writes. OUT_DIR gets the head's
colour, normal and occlusion-roughness-metal textures and the body's colour, as KTX2 with mipmaps
(the PNGs beside them are for looking at):

- the scan's hair goes first, in colour and height: inside each `hair` area, and wherever else
  its box is darker than its surroundings, the broad tone is drawn in from the skin around and
  the fine detail copied from clean skin at `donor`, scaled by `detail`;
- the scan's colour is calibrated: a scan's overall level is arbitrary, so `tone` names patches of
  it (x, y, radius) and the linear colour they should average;
- the body takes the atlas, its tone matched to the scan's on the `tone` patches. The head takes the
  scan's broad colour inside `face_outline` and its fine detail inside `skin_outline`, plain skin
  elsewhere (the atlas paints stubble over the scalp, a bald head's business, not this layer's), and
  near the seam with the body the body's own colour there, fading out over SEAM_FADE of real
  distance, so head and body meet in one colour wherever the seam runs;
- the normal map comes from the scan's mid and fine height channels and the broad one's detail
  (the mesh has the broad shape); occlusion from the height's creases; roughness from a base,
  `gloss` areas and the creases.

--cache keeps the scan with its hair removed, the slow part, between runs.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
import tifffile
from PIL import Image, ImageDraw
from scipy.ndimage import binary_closing, binary_dilation, gaussian_filter, map_coordinates
from scipy.spatial import cKDTree

sys.path.insert(0, str(Path(__file__).parent))

from head_layout import TEXTURE, Layout, island_at, uv_islands

Image.MAX_IMAGE_PIXELS = None

KTX_VERSION = "4.4.2"

# The feather inside the skin outline and inside the scan's edge, and the broader one of the face's
# broad colour, scan pixels.
FEATHER = 260.0
FACE_FEATHER = 700.0

# Where the head's skin turns into the mouth and the eye sockets the lips and lids' inner edges lie
# in shade no shadow map resolves: how far from that edge it reaches, metres, how much light they
# lose, and how rough they get.
CAVITY_EDGE = 0.0035
CAVITY_SHADE = 0.8
CAVITY_ROUGHNESS = 0.9

# Normal strength per texel of height slope, and how much a crease darkens and roughens.
NORMAL_STRENGTH = 2.5
CAVITY_DARKEN = 1.2
CAVITY_ROUGHEN = 0.4

BASE_ROUGHNESS = 0.5

# The share of the scan's detail kept when the map shrinks: the broad channel's blur radius.
BROAD_DETAIL = 14.0

# How far from the seam with the body the body's colour reaches into the head, metres, and how many
# of the seam's vertices share in each point's colour there.
SEAM_FADE = 0.045
SEAM_NEIGHBOURS = 8

# The blur that parts the scan's broad colour from its fine detail, in the head texture's texels.
DETAIL_BLUR = 28.0

# The texture sizes: the head's occlusion-roughness map and the body's colour.
HEAD_ORM = 2048
BODY_COLOR = 2048


def ellipse(shape, spec, origin=(0, 0)):
    cx, cy, rx, ry, turn = spec
    y, x = np.mgrid[origin[1] : origin[1] + shape[0], origin[0] : origin[0] + shape[1]].astype(np.float32)
    c, s = np.cos(turn), np.sin(turn)
    dx, dy = x - cx, y - cy
    return ((dx * c + dy * s) / rx) ** 2 + ((-dx * s + dy * c) / ry) ** 2


def per_channel(image, sigma):
    return np.stack([gaussian_filter(image[..., c], sigma) for c in range(image.shape[-1])], -1)


def drawn_in(image, keep):
    """The image's broad tone inside the holes, from what `keep` holds: coarse scales fill the
    middles, fine ones the edges."""
    out = None

    for sigma in (200, 70, 25):
        weight = gaussian_filter(keep.astype(np.float32), sigma)
        tone = per_channel(image * keep[..., None], sigma) / np.maximum(weight[..., None], 1e-4)
        reach = np.clip((weight - 0.02) / 0.2, 0, 1)[..., None]
        out = tone if out is None else out * (1 - reach) + tone * reach

    return out


def remove_hair(colour, height, areas):
    lightness = colour.mean(-1)

    for area in areas:
        x0, y0, x1, y1 = area["box"]
        box = (y1 - y0, x1 - x0)
        dark = gaussian_filter(lightness[y0:y1, x0:x1], 60) - lightness[y0:y1, x0:x1] > area["dark"]
        dark = binary_dilation(binary_closing(dark, iterations=3), iterations=6)
        hole = (ellipse(box, area["ellipse"], (x0, y0)) < 1) | dark
        soft = np.clip(gaussian_filter(hole.astype(np.float32), 10) * 1.6, 0, 1)[..., None]
        dx, dy = area["donor"]

        for image in (colour, height):
            here = image[y0:y1, x0:x1].copy()
            donor = image[y0 + dy : y1 + dy, x0 + dx : x1 + dx]
            detail = (donor - per_channel(donor, 25)) * area["detail"]
            image[y0:y1, x0:x1] = here * (1 - soft) + (drawn_in(here, ~hole) + detail) * soft

        print(f"SKIN {area['name']}: {hole.mean():.0%} of its box filled")


def outline_mask(skin, outline, feather=FEATHER):
    """Where the scan is inside the outline, fading to nothing at it and at the scan's edge."""
    width, height = skin["scan_size"]
    down = 8
    image = Image.new("L", (width // down, height // down), 0)
    ImageDraw.Draw(image).polygon([(x / down, y / down) for x, y in outline], fill=255)
    inside = np.asarray(image, dtype=np.float32) / 255
    # Black all round, so the blur also fades at the scan's edge.
    inside[0, :] = inside[-1, :] = 0
    inside[:, 0] = inside[:, -1] = 0
    blurred = gaussian_filter(inside, feather / down / 2, mode="constant")
    fade = np.clip((blurred - 0.5) / 0.48, 0, 1)
    return fade * fade * (3 - 2 * fade)


def triangles(starts, totals):
    corners = []

    for start, total in zip(starts, totals):
        corners.extend((start, start + i, start + i + 1) for i in range(1, total - 1))

    return np.array(corners)


def rasterize(texels, values, tris, size):
    """Values (per loop) interpolated over the triangles at their texels, and where any landed."""
    out = np.zeros((size, size, values.shape[1]), np.float32)
    hit = np.zeros((size, size), bool)

    for a, b, c in tris:
        pa, pb, pc = texels[a], texels[b], texels[c]
        low = np.floor(np.minimum(np.minimum(pa, pb), pc)).astype(int)
        high = np.ceil(np.maximum(np.maximum(pa, pb), pc)).astype(int) + 1
        low, high = np.clip(low, 0, size), np.clip(high, 0, size)

        if (high <= low).any():
            continue

        y, x = np.mgrid[low[1] : high[1], low[0] : high[0]]
        area = (pb[0] - pa[0]) * (pc[1] - pa[1]) - (pb[1] - pa[1]) * (pc[0] - pa[0])

        if abs(area) < 1e-6:
            continue

        px, py = x + 0.5, y + 0.5
        wb = ((px - pa[0]) * (pc[1] - pa[1]) - (py - pa[1]) * (pc[0] - pa[0])) / area
        wc = ((pb[0] - pa[0]) * (py - pa[1]) - (pb[1] - pa[1]) * (px - pa[0])) / area
        wa = 1 - wb - wc
        inside = (wa >= -0.002) & (wb >= -0.002) & (wc >= -0.002)
        out[y[inside], x[inside]] = (
            wa[inside, None] * values[a] + wb[inside, None] * values[b] + wc[inside, None] * values[c]
        )
        hit[y[inside], x[inside]] = True

    return out, hit


def bleed(image, hit):
    """Fills the unused texels from the used, so that mipmaps do not pull in black at the edges."""
    weight = hit.astype(np.float32)
    out = image.copy()
    missing = ~hit

    for sigma in (4, 16, 64, 256):
        tone = per_channel(image * weight[..., None], sigma) / np.maximum(
            gaussian_filter(weight, sigma)[..., None], 1e-5
        )
        reached = gaussian_filter(weight, sigma) > 1e-3
        out[missing & reached] = tone[missing & reached]
        missing &= ~reached

    return out


def scan_to_texture(image, layout):
    """The scan's pixels at the texture's scale, in place in a blank texture."""
    width, height = (layout.size * layout.scale).round().astype(int)
    x, y = layout.origin.round().astype(int)
    out = np.zeros((TEXTURE, TEXTURE, image.shape[-1]), np.float32)

    for channel in range(image.shape[-1]):
        small = Image.fromarray(image[..., channel]).resize((width, height), Image.BOX)
        out[y : y + height, x : x + width, channel] = np.asarray(small)

    return out, (x, y, width, height)


def mask_to_texture(mask, layout):
    y, x = np.mgrid[0:TEXTURE, 0:TEXTURE].astype(np.float32)
    down = mask.shape[1] / layout.size[0]
    return map_coordinates(
        mask,
        [(y + 0.5 - layout.origin[1]) / layout.scale * down, (x + 0.5 - layout.origin[0]) / layout.scale * down],
        order=1,
        mode="constant",
    )


def atlas_sampler(atlas):
    size = atlas.shape[0]

    def sample(uv):
        row, column = (1 - uv[:, 1]) * size - 0.5, uv[:, 0] * size - 0.5
        return np.stack([map_coordinates(atlas[..., c], [row, column], order=1, mode="nearest") for c in range(3)], -1)

    return sample


def save_png(image, path, srgb_size=None):
    data = (np.clip(image, 0, 1) * 255 + 0.5).astype(np.uint8)
    picture = Image.fromarray(data)

    if srgb_size:
        picture = picture.resize((srgb_size, srgb_size), Image.BOX)

    picture.save(path)
    print(f"SKIN wrote {path} {path.stat().st_size / 1e6:.1f} MB")


def find_ktx(explicit):
    repository = Path(__file__).resolve().parents[3]
    candidates = (
        explicit,
        os.environ.get("KTX_BIN"),
        repository.parent / "yarra" / ".tools" / "ktx" / KTX_VERSION / "bin" / "ktx",
        shutil.which("ktx"),
    )

    for candidate in candidates:
        if candidate and Path(candidate).is_file():
            return str(candidate)

    raise RuntimeError(f"Khronos ktx {KTX_VERSION} was not found; pass --ktx or set KTX_BIN")


def compress(ktx, source, srgb, alpha=False):
    pending = source.with_suffix(".ktx2.pending")
    format_name = f"R8G8B8{'A8' if alpha else ''}_{'SRGB' if srgb else 'UNORM'}"
    subprocess.run(
        [ktx, "create", "--format", format_name, "--generate-mipmap", "--encode", "uastc",
         "--uastc-quality", "1", "--zstd", "8", "--assign-tf", "srgb" if srgb else "linear",
         str(source), str(pending)],
        check=True,
    )
    pending.replace(source.with_suffix(".ktx2"))
    print(f"SKIN compressed {source.with_suffix('.ktx2')} {source.with_suffix('.ktx2').stat().st_size / 1e6:.1f} MB")


def to_linear(srgb):
    return np.where(srgb <= 0.04045, srgb / 12.92, ((srgb + 0.055) / 1.055) ** 2.4)


def to_srgb(linear):
    linear = np.clip(linear, 0, 1)
    return np.where(linear <= 0.0031308, linear * 12.92, 1.055 * linear ** (1 / 2.4) - 0.055)


def calibrate(colour, tone):
    """The scan at the skin file's tone, in place."""
    samples = [colour[y - r : y + r, x - r : x + r].reshape(-1, 3) for x, y, r in tone["reference"]]
    mean = to_linear(np.concatenate(samples)).mean(0)
    gain = np.array(tone["linear"], np.float32) / mean
    print(f"SKIN scan level {np.round(mean, 3)}, gain {np.round(gain, 2)}")

    for start in range(0, colour.shape[0], 256):
        colour[start : start + 256] = to_srgb(to_linear(colour[start : start + 256]) * gain)


def patch_mean(image, layout, patches):
    """A texture's mean colour over patches named in scan pixels."""
    samples = []

    for x, y, radius in patches:
        centre = layout.origin + np.array([x, y]) * layout.scale
        reach = max(int(radius * layout.scale), 1)
        cx, cy = centre.astype(int)
        samples.append(image[cy - reach : cy + reach, cx - reach : cx + reach].reshape(-1, 3))

    return np.concatenate(samples).mean(0)


def seam_colours(mesh, head, body, atlas, gain):
    """Per head loop, the body's colour at the nearest stretch of their seam, and how far that is."""
    vertices, positions, uv = mesh["vertices"], mesh["positions"], mesh["uv"]
    seam = np.intersect1d(vertices[head], vertices[body])
    # A little blur, so a seam vertex does not take the colour of one speck.
    soft = per_channel(atlas, 6)
    sample = atlas_sampler(soft)
    colours = {}
    body_loops = np.flatnonzero(body)
    on_seam = body_loops[np.isin(vertices[body_loops], seam)]
    sampled = sample(uv[on_seam]) * gain

    for vertex, colour in zip(vertices[on_seam], sampled):
        colours.setdefault(int(vertex), []).append(colour)

    seam_colour = np.array([np.mean(colours[int(vertex)], axis=0) for vertex in seam])
    distances, nearest = cKDTree(positions[seam]).query(positions[vertices], k=SEAM_NEIGHBOURS)
    weights = 1 / np.maximum(distances, 1e-4) ** 2
    weights /= weights.sum(1, keepdims=True)
    colour = (seam_colour[nearest] * weights[..., None]).sum(1)
    print(f"SKIN seam with the body: {len(seam)} vertices")
    return colour, distances[:, 0]


def cavity_edges(skin, mesh, head, islands):
    """Per loop, how near it lies to where the head's skin joins the mouth's or a socket's inside."""
    vertices, positions, uv = mesh["vertices"], mesh["positions"], mesh["uv"]
    cavity = np.zeros_like(head)

    for point in skin["cavity_uv_points"]:
        cavity |= island_at(uv, islands, point)

    edge = np.intersect1d(vertices[head], vertices[cavity])
    distances, _ = cKDTree(positions[edge]).query(positions[vertices])
    share = np.clip(1 - distances / CAVITY_EDGE, 0, 1)
    print(f"SKIN edges of the mouth and sockets: {len(edge)} vertices")
    return share * share * (3 - 2 * share)


def bake_colour(skin, layout, colour, atlas, mesh):
    uv, starts, totals = mesh["uv"], mesh["starts"], mesh["totals"]
    islands = uv_islands(uv, starts, totals)
    head = island_at(uv, islands, skin["head_uv_point"])
    others, counts = np.unique(islands[~head], return_counts=True)
    body = islands == others[np.argmax(counts)]
    tris = triangles(starts[head[starts]], totals[head[starts]])
    texels = layout.texels(layout.scan(uv))
    scan, _ = scan_to_texture(colour, layout)
    rect = scan.sum(-1) > 0
    filled, hit = rasterize(texels, uv, tris, TEXTURE)
    used = np.zeros((TEXTURE, TEXTURE, 3), np.float32)
    sample = atlas_sampler(atlas)

    for start in range(0, TEXTURE, 512):
        used[start : start + 512] = sample(filled[start : start + 512].reshape(-1, 2)).reshape(-1, TEXTURE, 3)

    gain = patch_mean(scan, layout, skin["tone"]["reference"]) / patch_mean(used, layout, skin["tone"]["reference"])
    seam_colour, seam_distance = seam_colours(mesh, head, body, atlas, gain)
    near_seam = np.clip(1 - seam_distance / SEAM_FADE, 0, 1)
    near_seam = near_seam * near_seam * (3 - 2 * near_seam)
    near_cavity = cavity_edges(skin, mesh, head, islands)
    seam, _ = rasterize(texels, np.c_[seam_colour, near_seam, near_cavity], tris, TEXTURE)
    seam_share, cavity = seam[..., 3:4], seam[..., 4]
    detail = mask_to_texture(outline_mask(skin, skin["skin_outline"]), layout)[..., None] * (1 - seam_share)
    face = mask_to_texture(outline_mask(skin, skin["face_outline"], FACE_FEATHER), layout)[..., None]
    face = np.minimum(face * (1 - seam_share), detail)
    # Broad and fine apart: the scan's fine detail runs on down the neck, past its broad colour.
    weight = gaussian_filter(rect.astype(np.float32), DETAIL_BLUR)[..., None]
    broad = per_channel(scan * rect[..., None], DETAIL_BLUR) / np.maximum(weight, 1e-3)
    fine = (scan - broad) * rect[..., None]
    edge = (face[..., 0] > 0.2) & (face[..., 0] < 0.9) & hit
    plain = broad[edge].mean(0)
    print(f"SKIN atlas tone gain {np.round(gain, 3)}, plain skin {np.round(plain, 3)}")
    outside = seam[..., :3] * seam_share + plain * (1 - seam_share)
    head_colour = (outside * (1 - face) + broad * face + fine * detail) * (1 - CAVITY_SHADE * cavity[..., None])
    return bleed(head_colour, hit), np.clip(atlas * gain, 0, 1), hit, detail[..., 0], cavity


def bake_surface(skin, layout, height, mask, hit, cavity):
    broad, mid, fine = (height[..., c] for c in range(3))
    relief = broad - gaussian_filter(broad, BROAD_DETAIL * 2) + (mid - 0.5) + (fine - 0.5)
    small, _ = scan_to_texture(relief[..., None], layout)
    small = small[..., 0] * mask
    slope_y, slope_x = np.gradient(small)
    x, y = -slope_x * NORMAL_STRENGTH, slope_y * NORMAL_STRENGTH
    length = np.sqrt(x * x + y * y + 1)
    normal = np.stack([x / length, y / length, 1 / length], -1) * 0.5 + 0.5
    crease = np.minimum(small - gaussian_filter(small, 6), 0.0)
    occlusion = (1 + crease * CAVITY_DARKEN) * (1 - CAVITY_SHADE * cavity)
    gloss = np.full((TEXTURE, TEXTURE), BASE_ROUGHNESS, np.float32)
    grid_y, grid_x = np.mgrid[0:TEXTURE, 0:TEXTURE].astype(np.float32)
    scan_x = (grid_x - layout.origin[0]) / layout.scale
    scan_y = (grid_y - layout.origin[1]) / layout.scale

    for area in skin["gloss"]:
        cx, cy, rx, ry, _ = area["ellipse"]
        fall = np.exp(-(((scan_x - cx) / rx) ** 2 + ((scan_y - cy) / ry) ** 2) * 1.5)
        gloss += area["roughness"] * fall * mask

    roughness = gloss - crease * CAVITY_ROUGHEN
    roughness += (CAVITY_ROUGHNESS - roughness) * cavity
    orm = np.stack([occlusion, roughness, np.zeros_like(roughness)], -1)
    return normal, bleed(orm, hit)


def clean_scan(folder, skin, cache):
    """The scan's colour and height with the hair removed."""
    if cache and (cache / "colour.npy").exists():
        return np.load(cache / "colour.npy"), np.load(cache / "height.npy")

    colour = tifffile.imread(folder / "fullface/face_CL_albedo.tif").astype(np.float32) / 65535
    height = tifffile.imread(folder / "fullface/fullface_CP_displacement.tif").astype(np.float32) / 65535
    remove_hair(colour, height, skin["hair"])

    if cache:
        cache.mkdir(parents=True, exist_ok=True)
        np.save(cache / "colour.npy", colour)
        np.save(cache / "height.npy", height)

    return colour, height


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("skin", type=Path)
    parser.add_argument("scan", type=Path)
    parser.add_argument("atlas", type=Path)
    parser.add_argument("mesh", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--ktx")
    parser.add_argument("--cache", type=Path)
    args = parser.parse_args()
    skin = json.loads(args.skin.read_text())
    ktx = find_ktx(args.ktx)
    args.out.mkdir(parents=True, exist_ok=True)
    colour, height = clean_scan(args.scan, skin, args.cache)
    calibrate(colour, skin["tone"])
    atlas = np.asarray(Image.open(args.atlas).convert("RGB"), dtype=np.float32) / 255
    mesh = np.load(args.mesh)
    layout = Layout(skin)
    head_colour, body_colour, hit, mask, cavity = bake_colour(skin, layout, colour, atlas, mesh)
    normal, orm = bake_surface(skin, layout, height, mask, hit, cavity)
    outputs = {
        "head_color": (head_colour, None, True),
        "head_normal": (normal, None, False),
        "head_orm": (orm, HEAD_ORM, False),
        "body_color": (body_colour, BODY_COLOR, True),
    }

    for name, (image, size, srgb) in outputs.items():
        path = args.out / f"{name}.png"
        save_png(image, path, size)
        compress(ktx, path, srgb)

    print("SKIN_DONE")


if __name__ == "__main__":
    main()
