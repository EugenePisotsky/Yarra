"""Fits MPFB's body sliders to another body's shape and writes them as a preset.

blender -b --python-exit-code 1 --python fit_body.py -- MODEL.glb BASE_PRESET.json OUT_PRESET.json \
  [POINTS.npz]

MODEL is a skinned glTF body in its rest pose, facing glTF's front with its feet at zero. The
preset keeps BASE_PRESET's face, skin and parts and takes the fitted macro sliders (height,
proportions, muscle, weight, cup size, firmness) and region sliders; nothing of the model's mesh
is kept, only slider values.

Each MPFB body vertex is matched to the nearest point of the model's surface (torso, legs and
head to the model's, arms to its arms) and the sliders minimise the distance along the model's
normal there, matches renewed every step (point-to-plane ICP), a step kept only if it brings the
bodies closer (Levenberg-Marquardt). The MPFB body may also move along the depth axis and lean
about its feet, and its posture sliders move torso, hips, neck and head forward or back: a model
standing differently would otherwise be matched by changing shape. Region sliders move vertices linearly; macro sliders by
MPFB's own piecewise blend of targets, linearised each step. The two bodies hold their arms
differently, so MPFB's arms are first turned to the model's: the upper arm about the shoulder,
then the forearm about the elbow, each vertex by its share of skin weight. Hands are left out
(their fingers are posed differently). POINTS, if given, receives both bodies' points as fitted,
for overlay.py.
"""
import json
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils.bvhtree import BVHTree

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import build_human, merged, mpfb_attribute, service, sides

HumanService = service("humanservice", "HumanService")
TargetService = service("targetservice", "TargetService")
HumanObjectProperties = mpfb_attribute("mpfb.entities.objectproperties", "HumanObjectProperties")

# MakeHuman's base mesh: vertices from here on are helper geometry (eyes, teeth, joint cubes).
BODY_VERTICES = 13380

MACROS = {"height": 0.57, "proportions": 1.0, "muscle": 0.6, "weight": 0.45, "cupsize": 0.5, "firmness": 0.5}
MACRO_STEP = 0.04
MACRO_MOVE = 0.1

# Region sliders, each (negative target, positive target). "l-" names move both sides.
SLIDERS = [
    ("measure-bust-circ-decr", "measure-bust-circ-incr"),
    ("measure-underbust-circ-decr", "measure-underbust-circ-incr"),
    ("measure-waist-circ-decr", "measure-waist-circ-incr"),
    ("measure-hips-circ-decr", "measure-hips-circ-incr"),
    ("measure-frontchest-dist-decr", "measure-frontchest-dist-incr"),
    ("measure-shoulder-dist-decr", "measure-shoulder-dist-incr"),
    ("measure-napetowaist-dist-decr", "measure-napetowaist-dist-incr"),
    ("measure-waisttohip-dist-decr", "measure-waisttohip-dist-incr"),
    ("torso-muscle-dorsi-decr", "torso-muscle-dorsi-incr"),
    ("torso-muscle-pectoral-decr", "torso-muscle-pectoral-incr"),
    ("torso-scale-depth-decr", "torso-scale-depth-incr"),
    ("torso-scale-horiz-decr", "torso-scale-horiz-incr"),
    ("torso-scale-vert-decr", "torso-scale-vert-incr"),
    ("torso-vshape-decr", "torso-vshape-incr"),
    ("hip-scale-depth-decr", "hip-scale-depth-incr"),
    ("hip-scale-horiz-decr", "hip-scale-horiz-incr"),
    ("hip-scale-vert-decr", "hip-scale-vert-incr"),
    ("hip-waist-down", "hip-waist-up"),
    ("stomach-pregnant-decr", "stomach-pregnant-incr"),
    ("buttocks-volume-decr", "buttocks-volume-incr"),
    ("breast-dist-decr", "breast-dist-incr"),
    ("breast-point-decr", "breast-point-incr"),
    ("breast-trans-down", "breast-trans-up"),
    ("breast-volume-vert-down", "breast-volume-vert-up"),
    ("l-upperleg-fat-decr", "l-upperleg-fat-incr"),
    ("l-upperleg-muscle-decr", "l-upperleg-muscle-incr"),
    ("l-upperleg-scale-depth-decr", "l-upperleg-scale-depth-incr"),
    ("l-upperleg-scale-horiz-decr", "l-upperleg-scale-horiz-incr"),
    ("l-lowerleg-fat-decr", "l-lowerleg-fat-incr"),
    ("l-lowerleg-muscle-decr", "l-lowerleg-muscle-incr"),
    ("l-lowerleg-scale-depth-decr", "l-lowerleg-scale-depth-incr"),
    ("l-lowerleg-scale-horiz-decr", "l-lowerleg-scale-horiz-incr"),
    ("l-leg-valgus-decr", "l-leg-valgus-incr"),
    ("measure-thigh-circ-decr", "measure-thigh-circ-incr"),
    ("measure-knee-circ-decr", "measure-knee-circ-incr"),
    ("measure-calf-circ-decr", "measure-calf-circ-incr"),
    ("measure-upperleg-height-decr", "measure-upperleg-height-incr"),
    ("measure-lowerleg-height-decr", "measure-lowerleg-height-incr"),
    ("measure-neck-circ-decr", "measure-neck-circ-incr"),
    ("measure-neck-height-decr", "measure-neck-height-incr"),
    ("neck-scale-depth-decr", "neck-scale-depth-incr"),
    ("neck-scale-horiz-decr", "neck-scale-horiz-incr"),
    ("head-scale-depth-decr", "head-scale-depth-incr"),
    ("head-scale-horiz-decr", "head-scale-horiz-incr"),
    ("head-scale-vert-decr", "head-scale-vert-incr"),
    ("torso-trans-backward", "torso-trans-forward"),
    ("hip-trans-backward", "hip-trans-forward"),
    ("neck-trans-backward", "neck-trans-forward"),
    ("head-trans-backward", "head-trans-forward"),
    ("measure-upperarm-length-decr", "measure-upperarm-length-incr"),
    ("measure-lowerarm-length-decr", "measure-lowerarm-length-incr"),
    ("measure-upperarm-circ-decr", "measure-upperarm-circ-incr"),
    ("l-upperarm-fat-decr", "l-upperarm-fat-incr"),
    ("l-upperarm-muscle-decr", "l-upperarm-muscle-incr"),
    ("l-upperarm-scale-depth-decr", "l-upperarm-scale-depth-incr"),
    ("l-upperarm-scale-horiz-decr", "l-upperarm-scale-horiz-incr"),
    ("l-upperarm-shoulder-muscle-decr", "l-upperarm-shoulder-muscle-incr"),
    ("l-lowerarm-fat-decr", "l-lowerarm-fat-incr"),
    ("l-lowerarm-muscle-decr", "l-lowerarm-muscle-incr"),
    ("l-lowerarm-scale-depth-decr", "l-lowerarm-scale-depth-incr"),
    ("l-lowerarm-scale-horiz-decr", "l-lowerarm-scale-horiz-incr"),
]

# Slider values cost this much each squared, against mean squared distances in square metres:
# enough to settle sliders that do the same thing, too little to hold any back.
SLIDER_PULL = 2e-6

# Matches farther apart than this are left out, metres.
MATCH_LIMIT = 0.06

STEPS = 25

# The largest change of lean in one step, radians.
LEAN_MOVE = 0.05

# The largest turn of an arm segment in one refinement, radians.
ARM_TURN = 0.05

# Refinements of the arms' turns before the sliders move.
FIRST_ARM_REFINEMENTS = 4

# Skin-weight groups of each arm segment, left side; a name ending in a side is one group, any
# other a family of groups (fingers).
MODEL_ARM = {
    "clavicle": ["shoulder.l"],
    "upper": ["arm_stretch.l", "arm_twist_2.l", "c_arm_twist_offset.l"],
    "fore": ["forearm_stretch.l", "forearm_twist.l", "forearm_twist_2.l"],
    "hand": ["hand.l", "index", "middle", "ring", "pinky", "thumb"],
}
MPFB_ARM = {
    "clavicle": ["clavicle.L", "shoulder01.L"],
    "upper": ["upperarm01.L", "upperarm02.L"],
    "fore": ["lowerarm01.L", "lowerarm02.L"],
    "hand": ["wrist.L", "finger", "metacarpal"],
}

LEFT = (".l", ".L")

RIGHT = (".r", ".R")

MIRROR = np.diag([-1.0, 1.0, 1.0])


def matches(name, pattern, suffixes):
    if not name.endswith(suffixes):
        return False

    if pattern.endswith(LEFT):
        return name[:-2] == pattern[:-2]

    return pattern in name


def weights_in(obj, patterns, suffixes):
    """Each vertex's summed weight in the matching groups."""
    groups = {g.index for g in obj.vertex_groups if any(matches(g.name, p, suffixes) for p in patterns)}
    weights = np.zeros(len(obj.data.vertices))

    for vertex in obj.data.vertices:
        for element in vertex.groups:
            if element.group in groups:
                weights[vertex.index] += element.weight

    return weights


def arm_segments(obj, segments, count):
    """Per side, each arm segment's weight per vertex."""
    return {
        side: {name: weights_in(obj, patterns, suffixes)[:count] for name, patterns in segments.items()}
        for side, suffixes in (("left", LEFT), ("right", RIGHT))
    }


def rotation(a, b):
    """The rotation turning unit vector a onto unit vector b, as an axis and an angle."""
    axis = np.cross(a, b)
    length = np.linalg.norm(axis)

    if length < 1e-9:
        return np.array([0.0, 0.0, 1.0]), 0.0

    return axis / length, float(np.arctan2(length, a @ b))


def rotations(axis, angles):
    """One rotation matrix per angle about one axis (Rodrigues)."""
    k = np.array([[0, -axis[2], axis[1]], [axis[2], 0, -axis[0]], [-axis[1], axis[0], 0]])
    sin, cos = np.sin(angles)[:, None, None], np.cos(angles)[:, None, None]
    return np.eye(3) + sin * k + (1 - cos) * (k @ k)


def turn_about(posed, turns, pivot, turn, weights):
    """Turns vertices about a pivot by their share of a rotation, in place."""
    axis, angle = axis_angle(turn)
    moved = np.where(weights > 0.01)[0]
    turn = rotations(axis, angle * np.clip(weights[moved], 0, 1))
    posed[moved] = pivot + np.einsum("nij,nj->ni", turn, posed[moved] - pivot)
    turns[moved] = turn @ turns[moved]


def axis_angle(turn):
    angle = float(np.arccos(np.clip((np.trace(turn) - 1) / 2, -1.0, 1.0)))

    if angle < 1e-9:
        return np.array([0.0, 0.0, 1.0]), 0.0

    axis = np.array([turn[2, 1] - turn[1, 2], turn[0, 2] - turn[2, 0], turn[1, 0] - turn[0, 1]])
    return axis / np.linalg.norm(axis), angle


def best_turn(points, goals):
    """The rotation about the origin taking points nearest their goals, limited to ARM_TURN."""
    u, _, vt = np.linalg.svd(points.T @ goals)
    flip = np.sign(np.linalg.det(vt.T @ u.T))
    turn = vt.T @ np.diag([1.0, 1.0, flip]) @ u.T
    axis, angle = axis_angle(turn)
    return rotations(axis, np.array([min(angle, ARM_TURN)]))[0]


def principal_axis(points, outward):
    centred = points - points.mean(axis=0)
    direction = np.linalg.svd(centred, full_matrices=False)[2][0]
    return direction if direction @ outward > 0 else -direction


def meeting(positions, first, second):
    """Where two segments' skin weights meet: the weighted middle of the band they share."""
    share = np.minimum(first, second)
    share[share < 0.15] = 0
    return (positions * share[:, None]).sum(axis=0) / share.sum()


def arm_axes(positions, segments, outward):
    """Directions of the upper arm and forearm from their mostly-weighted vertices."""
    upper = principal_axis(positions[segments["upper"] > 0.8], outward)
    fore = principal_axis(positions[segments["fore"] > 0.8], outward)
    return upper, fore


def load_model(path):
    """The model's world positions, triangles, arm weights and joints (bone heads), its objects
    then removed."""
    bpy.ops.import_scene.gltf(filepath=str(path))

    for armature in bpy.data.armatures:
        armature.pose_position = "REST"

    rig = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    body = next(o for o in bpy.data.objects if o.type == "MESH" and any(m.type == "ARMATURE" for m in o.modifiers))
    shaped = body.evaluated_get(bpy.context.evaluated_depsgraph_get()).data
    shaped.calc_loop_triangles()
    matrix = np.array(body.matrix_world)
    local = np.array([v.co for v in shaped.vertices])
    model = {
        "positions": local @ matrix[:3, :3].T + matrix[:3, 3],
        "triangles": np.array([t.vertices[:] for t in shaped.loop_triangles]),
        "segments": arm_segments(body, MODEL_ARM, len(shaped.vertices)),
        "joints": {bone.name: np.array(rig.matrix_world @ bone.head_local) for bone in rig.data.bones},
    }
    bpy.data.batch_remove(list(bpy.data.objects))
    return model


class Human:
    """The MPFB body: its basis, macro sliders and the region sliders' offsets, in world space."""

    def __init__(self, preset):
        parts = {"clothes": [], "hair": "", "eyebrows": "", "eyelashes": "", "teeth": "", "tongue": "", "proxy": ""}
        info = merged(preset, parts)
        info["targets"] = []
        # A wrap (wrap_body.py) lies over the fitted sliders: the fit sees MPFB's own shape.
        info.pop("wrap", None)
        self.basemesh, _, _ = build_human(info, subdivision=0)
        HumanService.add_builtin_rig(self.basemesh, "default")
        matrix = np.array(self.basemesh.matrix_world)
        self.linear, self.offset = matrix[:3, :3], matrix[:3, 3]
        self.basis = self.local(self.basemesh.data.shape_keys.key_blocks[0])
        self.segments = arm_segments(self.basemesh, MPFB_ARM, BODY_VERTICES)
        self.deltas = {}

    def local(self, key):
        positions = np.empty(len(key.data) * 3, dtype=np.float64)
        key.data.foreach_get("co", positions)
        return positions.reshape(-1, 3)[:BODY_VERTICES]

    def delta(self, target):
        """A target's offsets in world space (linear part only)."""
        if target not in self.deltas:
            path = TargetService.target_full_path(target)
            key = TargetService.load_target(self.basemesh, path, weight=0.0, name=f"fit-{target}")
            self.deltas[target] = (self.local(key) - self.basis) @ self.linear.T
            self.basemesh.shape_key_remove(key)

        return self.deltas[target]

    def slider(self, slider, value):
        """A slider's offsets at `value` (negative takes the first target)."""
        negative, positive = slider
        name = negative if value < 0 else positive
        return sum(self.delta(target) for target in sides(name)) * abs(value)

    def column(self, slider, value):
        """The offsets per unit of a slider at `value`; both directions averaged at zero."""
        negative, positive = slider
        up = sum(self.delta(target) for target in sides(positive))
        down = -sum(self.delta(target) for target in sides(negative))
        return up if value > 0 else down if value < 0 else (up + down) / 2

    def with_macros(self, macros):
        """Body positions with these macro sliders and no region sliders."""
        for name, value in macros.items():
            HumanObjectProperties.set_value(name, value, entity_reference=self.basemesh)

        TargetService.reapply_macro_details(self.basemesh)
        local = self.basis.copy()

        for key in self.basemesh.data.shape_keys.key_blocks[1:]:
            if key.name.startswith("$md-") and key.value != 0:
                local += key.value * (self.local(key) - self.basis)

        return local @ self.linear.T + self.offset


class Fit:
    def __init__(self, human, model):
        self.human = human
        positions, triangles = model["positions"], model["triangles"]
        model_arm = sum(model["segments"][s][n] for s in ("left", "right") for n in ("upper", "fore", "hand"))
        model_hand = model["segments"]["left"]["hand"] + model["segments"]["right"]["hand"]
        torso = model_arm[triangles].max(axis=1) < 0.5
        arms = (model_arm[triangles].min(axis=1) > 0.5) & (model_hand[triangles].max(axis=1) < 0.3)
        self.trees = [BVHTree.FromPolygons(positions.tolist(), triangles[m].tolist()) for m in (torso, arms)]
        left = model["segments"]["left"]
        self.model_axes = arm_axes(positions, left, np.array([1.0, 0.0, 0.0]))
        segments = human.segments
        arm = sum(segments[s][n] for s in ("left", "right") for n in ("upper", "fore", "hand"))
        hand = segments["left"]["hand"] + segments["right"]["hand"]
        self.groups = [np.where(arm < 0.3)[0], np.where((arm > 0.5) & (hand < 0.3))[0]]
        self.arm_turns = {}

    def pose(self, positions):
        """Positions with MPFB's arms turned to the model's, and each vertex's rotation: per side,
        the whole arm turns about the shoulder, then forearm and hand about the elbow, each vertex
        by its share of the segment's skin weight. The turns start where the segments' axes meet
        the model's and are refined by the fit (refine_arms)."""
        turns = np.repeat(np.eye(3)[None], len(positions), axis=0)
        posed = positions.copy()

        for side, mirror in (("left", np.eye(3)), ("right", MIRROR)):
            segments = self.human.segments[side]

            if side not in self.arm_turns:
                self.arm_turns[side] = self.first_turns(posed, segments, mirror)

            upper_turn, fore_turn = self.arm_turns[side]
            arm = segments["upper"] + segments["fore"] + segments["hand"]
            shoulder = meeting(posed, segments["clavicle"], segments["upper"])
            turn_about(posed, turns, shoulder, upper_turn, arm)
            elbow = meeting(posed, segments["upper"], segments["fore"])
            turn_about(posed, turns, elbow, fore_turn, segments["fore"] + segments["hand"])

        return posed, turns

    def first_turns(self, positions, segments, mirror):
        """Turns bringing the upper arm's and the forearm's axes onto the model's."""
        outward = mirror @ np.array([1.0, 0.0, 0.0])
        goals = [mirror @ axis for axis in self.model_axes]
        posed = positions.copy()
        scratch = np.repeat(np.eye(3)[None], len(positions), axis=0)
        found = []

        for (segment, weights, pivot_of), goal in zip(
            [
                ("upper", segments["upper"] + segments["fore"] + segments["hand"], ("clavicle", "upper")),
                ("fore", segments["fore"] + segments["hand"], ("upper", "fore")),
            ],
            goals,
        ):
            now = principal_axis(posed[segments[segment] > 0.8], outward)
            axis, angle = rotation(now, goal)
            turn = rotations(axis, np.array([angle]))[0]
            pivot = meeting(posed, segments[pivot_of[0]], segments[pivot_of[1]])
            turn_about(posed, scratch, pivot, turn, weights)
            found.append(turn)

        return found

    def refine_arms(self, fitted):
        """Turns each arm segment by the rotation about its joint that best brings its points
        onto their matches (Kabsch), at most ARM_TURN a step."""
        posed = fitted["posed"]
        index, points, _ = fitted["matched"]
        matched = np.full(posed.shape, np.nan)
        matched[index] = points

        for side in ("left", "right"):
            segments = self.human.segments[side]
            upper_turn, fore_turn = self.arm_turns[side]
            shoulder = meeting(posed, segments["clavicle"], segments["upper"])
            elbow = meeting(posed, segments["upper"], segments["fore"])
            upper = np.where((segments["upper"] > 0.8) & ~np.isnan(matched[:, 0]))[0]
            change = best_turn(posed[upper] - shoulder, matched[upper] - shoulder)
            upper_turn = change @ upper_turn
            fore_turn = change @ fore_turn @ change.T
            fore = np.where((segments["fore"] > 0.8) & ~np.isnan(matched[:, 0]))[0]
            moved = (posed[fore] - shoulder) @ change.T + shoulder
            elbow = (elbow - shoulder) @ change.T + shoulder
            fore_turn = best_turn(moved - elbow, matched[fore] - elbow) @ fore_turn
            self.arm_turns[side] = [upper_turn, fore_turn]

    def evaluate(self, state):
        """The body as fitted, each vertex's turn, its placement and the matches: (indices,
        points, normals)."""
        macros, values, shift, lean = state
        base = self.human.with_macros(macros)
        body = base + sum(self.human.slider(s, v) for s, v in zip(SLIDERS, values))
        placed, tilt, pivot = place(body, shift, lean)
        posed, turns = self.pose(placed)
        index, points, normals = [], [], []

        for tree, group in zip(self.trees, self.groups):
            for i in group:
                point, normal, _, distance = tree.find_nearest(tuple(posed[i]))

                if point is not None and distance < MATCH_LIMIT:
                    index.append(i)
                    points.append(point)
                    normals.append(normal)

        matched = (np.array(index), np.array(points), np.array(normals))
        return {
            "base": base,
            "body": body,
            "placed": placed,
            "tilt": tilt,
            "pivot": pivot,
            "posed": posed,
            "turns": turns,
            "matched": matched,
        }

    def cost(self, posed, matched, values):
        index, points, normals = matched
        gap = np.einsum("ij,ij->i", posed[index] - points, normals)
        return float(np.mean(gap**2) + SLIDER_PULL * np.sum(values**2)), gap

    def run(self, state):
        """Fits from a state: (macros, region slider values, depth shift, lean)."""
        damping = 1e-3
        fitted = self.evaluate(state)

        for _ in range(FIRST_ARM_REFINEMENTS):
            self.refine_arms(fitted)
            fitted = self.evaluate(state)

        cost, gap = self.cost(fitted["posed"], fitted["matched"], state[1])
        report(0, state, fitted, gap)

        for step in range(1, STEPS + 1):
            a, b = self.system(state, fitted, gap)

            for _ in range(6):
                change = self.solve(a, b, state[1], damping)
                tried = self.apply(state, change)
                fitted_t = self.evaluate(tried)
                cost_t, gap_t = self.cost(fitted_t["posed"], fitted_t["matched"], tried[1])

                if cost_t < cost:
                    state = tried
                    self.refine_arms(fitted_t)
                    fitted = self.evaluate(state)
                    cost, gap = self.cost(fitted["posed"], fitted["matched"], state[1])
                    damping = max(damping / 3, 1e-6)
                    break

                damping *= 4
            else:
                print(f"FIT converged after {step - 1} steps")
                break

            report(step, state, fitted, gap)

        return state, fitted

    def run_placement(self, state, steps):
        """Fits only the placement (depth shift, lean) and the arms' turns, the sliders held."""
        fitted = self.evaluate(state)

        for _ in range(steps):
            self.refine_arms(fitted)
            fitted = self.evaluate(state)
            _, gap = self.cost(fitted["posed"], fitted["matched"], state[1])
            change = np.linalg.lstsq(self.gaps(self.placement(state, fitted), fitted), -gap, rcond=None)[0]
            macros, values, shift, lean = state
            lean += float(np.clip(change[1], -LEAN_MOVE, LEAN_MOVE))
            state = (macros, values, shift + float(change[0]), lean)
            fitted = self.evaluate(state)

        report(steps, state, fitted, self.cost(fitted["posed"], fitted["matched"], state[1])[1])
        return state, fitted

    def system(self, state, fitted, gap):
        """Each unknown's effect on each match's gap: macros by finite differences."""
        macros, values, _, _ = state
        base = fitted["base"]
        columns = []

        for name in macros:
            h = MACRO_STEP if macros[name] + MACRO_STEP <= 1 else -MACRO_STEP
            columns.append((self.human.with_macros(dict(macros, **{name: macros[name] + h})) - base) / h)

        self.human.with_macros(macros)
        columns += [self.human.column(s, v) for s, v in zip(SLIDERS, values)]
        columns = [c @ fitted["tilt"].T for c in columns] + self.placement(state, fitted)
        return self.gaps(columns, fitted), -gap

    def placement(self, state, fitted):
        """The depth shift's and the lean's effect on each vertex."""
        about = fitted["placed"] - fitted["pivot"] - np.array([0.0, state[2], 0.0])
        return [np.tile([0.0, 1.0, 0.0], (len(about), 1)), np.cross(np.array([1.0, 0.0, 0.0]), about)]

    def gaps(self, columns, fitted):
        """Each column's effect on each match's gap, once the arms turn."""
        index, normals = fitted["matched"][0], fitted["matched"][2]
        turned = [np.einsum("nij,nj->ni", fitted["turns"][index], c[index]) for c in columns]
        return np.stack([np.einsum("ij,ij->i", c, normals) for c in turned], axis=1)

    def solve(self, a, b, values, damping):
        count = len(a)
        lhs = a.T @ a / count
        rhs = a.T @ b / count
        sliders = slice(len(MACROS), len(MACROS) + len(values))
        lhs[sliders, sliders] += SLIDER_PULL * np.eye(len(values))
        rhs[sliders] -= SLIDER_PULL * values
        lhs += damping * np.diag(np.diag(lhs) + 1e-12)
        return np.linalg.solve(lhs, rhs)

    def apply(self, state, change):
        macros, values, shift, lean = state
        moved = {
            name: float(np.clip(value + np.clip(change[j], -MACRO_MOVE, MACRO_MOVE), 0.0, 1.0))
            for j, (name, value) in enumerate(macros.items())
        }
        sliders = np.clip(values + change[len(macros) : len(macros) + len(values)], -1.0, 1.0)
        return moved, sliders, shift + float(change[-2]), lean + float(np.clip(change[-1], -LEAN_MOVE, LEAN_MOVE))


def place(body, shift, lean):
    """The body moved along the depth axis and leant forward about its feet."""
    pivot = np.array([0.0, body[body[:, 2] < 0.05, 1].mean(), 0.0])
    cos, sin = np.cos(lean), np.sin(lean)
    tilt = np.array([[1.0, 0.0, 0.0], [0.0, cos, -sin], [0.0, sin, cos]])
    return (body - pivot) @ tilt.T + pivot + np.array([0.0, shift, 0.0]), tilt, pivot


def report(step, state, fitted, gap):
    macros, _, shift, lean = state
    index, points, _ = fitted["matched"]
    distance = np.linalg.norm(fitted["posed"][index] - points, axis=1)
    settings = " ".join(f"{k}={v:.2f}" for k, v in macros.items())
    print(
        f"FIT step {step:2} | along normal rms {1000 * np.sqrt(np.mean(gap**2)):.1f} mm, "
        f"distance mean {1000 * distance.mean():.1f} p95 {1000 * np.percentile(distance, 95):.1f} mm, "
        f"{len(index)} matched | {settings} shift={1000 * shift:.0f}mm lean={np.degrees(lean):.1f}deg"
    )


def regions(fit, fitted):
    """Mean distance per region, millimetres."""
    posed = fitted["posed"]
    index, points, _ = fitted["matched"]
    distance = np.linalg.norm(posed[index] - points, axis=1) * 1000
    height = posed[:, 2].max()
    z = posed[index, 2] / height
    arm = np.isin(index, fit.groups[1])
    bands = {
        "head": (z > 0.87) & ~arm,
        "torso": (z > 0.48) & (z <= 0.87) & ~arm,
        "legs": (z <= 0.48) & ~arm,
        "arms": arm,
    }
    return {name: float(distance[mask].mean()) for name, mask in bands.items() if mask.any()}


def targets(values):
    out = []

    for (negative, positive), value in zip(SLIDERS, values):
        if abs(value) >= 0.01:
            name = negative if value < 0 else positive
            out += [{"target": t, "value": round(abs(float(value)), 3)} for t in sides(name)]

    return out


def main():
    paths = [Path(arg) for arg in sys.argv[sys.argv.index("--") + 1 :]]
    model_path, preset_path, out_path = paths[:3]
    preset = json.loads(preset_path.read_text())
    model = load_model(model_path)
    human = Human(preset)
    fit = Fit(human, model)
    (macros, values, shift, lean), fitted = fit.run((dict(MACROS), np.zeros(len(SLIDERS)), 0.0, 0.0))
    out = merged(preset, {"phenotype": macros})
    out["targets"] = targets(values)
    out_path.write_text(json.dumps(out, indent=1) + "\n")
    by_region = " ".join(f"{k} {v:.1f}" for k, v in regions(fit, fitted).items())
    print(f"FIT mean distance by region (mm): {by_region}")
    print(
        f"FIT wrote {out_path}: depth shift {1000 * shift:.0f} mm, lean {np.degrees(lean):.1f} deg, "
        f"{len(out['targets'])} targets"
    )

    if len(paths) > 3:
        np.savez(paths[3], model=model["positions"], fitted=fitted["posed"])

    for (negative, positive), value in sorted(zip(SLIDERS, values), key=lambda sv: -abs(sv[1]))[:20]:
        print(f"FIT slider {positive if value > 0 else negative:36} {value:+.2f}")


if __name__ == "__main__":
    main()
