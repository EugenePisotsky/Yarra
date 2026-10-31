"""Bakes a bought eye pack's textures for the character's eyes.

uv run --with numpy --with scipy --with pillow --with tifffile python eye_bake.py \
  PACK_DIR IRIS MAP.npz OUT_DIR [--ktx PATH]

PACK_DIR is the EyePack folder, IRIS the number of its Realistic iris, MAP what eye_mesh.py wrote.
OUT_DIR gets the iris's colour, the sclera's colour and normal, the wet layer's and a lash strand's
(fit_parts.py), as KTX2 with mipmaps.

The wet layer lies over the whole front of the eye, held by the lids rather than the eyeball. Its
alpha is the shade the lids throw, most of why an eye without it reads as a doll's, and a soft dark
ring where iris meets sclera, which no real eye has a hard edge at. Its colour is a faint reflection
of the sky, which the lighting scales with the scene.
"""
import argparse
import sys
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).parent))

from eye_shape import IRIS_SHARE
from skin_bake import compress, find_ktx

Image.MAX_IMAGE_PIXELS = None

IRIS_SIZE, SCLERA_SIZE, WET_SIZE = 1024, 1024, 512

# Sclera is never paper white: a warm grey under the veins.
SCLERA_TONE = np.array([0.80, 0.76, 0.72])

# A bought iris is a macro photograph, far more saturated and contrasty than an eye reads at any
# real distance: its colour is scaled and desaturated.
IRIS_LEVEL, IRIS_SATURATION = 0.85, 0.8

# The inner corner of the eye, where the sclera gives way to the caruncle's pink, on the map's +U
# side (fit_parts.py turns each eye's map to the nose): angles from the front, the turn round the
# eye it spreads over (as cosines), and its colour.
CORNER_ANGLES = (38.0, 58.0)
CORNER_SPREAD = (0.55, 0.95)
CORNER_COLOUR = np.array([0.72, 0.42, 0.40])

# The wet layer's shade: its heights and sides as shares of the eye's radius from the centre, and
# how dark each gets. The upper lid throws the most.
UPPER_SHADE = (0.08, 0.32, 0.72)
LOWER_SHADE = (0.34, 0.62, 0.30)
SIDE_SHADE = (0.40, 0.80, 0.55)

# The dark ring at the limbus: its width as a share of the eye's radius, and its darkness.
LIMBUS_WIDTH, LIMBUS_DARK = 0.05, 0.72

# A lash strand's texture: its colour, where its edge fades across (shares of its half width) and how
# much of its length tapers out at the tip. Its root is at the image's bottom. Solid but for the edge:
# a soft profile averages to half alpha in the mipmaps, and lashes go pale a metre away.
LASH_SIZE = 64
LASH_COLOUR = np.array([0.03, 0.022, 0.018])
LASH_EDGE, LASH_TIP = (1.0, 0.72), 0.3

# The reflection's strength at the top of the eye, as linear reflected colour, and where it fades.
REFLECTION = np.array([0.007, 0.008, 0.010])
REFLECTION_FADE = (-0.1, 0.5)


def smooth(low, high, value):
    t = np.clip((value - low) / (high - low), 0, 1)
    return t * t * (3 - 2 * t)


def to_srgb(linear):
    return np.where(linear <= 0.0031308, linear * 12.92, 1.055 * np.maximum(linear, 1e-6) ** (1 / 2.4) - 0.055)


def wet_layer(size):
    y, x = np.mgrid[0:size, 0:size].astype(np.float32)
    across = (x + 0.5) / size * 2 - 1
    up = 1 - (y + 0.5) / size * 2
    upper = smooth(UPPER_SHADE[0], UPPER_SHADE[1], up) * UPPER_SHADE[2]
    lower = smooth(LOWER_SHADE[0], LOWER_SHADE[1], -up) * LOWER_SHADE[2]
    side = smooth(SIDE_SHADE[0], SIDE_SHADE[1], np.abs(across)) * SIDE_SHADE[2]
    limbus = np.exp(-(((np.hypot(across, up) - IRIS_SHARE) / LIMBUS_WIDTH) ** 2)) * LIMBUS_DARK
    alpha = 1 - (1 - upper) * (1 - lower) * (1 - side) * (1 - limbus)
    reflected = smooth(REFLECTION_FADE[0], REFLECTION_FADE[1], up)[..., None] * REFLECTION
    return np.dstack([to_srgb(reflected), alpha])


def lash_strand():
    y, x = np.mgrid[0:LASH_SIZE, 0:LASH_SIZE].astype(np.float32)
    across = np.abs((x + 0.5) / LASH_SIZE - 0.5) * 2
    along = (y + 0.5) / LASH_SIZE
    alpha = (1 - smooth(LASH_EDGE[1], LASH_EDGE[0], across)) * smooth(0.0, LASH_TIP, along)
    return np.dstack([np.full_like(alpha, value) for value in LASH_COLOUR] + [alpha])


def save(image, path, ktx, srgb, alpha=False):
    Image.fromarray((np.clip(image, 0, 1) * 255 + 0.5).astype(np.uint8)).save(path)
    compress(ktx, path, srgb, alpha)


def load(path, size):
    picture = Image.open(path).convert("RGB").resize((size, size), Image.LANCZOS)
    return np.asarray(picture, dtype=np.float32) / 255


def iris_colour(path):
    image = load(path, IRIS_SIZE)
    grey = image.mean(-1, keepdims=True)
    return (grey + (image - grey) * IRIS_SATURATION) * IRIS_LEVEL


def sclera_colour(path, table):
    """The sclera's map at its tone, with the inner corner's pink painted in."""
    image = load(path, SCLERA_SIZE) * SCLERA_TONE
    y, x = np.mgrid[0:SCLERA_SIZE, 0:SCLERA_SIZE].astype(np.float32)
    across = (x + 0.5) / SCLERA_SIZE - 0.5
    up = 0.5 - (y + 0.5) / SCLERA_SIZE
    radius = np.hypot(across, up)
    angle = np.interp(radius, table["uv_radius"], table["angles"])
    towards = across / np.maximum(radius, 1e-5)
    corner = smooth(*CORNER_ANGLES, angle) * smooth(*CORNER_SPREAD, towards)
    return image * (1 - corner[..., None]) + CORNER_COLOUR * corner[..., None]


def find_map(folder, kind):
    return next(path for path in folder.iterdir() if kind in path.name.lower())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pack", type=Path)
    parser.add_argument("iris")
    parser.add_argument("map", type=Path)
    parser.add_argument("out", type=Path)
    parser.add_argument("--ktx")
    args = parser.parse_args()
    ktx = find_ktx(args.ktx)
    args.out.mkdir(parents=True, exist_ok=True)
    iris = args.pack / "Realistic" / args.iris
    other = args.pack / "Other Textures"
    table = np.load(args.map)
    save(iris_colour(find_map(iris, "diffuse")), args.out / "eye_iris_color.png", ktx, True)
    sclera = sclera_colour(other / "Sclera-Diffuse.jpg", table)
    save(sclera, args.out / "eye_sclera_color.png", ktx, True)
    save(load(other / "Sclera_normal.jpg", SCLERA_SIZE), args.out / "eye_sclera_normal.png", ktx, False)
    save(wet_layer(WET_SIZE), args.out / "eye_wet.png", ktx, True, alpha=True)
    save(lash_strand(), args.out / "eye_lash.png", ktx, True, alpha=True)
    print("EYE_DONE")


if __name__ == "__main__":
    main()
