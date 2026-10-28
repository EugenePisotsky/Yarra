"""Wraps a fitted MPFB body onto the body it was fitted to: the shape sliders cannot give.

blender -b --python-exit-code 1 --python wrap_body.py -- MODEL.glb PRESET.json OUT.npz

PRESET is a preset fit_body.py fitted to MODEL. Its body, sliders held, is placed and its arms
turned onto MODEL as in the fit, and every body vertex is moved onto MODEL's surface: a smooth
field of offsets drawn to the nearest points (stiff first, then looser, so a vertex follows its
neighbours where matches are wrong), then each vertex onto its own match. The head, hands and feet
stay MPFB's (its face sliders, eyes and fingers fit them), blended in at the neck, wrists and
ankles by skin weight.

MPFB's rig puts each joint at the middle of a cube of helper vertices. The cubes of the joints
MODEL's clips turn its limbs about (shoulders, elbows, hips and knees) move to MODEL's joints, so
the limbs turn where the clips expect; every other helper (joint cubes, the tights and skirt
shells clothes are fitted to) follows the body near it.

OUT holds one offset per base-mesh vertex in MPFB's rest pose, a target over the preset's sliders.
It is MODEL's shape, so it stays with MODEL in assets/local; a preset names it as "wrap", and
build_human applies it and refits the rig and parts.
"""
import json
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils.bvhtree import BVHTree
from mathutils.kdtree import KDTree

sys.path.insert(0, str(Path(__file__).parent))

from fit_body import BODY_VERTICES, MACROS, SLIDERS, Fit, Human, TargetService, load_model, sides

# Joints placed at MODEL's: MPFB's joint cube, MODEL's bone whose head is the joint (the point its
# clips turn the limb about), and the MPFB arm segment that turns with the limb below it in the
# fit's pose (legs are not turned).
JOINTS = [
    ("shoulder", "c_arm_twist_offset", "upper"),
    ("elbow", "forearm_stretch", "fore"),
    ("upper-leg", "thigh_twist", None),
    ("knee", "leg_stretch", None),
]

SIDES = (("l", "left"), ("r", "right"))

# MPFB's default-rig bones whose vertices (with their children's) stay MPFB's.
HELD = ("head", "wrist.L", "wrist.R", "foot.L", "foot.R")

# Smoothing passes widening the blend between held and wrapped vertices.
BLEND_PASSES = 8

# Matches farther than this, metres, or whose surfaces face apart, are left out.
MATCH_LIMIT = 0.04
FACING = 0.5

# Rounds fitting the preset's body's placement and arms to MODEL before it is wrapped.
PLACEMENT_STEPS = 8

# Each offset's pull towards its neighbours' against its pull to its match, loosening.
STIFFNESS = (10.0, 3.0, 1.0, 0.3)
SMOOTHING_PASSES = 300

# A held vertex's pull to its own place.
ANCHOR = 100.0

# Body vertices whose offsets a helper vertex takes, nearest first.
HELPER_NEIGHBOURS = 8


def preset_state(preset):
    """The fit's starting state from a preset fit_body.py wrote."""
    macros = {name: preset["phenotype"][name] for name in MACROS}
    targets = {t["target"]: t["value"] for t in preset["targets"]}
    values = np.zeros(len(SLIDERS))

    for k, (negative, positive) in enumerate(SLIDERS):
        for sign, name in ((-1.0, negative), (1.0, positive)):
            values[k] += sign * targets.get(sides(name)[0], 0.0)

    return macros, values, 0.0, 0.0


def body_topology(basemesh):
    """The body's edges and triangles (fans of its faces), helpers left out."""
    mesh = basemesh.data
    edges = np.array([e.vertices[:] for e in mesh.edges if max(e.vertices) < BODY_VERTICES])
    triangles = [
        (face[0], face[k], face[k + 1])
        for face in (p.vertices[:] for p in mesh.polygons if max(p.vertices) < BODY_VERTICES)
        for k in range(1, len(face) - 1)
    ]
    return edges, np.array(triangles)


def neighbour_sum(values, edges):
    i, j = edges.T
    count = len(values)
    return np.stack(
        [
            np.bincount(i, weights=values[j, c], minlength=count) + np.bincount(j, weights=values[i, c], minlength=count)
            for c in range(values.shape[1])
        ],
        axis=1,
    )


def degrees(edges, count):
    return np.bincount(edges[:, 0], minlength=count) + np.bincount(edges[:, 1], minlength=count)


def face_normals(positions, triangles):
    a, b, c = (positions[triangles[:, k]] for k in range(3))
    return np.cross(b - a, c - a)


def vertex_normals(positions, triangles):
    faces = face_normals(positions, triangles)
    normals = np.zeros_like(positions)

    for k in range(3):
        np.add.at(normals, triangles[:, k], faces)

    return normals / np.maximum(np.linalg.norm(normals, axis=1, keepdims=True), 1e-12)


def held_weights(basemesh, edges):
    """How much each body vertex stays MPFB's: its skin weight in the held bones, the blend
    widened by smoothing over the mesh."""
    rig = basemesh.parent
    names = set()

    for name in HELD:
        bone = rig.data.bones[name]
        names |= {bone.name, *(child.name for child in bone.children_recursive)}

    groups = {g.index for g in basemesh.vertex_groups if g.name in names}
    held = np.zeros(BODY_VERTICES)

    for vertex in basemesh.data.vertices[:BODY_VERTICES]:
        held[vertex.index] = sum(e.weight for e in vertex.groups if e.group in groups)

    held = np.clip(held, 0.0, 1.0)[:, None]
    degree = degrees(edges, BODY_VERTICES)[:, None]

    for _ in range(BLEND_PASSES):
        held = (held + neighbour_sum(held, edges)) / (1 + degree)

    return held[:, 0]


class Surface:
    """MODEL's surface split where its arms meet its body; MPFB's arm vertices match its arms."""

    def __init__(self, model, mpfb_segments):
        positions, triangles = model["positions"], model["triangles"]
        arm = sum(model["segments"][s][n] for s in ("left", "right") for n in ("upper", "fore", "hand"))
        self.trees = [
            BVHTree.FromPolygons(positions.tolist(), triangles[m].tolist())
            for m in (arm[triangles].min(axis=1) < 0.5, arm[triangles].max(axis=1) > 0.5)
        ]
        mpfb_arm = sum(mpfb_segments[s][n] for s in ("left", "right") for n in ("upper", "fore", "hand"))
        self.on_arm = mpfb_arm >= 0.5

    def matches(self, positions, normals):
        """Each vertex's nearest point on MODEL's surface, NaN where none is near or facing alike."""
        out = np.full(positions.shape, np.nan)

        for i, (position, normal) in enumerate(zip(positions, normals)):
            point, facing, _, _ = self.trees[int(self.on_arm[i])].find_nearest(tuple(position), MATCH_LIMIT)

            if point is not None and np.dot(facing, normal) > FACING:
                out[i] = point

        return out


def wrap(posed, surface, held, edges, triangles):
    """Offsets moving the posed body onto MODEL's surface, held vertices kept."""
    offsets = np.zeros_like(posed)
    degree = degrees(edges, len(posed))
    free = 1.0 - held

    for stiffness in STIFFNESS:
        moved = posed + offsets
        goals = surface.matches(moved, vertex_normals(moved, triangles)) - posed
        pull = np.where(np.isnan(goals[:, 0]), 0.0, free)
        goals = np.nan_to_num(goals)

        for _ in range(SMOOTHING_PASSES):
            offsets = (pull[:, None] * goals + stiffness * neighbour_sum(offsets, edges)) / (
                pull + ANCHOR * held + stiffness * degree
            )[:, None]

    moved = posed + offsets
    goals = surface.matches(moved, vertex_normals(moved, triangles)) - posed
    found = ~np.isnan(goals[:, 0])
    offsets[found] += free[found, None] * (goals[found] - offsets[found])
    return offsets, found


def report(posed, offsets, found, held, triangles):
    moved = posed + offsets
    wrapped = found & (held < 0.01)
    before = face_normals(posed, triangles)
    after = face_normals(moved, triangles)
    flipped = int(np.sum(np.einsum("ij,ij->i", before, after) < 0))
    stretch = np.linalg.norm(after, axis=1) / np.maximum(np.linalg.norm(before, axis=1), 1e-12)
    moves = np.linalg.norm(offsets[wrapped], axis=1) * 1000
    print(
        f"WRAP {wrapped.sum()} vertices wrapped, {int((held >= 0.99).sum())} held, "
        f"{int((~found & (held < 0.01)).sum())} unmatched | moved mean {moves.mean():.1f} p95 "
        f"{np.percentile(moves, 95):.1f} max {moves.max():.1f} mm | {flipped} faces flipped, "
        f"area change p1 {np.percentile(stretch, 1):.2f} p99 {np.percentile(stretch, 99):.2f}"
    )


def shaped(human, macros, values):
    """Every base-mesh vertex, helpers too, with the fitted sliders: world positions."""
    basemesh = human.basemesh
    human.with_macros(macros)
    keys = []

    for (negative, positive), value in zip(SLIDERS, values):
        if abs(value) >= 1e-6:
            for target in sides(negative if value < 0 else positive):
                path = TargetService.target_full_path(target)
                keys.append(TargetService.load_target(basemesh, path, weight=abs(float(value)), name=f"wrap-{target}"))

    mix = basemesh.shape_key_add(name="wrap-mix", from_mix=True)
    positions = np.empty(len(mix.data) * 3)
    mix.data.foreach_get("co", positions)

    for key in [mix, *keys]:
        basemesh.shape_key_remove(key)

    return positions.reshape(-1, 3) @ human.linear.T + human.offset


def cube(basemesh, name):
    group = basemesh.vertex_groups[name].index
    return np.array([v.index for v in basemesh.data.vertices if any(e.group == group for e in v.groups)])


def unposed(point, fitted, near):
    """A posed point brought back to the rest pose by the turn of the body vertices `near` it
    (one rigid part, so they agree)."""
    linear = np.einsum("nij,jk->nik", fitted["turns"][near], fitted["tilt"])
    back = np.einsum("nji,nj->ni", linear, point - fitted["posed"][near])
    return (fitted["body"][near] + back).mean(axis=0)


def joint_offsets(human, fitted, rest, model_joints):
    """Offsets moving the joint cubes to MODEL's joints, by cube vertex."""
    out = {}
    posed = fitted["posed"]

    for joint, bone, segment in JOINTS:
        for s, side in SIDES:
            goal = model_joints[f"{bone}.{s}"]

            if segment:
                part = np.where(human.segments[side][segment] > 0.9)[0]
            else:
                part = np.arange(BODY_VERTICES)

            near = part[np.argsort(np.linalg.norm(posed[part] - goal, axis=1))[:32]]
            target = unposed(goal, fitted, near)
            vertices = cube(human.basemesh, f"joint-{s}-{joint}")
            centre = rest[vertices].mean(axis=0)
            print(f"JOINT {joint}.{s} moved {1000 * np.linalg.norm(target - centre):.0f} mm: {np.round(centre, 3)} -> {np.round(target, 3)}")

            for v in vertices:
                out[v] = target - centre

    return out


def helper_offsets(rest, body_offsets):
    """Each helper vertex's offset: the mean of its nearest body vertices', by inverse distance."""
    tree = KDTree(BODY_VERTICES)

    for i, position in enumerate(rest[:BODY_VERTICES]):
        tree.insert(position, i)

    tree.balance()
    out = np.zeros((len(rest) - BODY_VERTICES, 3))

    for k, position in enumerate(rest[BODY_VERTICES:]):
        found = tree.find_n(position, HELPER_NEIGHBOURS)
        weights = np.array([1.0 / max(distance, 1e-4) for _, _, distance in found])
        out[k] = sum(w * body_offsets[i] for w, (_, i, _) in zip(weights, found)) / weights.sum()

    return out


def main():
    model_path, preset_path, out_path = (Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :])
    preset = json.loads(preset_path.read_text())
    model = load_model(model_path)
    human = Human(preset)
    fit = Fit(human, model)
    (macros, values, _, _), fitted = fit.run_placement(preset_state(preset), PLACEMENT_STEPS)
    edges, triangles = body_topology(human.basemesh)
    held = held_weights(human.basemesh, edges)
    posed = fitted["posed"]
    offsets, found = wrap(posed, Surface(model, human.segments), held, edges, triangles)
    report(posed, offsets, found, held, triangles)

    # Back to the rest pose: undo each vertex's turn in the fit's pose.
    linear = np.einsum("nij,jk->nik", fitted["turns"], fitted["tilt"])
    body_offsets = np.einsum("nji,nj->ni", linear, offsets)
    rest = shaped(human, macros, values)
    print(f"WRAP rest bodies differ by at most {1000 * np.abs(rest[:BODY_VERTICES] - fitted['body']).max():.2f} mm")
    out = np.zeros_like(rest)
    out[:BODY_VERTICES] = body_offsets
    out[BODY_VERTICES:] = helper_offsets(rest, body_offsets)

    for vertex, offset in joint_offsets(human, fitted, rest, model["joints"]).items():
        out[vertex] = offset

    out_path.parent.mkdir(parents=True, exist_ok=True)
    np.savez(out_path, offsets=out)
    print(f"WRAP wrote {out_path}: {len(out)} offsets")


main()
