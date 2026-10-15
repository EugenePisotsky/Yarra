"""Turn a Megascans fabric surface into a cloth fabric: base colour, normal and roughness as
KTX2 with mipmaps (Bevy makes none for PNGs, and a weave without them shimmers), and
`fabric.json` with the size one tile covers. Megascans normals here are green-up, as Bevy's.

    python3 tools/cloth/import_fabric.py \
        ~/Downloads/fabric_clothes_cotton_polyester_twill_weave_black_xkskbia_4k \
        assets/local/fabrics/black_twill [--size 2048] [--albedo 0.03]

`--albedo` rescales the base colour to that mean linear luminance, hue kept.
Needs Pillow and Khronos ktx (on PATH, or the project's copy, or `--ktx`).
"""
import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]


def decode(c):
    c /= 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def encode(x):
    x = min(max(x, 0.0), 1.0)
    c = x * 12.92 if x <= 0.0031308 else 1.055 * x ** (1 / 2.4) - 0.055
    return round(c * 255.0)


def find(source, suffix):
    found = sorted(source.glob(f"*_{suffix}.jpg")) + sorted(source.glob(f"*_{suffix}.png"))
    if not found:
        raise SystemExit(f"{source}: no *_{suffix} map")
    return found[0]


def tile_size(source):
    """Metres one tile covers, from the surface's metadata."""
    for path in source.glob("*.json"):
        info = json.loads(path.read_text())
        for m in info.get("maps", []):
            if m.get("type") == "basecolor" and m.get("physicalSize"):
                return float(m["physicalSize"].split("x")[0])
        for m in info.get("meta", []):
            if m.get("key") == "scanArea":
                return float(m["value"].split("x")[0])
    raise SystemExit(f"{source}: no physical size in its metadata")


def ktx_create(ktx, png, output, srgb, normal):
    cmd = [str(ktx), "create", "--format", "R8G8B8A8_SRGB" if srgb else "R8G8B8A8_UNORM"]
    cmd += ["--assign-tf", "srgb" if srgb else "linear", "--encode", "uastc"]
    cmd += ["--uastc-quality", "2", "--zstd", "8", "--threads", "4", "--generate-mipmap"]
    if normal:
        cmd += ["--normalize"]
    subprocess.run(cmd + [str(png), str(output)], check=True)
    subprocess.run([str(ktx), "validate", str(output)], check=True, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--size", type=int, default=2048)
    parser.add_argument("--albedo", type=float)
    parser.add_argument(
        "--ktx",
        type=Path,
        default=Path(shutil.which("ktx") or ROOT.parent / "yarra/.tools/ktx/4.4.2/bin/ktx"),
    )
    args = parser.parse_args()
    if not args.ktx.exists():
        parser.error("Khronos ktx was not found; install it on PATH or pass --ktx")
    size = (args.size, args.size)
    color = Image.open(find(args.source, "BaseColor")).convert("RGB").resize(size, Image.LANCZOS)
    linear = [decode(float(c)) for c in range(256)]
    if args.albedo:
        # Mean linear luminance over a thumbnail, then one scale for every channel.
        r, g, b = (list(ch.getdata()) for ch in color.resize((256, 256), Image.BOX).split())
        mean = sum(0.2126 * linear[x] + 0.7152 * linear[y] + 0.0722 * linear[z] for x, y, z in zip(r, g, b))
        mean /= len(r)
        scale = args.albedo / max(mean, 1e-6)
        color = color.point([encode(linear[c % 256] * scale) for c in range(768)])
        print(f"albedo {mean:.4f} -> {args.albedo:.4f} (x{scale:.2f})")
    normal = Image.open(find(args.source, "Normal")).convert("RGB").resize(size, Image.LANCZOS)
    rough = Image.open(find(args.source, "Roughness")).convert("L").resize(size, Image.LANCZOS)
    # Bevy reads roughness from green and metalness from blue.
    black = Image.new("L", size, 0)
    metallic_roughness = Image.merge("RGB", (black, rough, black))
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        for name, image, srgb, is_normal in (
            ("color", color, True, False),
            ("normal", normal, False, True),
            ("roughness", metallic_roughness, False, False),
        ):
            png = temp / f"{name}.png"
            image.convert("RGBA").save(png)
            ktx_create(args.ktx, png, args.output / f"{name}.ktx2", srgb, is_normal)
    tile = tile_size(args.source)
    (args.output / "fabric.json").write_text(
        json.dumps({"tile": tile, "source": args.source.name}, indent=2) + "\n"
    )
    print(f"wrote {args.output}: {args.size}² maps, tile {tile} m")


if __name__ == "__main__":
    main()
