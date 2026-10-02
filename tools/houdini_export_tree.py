#!/usr/bin/env hython
"""Export trees from a Houdini tree scene as game-ready glTF LODs and an asset catalog.

Run with Houdini's Python (Indie and Apprentice licences both allow it):

    hython tools/houdini_export_tree.py SCENE.hiplc assets/local/yarra_trees \
        --catalog assets/packs/yarra_trees/trees.catalog.ron \
        --variant broadleaf_a:1:15.5:7.5:5 --variant broadleaf_b:2:12:6.5:4

The scene object (--object, /obj/FOREST_TREE by default) needs a CONTROLS node (seed,
height, crown_width, crown_base, lod and texture paths), a BAKE_* node that renders the
leaf card atlas, and an OUT_TREE node producing triangles with point P, N, Cd, uv2,
vertex uv and a primitive `mat` of "bark" or "leaf". Each --variant NAME:SEED:HEIGHT:WIDTH[:CROWN_BASE]
is cooked at LOD0-3 and written to OUTPUT/runtime/NAME/NAME_lodK.gltf. Textures are
written once to OUTPUT/runtime/textures as mipmapped UASTC KTX2; the leaf atlas is the
branch cluster bake (BAKE_CLUSTERS) and its colour mips keep their alpha coverage so
foliage does not thin out with distance. The scene is copied to
OUTPUT/source/ because the asset importer requires a local source file.

Foliage materials carry the engine's wind contract (`yarra_wind`, TEXCOORD_1 =
flutter, branch) and `yarra_shading`: "crown_v1" (normals come from the crown and are
shared by both sides of a card) or, with CONTROLS shading_mode 1, "pad_v1" (flat pads
whose undersides face down). COLOR_0 is crown occlusion for indirect light only.
CONTROLS may set leaf_luminance, bark_luminance and bark_upper_luminance: the exporter
scales those textures in linear space to that mean luminance (the kit trees sit near 0.18),
after multiplying the leaf atlas by an optional leaf_tint colour.
When CONTROLS has bark_upper_* textures, the bark material also carries `yarra_bark:
"blend_v1"`: COLOR_0 alpha (the scene's point Alpha) blends from the main bark to that
upper bark, whose textures the extras list by asset path.
"""
import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import hou
import numpy as np
from PIL import Image

KTX_VERSION = "4.4.2"
LEAF_SIZE = 2048
BARK_MAX = 2048  # longest side of a bark tile; scans keep their aspect ratio


def find_ktx(explicit):
    repository = Path(__file__).resolve().parents[1]
    for candidate in (explicit, repository.parent / "yarra" / ".tools" / "ktx" / KTX_VERSION / "bin" / "ktx",
                      shutil.which("ktx")):
        if candidate and Path(candidate).is_file():
            return Path(candidate)
    sys.exit(f"Khronos ktx {KTX_VERSION} was not found; pass --ktx")


# ------------------------------------------------------------------ textures
def bleed(rgb, alpha, iterations=24):
    """Spread colour into transparent texels so filtering never pulls in the background."""
    rgb = rgb.copy()
    known = alpha > 0.02
    for _ in range(iterations):
        if known.all():
            break
        acc = np.zeros_like(rgb)
        cnt = np.zeros(known.shape, dtype=np.float32)
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            k = np.roll(known, (dy, dx), (0, 1))
            acc += np.roll(rgb, (dy, dx), (0, 1)) * k[..., None]
            cnt += k
        grow = (~known) & (cnt > 0)
        rgb[grow] = acc[grow] / cnt[grow][:, None]
        known = known | grow
    return rgb


def downsample(a):
    h, w = a.shape[:2]
    if h > 1:
        a = 0.5 * (a[0:h - h % 2:2] + a[1:h - h % 2 + 1:2])
    if w > 1:
        a = 0.5 * (a[:, 0:w - w % 2:2] + a[:, 1:w - w % 2 + 1:2])
    return a


def coverage(alpha, cutoff=0.5):
    return float((alpha > cutoff).mean())


def keep_coverage(alpha, target, cutoff=0.5):
    """Scale alpha so the fraction above the cutoff matches the top level."""
    lo, hi = 0.0, 8.0
    for _ in range(24):
        mid = 0.5 * (lo + hi)
        if coverage(np.clip(alpha * mid, 0, 1), cutoff) < target:
            lo = mid
        else:
            hi = mid
    return np.clip(alpha * hi, 0, 1)


def mip_chain(image, alpha_coverage=False, normal=False):
    levels = [image]
    target = coverage(image[..., 3]) if alpha_coverage else None
    while max(levels[-1].shape[:2]) > 1:
        nxt = downsample(levels[-1])
        if normal:
            v = nxt[..., :3] * 2 - 1
            v /= np.linalg.norm(v, axis=2, keepdims=True) + 1e-6
            nxt[..., :3] = v * 0.5 + 0.5
        if alpha_coverage:
            nxt[..., 3] = keep_coverage(nxt[..., 3], target)
        levels.append(nxt)
    return levels


def load(path, size, mode="RGB"):
    """Square at `size`, or, for a tuple (None, longest), the source aspect capped at longest."""
    img = Image.open(path).convert(mode)
    if isinstance(size, tuple):
        k = min(1.0, size[1] / max(img.size))
        target = (max(1, round(img.size[0] * k)), max(1, round(img.size[1] * k)))
    else:
        target = (size, size)
    if img.size != target:
        img = img.resize(target, Image.LANCZOS)
    return np.asarray(img, dtype=np.float32) / 255.0


def calibrate(rgb, mask, target, tint=None):
    """Tint sRGB colours in linear space, then scale them so the masked texels' mean
    luminance is `target`. Scanned albedos are physically dark; the game's exposure is set
    for brighter assets."""
    if (not target or target <= 0) and tint is None:
        return rgb
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    if tint is not None:
        lin = lin * np.asarray(tint, np.float32)
    if not target or target <= 0:
        lin = np.clip(lin, 0.0, 1.0)
        return np.where(lin <= 0.0031308, lin * 12.92, 1.055 * lin ** (1 / 2.4) - 0.055).astype(np.float32)
    lum = float((lin[mask] @ np.array([0.2126, 0.7152, 0.0722], np.float32)).mean())
    lin = np.clip(lin * (target / max(lum, 1e-4)), 0.0, 1.0)
    return np.where(lin <= 0.0031308, lin * 12.92, 1.055 * lin ** (1 / 2.4) - 0.055).astype(np.float32)


def control(ctl, name, default=None):
    parm = ctl.parm(name)
    return parm.eval() if parm is not None else default


def write_ktx2(ktx, levels, destination, srgb, preview=None):
    if preview is not None:
        preview.mkdir(parents=True, exist_ok=True)
        Image.fromarray(np.round(np.clip(levels[0], 0, 1) * 255).astype(np.uint8), "RGBA").save(
            preview / destination.with_suffix(".png").name)
    with tempfile.TemporaryDirectory() as tmp:
        files = []
        for i, level in enumerate(levels):
            path = Path(tmp) / f"level{i}.png"
            Image.fromarray(np.round(np.clip(level, 0, 1) * 255).astype(np.uint8), "RGBA").save(path)
            files.append(str(path))
        pending = destination.with_suffix(".ktx2.pending")
        command = [str(ktx), "create", "--format", "R8G8B8A8_SRGB" if srgb else "R8G8B8A8_UNORM",
                   "--assign-tf", "srgb" if srgb else "linear", "--levels", str(len(levels)),
                   "--encode", "uastc", "--uastc-quality", "2", "--zstd", "8", *files, str(pending)]
        subprocess.run(command, check=True)
        subprocess.run([str(ktx), "validate", str(pending)], check=True, stdout=subprocess.DEVNULL)
        pending.replace(destination)


def export_textures(ctl, ktx, folder, preview=None):
    folder.mkdir(parents=True, exist_ok=True)
    ev = ctl.evalParm
    out = {}

    # Leaf cards use the branch cluster atlas that BAKE_CLUSTERS renders from the sprigs.
    bake = Path(ctl.parm("bake_dir").evalAsString())
    rgba = load(bake / "cluster_color.png", LEAF_SIZE, "RGBA")
    alpha = rgba[..., 3]
    tint = ctl.parmTuple("leaf_tint").eval() if ctl.parmTuple("leaf_tint") is not None else None
    color = calibrate(rgba[..., :3], alpha > 0.5, control(ctl, "leaf_luminance"), tint)
    rgba = np.dstack([bleed(color, alpha), alpha])
    write_ktx2(ktx, mip_chain(rgba, alpha_coverage=True), folder / "leaf_color.ktx2", True, preview)
    out["leaf_color"] = "leaf_color.ktx2"

    normal = load(bake / "cluster_normal.png", LEAF_SIZE)
    normal = np.dstack([bleed(normal, alpha), np.ones_like(alpha)])
    write_ktx2(ktx, mip_chain(normal, normal=True), folder / "leaf_normal.ktx2", False, preview)
    out["leaf_normal"] = "leaf_normal.ktx2"

    # Crown normals do not follow the real leaf surfaces, so glossy highlights would land in
    # the wrong places; keep the atlas variation but within a rough range.
    floor = ev("leaf_roughness_floor")
    rough = floor + (1.0 - floor) * load(bake / "cluster_roughness.png", LEAF_SIZE, "L")
    mr = np.dstack([np.ones_like(rough), bleed(rough[..., None], alpha)[..., 0], np.zeros_like(rough),
                    np.ones_like(rough)])
    write_ktx2(ktx, mip_chain(mr), folder / "leaf_metallic_roughness.ktx2", False, preview)
    out["leaf_metallic_roughness"] = "leaf_metallic_roughness.ktx2"

    layers = [("bark", "bark")]
    if ctl.parm("bark_upper_color") is not None:
        layers.append(("bark_upper", "bark_upper"))
    for key, parm in layers:
        size = (None, BARK_MAX)
        color = load(ctl.parm(f"{parm}_color").evalAsString(), size)
        color = calibrate(color, np.ones(color.shape[:2], bool), control(ctl, f"{parm}_luminance"))
        normal = load(ctl.parm(f"{parm}_normal").evalAsString(), size)
        if ctl.parm(f"{parm}_roughness") is not None:
            rough = load(ctl.parm(f"{parm}_roughness").evalAsString(), size, "L")
            mr = np.dstack([np.ones_like(rough), rough, np.zeros_like(rough)])
        else:  # already glTF-packed: roughness in G, metalness in B
            mr = load(ctl.parm(f"{parm}_metallic_roughness").evalAsString(), size)
        for suffix, img, srgb, is_normal in (("color", color, True, False), ("normal", normal, False, True),
                                             ("metallic_roughness", mr, False, False)):
            img = np.dstack([img, np.ones(img.shape[:2], dtype=np.float32)])
            name = f"{key}_{suffix}.ktx2"
            write_ktx2(ktx, mip_chain(img, normal=is_normal), folder / name, srgb, preview)
            out[f"{key}_{suffix}"] = name
    return out


# ------------------------------------------------------------------ geometry
def read_mesh(geo):
    P = np.array(geo.pointFloatAttribValues("P"), dtype=np.float32).reshape(-1, 3)
    N = np.array(geo.pointFloatAttribValues("N"), dtype=np.float32).reshape(-1, 3)
    Cd = np.array(geo.pointFloatAttribValues("Cd"), dtype=np.float32).reshape(-1, 3)
    alpha = (np.array(geo.pointFloatAttribValues("Alpha"), dtype=np.float32)
             if geo.findPointAttrib("Alpha") is not None else np.ones(len(Cd), np.float32))
    Cd = np.hstack([Cd, alpha[:, None]])  # alpha: bark blend weight (1 on foliage)
    uv2 = np.array(geo.pointFloatAttribValues("uv2"), dtype=np.float32).reshape(-1, 3)[:, :2]
    uv = np.array(geo.vertexFloatAttribValues("uv"), dtype=np.float32).reshape(-1, 3)[:, :2]
    # Branch cards that turn towards the camera in the engine (_CARD_PIVOT, _CARD_AXIS,
    # _CARD_NORMAL; a zero axis keeps a card fixed). Optional; zero for bark.
    cards = {}
    for name, attrib in (("_CARD_PIVOT", "cardpivot"), ("_CARD_AXIS", "axis"), ("_CARD_NORMAL", "cardn")):
        if geo.findPointAttrib(attrib) is not None:
            cards[name] = np.array(geo.pointFloatAttribValues(attrib), dtype=np.float32).reshape(-1, 3)
    mats = geo.primStringAttribValues("mat")
    tris = {"bark": [], "leaf": []}
    vtx = 0
    for prim, mat in zip(geo.prims(), mats):
        pts = [v.point().number() for v in prim.vertices()]
        if len(pts) != 3:
            sys.exit("OUT_TREE must be triangulated")
        # Houdini's front faces wind clockwise; glTF's wind counter-clockwise.
        tris[mat].append([(pts[0], vtx), (pts[2], vtx + 2), (pts[1], vtx + 1)])
        vtx += 3
    return P, N, Cd, uv2, uv, tris, cards


def build_primitive(P, N, Cd, uv2, uv, tris, double=False, cards=None):
    keys = {}
    order = []
    index = []
    for tri in tris:
        for pt, vt in tri:
            key = (pt, round(float(uv[vt, 0]), 5), round(float(uv[vt, 1]), 5))
            if key not in keys:
                keys[key] = len(order)
                order.append((pt, vt))
            index.append(keys[key])
    pts = np.array([o[0] for o in order])
    vts = np.array([o[1] for o in order])
    pos, nrm, col, wind = P[pts], N[pts], Cd[pts], uv2[pts]
    nrm = nrm / (np.linalg.norm(nrm, axis=1, keepdims=True) + 1e-8)
    tex = uv[vts].copy()
    tex[:, 1] = 1.0 - tex[:, 1]  # glTF texture space starts at the top
    idx = np.array(index, dtype=np.uint32).reshape(-1, 3)

    # Per-vertex tangents from UV derivatives, in glTF UV space.
    p0, p1, p2 = pos[idx[:, 0]], pos[idx[:, 1]], pos[idx[:, 2]]
    t0, t1, t2 = tex[idx[:, 0]], tex[idx[:, 1]], tex[idx[:, 2]]
    e1, e2 = p1 - p0, p2 - p0
    d1, d2 = t1 - t0, t2 - t0
    det = d1[:, 0] * d2[:, 1] - d2[:, 0] * d1[:, 1]
    det = np.where(np.abs(det) < 1e-12, 1e-12, det)
    T = (e1 * d2[:, 1:2] - e2 * d1[:, 1:2]) / det[:, None]
    B = (e2 * d1[:, 0:1] - e1 * d2[:, 0:1]) / det[:, None]
    tan = np.zeros_like(pos)
    bit = np.zeros_like(pos)
    for k in range(3):
        np.add.at(tan, idx[:, k], T)
        np.add.at(bit, idx[:, k], B)
    tan -= nrm * np.sum(tan * nrm, axis=1, keepdims=True)
    bad = np.linalg.norm(tan, axis=1) < 1e-8
    fallback = np.cross(nrm, np.array([0.0, 1.0, 0.0], dtype=np.float32))
    fallback[np.linalg.norm(fallback, axis=1) < 1e-6] = (1.0, 0.0, 0.0)
    tan[bad] = fallback[bad]
    tan /= np.linalg.norm(tan, axis=1, keepdims=True)
    w = np.where(np.sum(np.cross(nrm, tan) * bit, axis=1) < 0, -1.0, 1.0).astype(np.float32)
    tangent = np.hstack([tan, w[:, None]]).astype(np.float32)
    if double:
        # For viewers that flip normals on back faces: a reversed copy of every
        # triangle shares the same vertices, so both sides keep the crown normal.
        idx = np.vstack([idx, idx[:, ::-1]])
    attrs = {"POSITION": pos, "NORMAL": nrm, "TANGENT": tangent, "TEXCOORD_0": tex,
             "TEXCOORD_1": wind, "COLOR_0": col}
    for name, values in (cards or {}).items():
        attrs[name] = values[pts]
    return attrs, idx


def write_gltf(path, primitives, textures, double_sided=True, bark_extras=None, shading="crown_v1"):
    blob = bytearray()
    views, accessors = [], []

    def add(data, target, kind, component, minmax=False):
        data = np.ascontiguousarray(data)
        while len(blob) % 4:
            blob.append(0)
        views.append({"buffer": 0, "byteOffset": len(blob), "byteLength": data.nbytes, "target": target})
        blob.extend(data.tobytes())
        acc = {"bufferView": len(views) - 1, "componentType": component, "count": int(data.shape[0]),
               "type": kind}
        if minmax:
            acc["min"] = data.min(0).tolist()
            acc["max"] = data.max(0).tolist()
        accessors.append(acc)
        return len(accessors) - 1

    images = sorted({v for k, v in textures.items() if not k.startswith("bark_upper")})
    image_index = {name: i for i, name in enumerate(images)}

    def tex(key):
        return {"index": image_index[textures[key]]}

    materials = [
        {"name": "bark", "pbrMetallicRoughness": {"baseColorTexture": tex("bark_color"),
                                                  "metallicRoughnessTexture": tex("bark_metallic_roughness"),
                                                  "metallicFactor": 0.0, "roughnessFactor": 1.0},
         "normalTexture": {**tex("bark_normal"), "scale": 1.0},
         **({"extras": bark_extras} if bark_extras else {})},
        {"name": "leaves", "pbrMetallicRoughness": {"baseColorTexture": tex("leaf_color"),
                                                    "metallicRoughnessTexture": tex("leaf_metallic_roughness"),
                                                    "metallicFactor": 0.0, "roughnessFactor": 1.0},
         "normalTexture": {**tex("leaf_normal"), "scale": 0.6},
         "alphaMode": "MASK", "alphaCutoff": 0.5, "doubleSided": double_sided,
         # Half the default reflectance: less grey sky sheen on leaves in shade.
         "extensions": {"KHR_materials_specular": {"specularFactor": 0.5}},
         "extras": {"yarra_wind": "foliage_uv1_v1", "yarra_shading": shading}},
    ]
    prims = []
    for material, (attrs, idx) in enumerate(primitives):
        if len(idx) == 0:
            continue
        a = {
            "POSITION": add(attrs["POSITION"], 34962, "VEC3", 5126, True),
            "NORMAL": add(attrs["NORMAL"], 34962, "VEC3", 5126),
            "TANGENT": add(attrs["TANGENT"], 34962, "VEC4", 5126),
            "TEXCOORD_0": add(attrs["TEXCOORD_0"], 34962, "VEC2", 5126),
            "TEXCOORD_1": add(attrs["TEXCOORD_1"], 34962, "VEC2", 5126),
            "COLOR_0": add(attrs["COLOR_0"], 34962, "VEC4", 5126),
        }
        for name in ("_CARD_PIVOT", "_CARD_AXIS", "_CARD_NORMAL"):
            if name in attrs:
                a[name] = add(attrs[name], 34962, "VEC3", 5126)
        if len(attrs["POSITION"]) < 65536:
            indices = add(idx.reshape(-1).astype(np.uint16), 34963, "SCALAR", 5123)
        else:
            indices = add(idx.reshape(-1).astype(np.uint32), 34963, "SCALAR", 5125)
        prims.append({"attributes": a, "indices": indices, "material": material})
    sampler = {"magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497}
    doc = {
        "asset": {"version": "2.0", "generator": "yarra houdini_export_tree.py"},
        "extensionsUsed": ["KHR_materials_specular"],
        "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"name": path.stem, "mesh": 0}],
        "meshes": [{"name": path.stem, "primitives": prims}],
        "materials": materials,
        "samplers": [sampler],
        "images": [{"uri": f"../textures/{name}"} for name in images],
        "textures": [{"sampler": 0, "source": i} for i in range(len(images))],
        "buffers": [{"uri": path.with_suffix(".bin").name, "byteLength": len(blob)}],
        "bufferViews": views,
        "accessors": accessors,
    }
    path.with_suffix(".bin").write_bytes(bytes(blob))
    path.write_text(json.dumps(doc, indent=1))
    return len(blob)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("scene")
    parser.add_argument("output", type=Path, help="pack directory under assets/, e.g. assets/local/yarra_trees")
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--variant", action="append", required=True,
                        metavar="NAME:SEED:HEIGHT:WIDTH[:CROWN_BASE[:PARM=VALUE...]]",
                        help="extra PARM=VALUE fields override CONTROLS for this variant only")
    parser.add_argument("--ktx")
    parser.add_argument("--object", default="/obj/FOREST_TREE", help="the tree object in the scene")
    parser.add_argument("--skip-textures", action="store_true")
    parser.add_argument("--set", action="append", default=[], metavar="PARM=VALUE",
                        help="override a CONTROLS parameter for every variant, e.g. lod_shell=0")
    parser.add_argument("--lod-heights", default="320,160,80,0", metavar="H0,H1,H2,H3",
                        help="minimum projected height in logical pixels for each LOD; the last must be 0")
    parser.add_argument("--preview", type=Path, metavar="DIR",
                        help="also write PNG-textured LODs with both leaf sides as triangles, for DCC viewers")
    args = parser.parse_args()

    assets = Path(__file__).resolve().parents[1] / "assets"
    output = args.output.resolve()
    try:
        pack = output.relative_to(assets)
    except ValueError:
        sys.exit(f"{output} must be inside {assets}")

    hou.hipFile.load(args.scene, suppress_save_prompt=True, ignore_load_warnings=True)
    obj = hou.node(args.object)
    if obj is None:
        sys.exit(f"{args.scene} has no {args.object}")
    for bake in (n for n in obj.children() if n.name().startswith("BAKE_")):
        bake.cook(force=True)  # the leaf atlas must match the scene
    ctl, out = obj.node("CONTROLS"), obj.node("OUT_TREE")
    def apply(items):
        """Set PARM=VALUE overrides; returns the previous values so they can be restored."""
        previous = []
        for item in items:
            name, value = item.split("=", 1)
            parm = ctl.parm(name)
            if parm is None:
                sys.exit(f"CONTROLS has no parameter {name}")
            previous.append(f"{name}={parm.eval()}")
            parm.set(type(parm.eval())(value))
        return previous

    apply(args.set)
    heights = [float(h) for h in args.lod_heights.split(",")]
    if len(heights) != 4 or heights[-1] != 0 or heights != sorted(heights, reverse=True):
        sys.exit("--lod-heights needs four descending values ending in 0")
    runtime = output / "runtime"
    names = {f"{key}_{suffix}": f"{key}_{suffix}.ktx2"
             for key in ("leaf", "bark") + (("bark_upper",) if ctl.parm("bark_upper_color") else ())
             for suffix in ("color", "normal", "metallic_roughness")}
    if not args.skip_textures:
        names = export_textures(ctl, find_ktx(args.ktx), runtime / "textures",
                                args.preview / "textures" if args.preview else None)
    bark_extras = None
    if "bark_upper_color" in names:
        textures = f"{pack.as_posix()}/runtime/textures"
        bark_extras = {"yarra_bark": "blend_v1", "yarra_bark_upper": {
            suffix: f"{textures}/{names['bark_upper_' + suffix]}"
            for suffix in ("color", "normal", "metallic_roughness")}}
    # CONTROLS shading_mode 1: flat needle pads whose undersides fall dark (pad_v1).
    shading = "pad_v1" if control(ctl, "shading_mode", 0) == 1 else "crown_v1"
    source = output / "source" / Path(args.scene).name
    source.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(args.scene, source)

    entries = []
    for spec in args.variant:
        name, seed, height, width, *rest = spec.split(":")
        overrides = [f"crown_base={rest[0]}"] if rest else []
        restore = apply(overrides + rest[1:])
        ctl.parm("seed").set(int(seed))
        ctl.parm("height").set(float(height))
        ctl.parm("crown_width").set(float(width))
        variants, bounds = [], None
        for lod, screen in enumerate(heights):
            ctl.parm("lod").set(lod)
            geo = out.geometry()
            P, N, Cd, uv2, uv, tris, cards = read_mesh(geo)
            prims = [build_primitive(P, N, Cd, uv2, uv, tris[m], cards=cards if m == "leaf" else None)
                     if tris[m] else ({}, np.zeros((0, 3))) for m in ("bark", "leaf")]
            path = runtime / name / f"{name}_lod{lod}.gltf"
            path.parent.mkdir(parents=True, exist_ok=True)
            size = write_gltf(path, prims, names, bark_extras=bark_extras, shading=shading)
            if args.preview:
                preview = args.preview / name / path.name
                preview.parent.mkdir(parents=True, exist_ok=True)
                doubled = [prims[0], build_primitive(P, N, Cd, uv2, uv, tris["leaf"], True)]  # rest pose
                write_gltf(preview, doubled, {k: v.replace(".ktx2", ".png") for k, v in names.items()}, False)
            span = (float(max(abs(P[:, 0]).max(), 1e-3) * 2), float(P[:, 1].max()),
                    float(max(abs(P[:, 2]).max(), 1e-3) * 2))
            bounds = span if bounds is None else tuple(max(a, b) for a, b in zip(bounds, span))
            tri_count = sum(len(t) for t in tris.values())
            leaf = np.array([[pt for pt, _ in t] for t in tris["leaf"]], dtype=np.int64).reshape(-1, 3)
            corners = P[leaf]
            area = 0.5 * np.linalg.norm(np.cross(corners[:, 1] - corners[:, 0], corners[:, 2] - corners[:, 0]),
                                        axis=1).sum()
            print(f"{name} LOD{lod}: {tri_count} triangles ({len(tris['bark'])} bark, "
                  f"{len(tris['leaf'])} leaf), {area:.0f} m2 of leaf cards, {size} bytes, height {span[1]:.1f} m")
            variants.append((f"{pack.as_posix()}/runtime/{name}/{path.name}", size, screen))
        apply(restore)
        title = name.replace("_", " ").title()
        lines = [f'(key: "{pack.name}/{name}", display_name: "{title}", '
                 f'source_uri: "{pack.as_posix()}/source/{source.name}", variants: [']
        for uri, size, screen in variants:
            lines.append(f'(uri: "{uri}", bounds: ({bounds[0]:.4f}, {bounds[1]:.4f}, {bounds[2]:.4f}), '
                         f'gpu_bytes_estimate: {size}, minimum_screen_height: {screen:.1f}),')
        lines.append("]),")
        entries.append("\n".join(lines))
    args.catalog.parent.mkdir(parents=True, exist_ok=True)
    args.catalog.write_text("(schema_version: 1, assets: [\n" + "\n".join(entries) + "\n])\n")
    print("catalog", args.catalog)


if __name__ == "__main__":
    main()
