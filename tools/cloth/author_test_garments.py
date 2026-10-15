"""Author the cloth lab's test skirt and cape in Blender: drape them over the body with
Blender's cloth simulation, so they rest in real folds, and write what the game needs.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
        --python tools/cloth/author_test_garments.py -- \
        --body assets/local/characters/female_main/female_main_locomotion.glb \
        --out assets/local/characters/female_main

writes `test_skirt.cloth.json` and `test_cape.cloth.json` there (`--garments` picks). The skirt
is draped first: a gathered skirt, half again as much cloth as the waist, drawn in at the
waistband while it drapes over the body, with the arms swung a little out of its way
(`--skirt-arms`; at rest the hands touch the thighs, and in the game they press it through their
colliders). Then the cape, arms down again, over the body and the draped skirt, so the two lie
on each other as worn.

Both are draped in a pose of the body's animations (`--pose`, the idle's first frame: arms
down), not the bind pose: draped over the bind pose's raised arms a cape tents out from them.

A garment is a grid. The game simulates a coarse grid of particles; the mesh it draws is that
grid subdivided `--subdivide` times, draped here and given thickness. Each drawn vertex records
where it lies on the coarse grid, so the game skins it to the four particles around it and the
folds draped here ride on the simulation. The skirt's grid wraps round the body.

Per particle the script writes:
- where it lies as draped, and its bind-space rest position: the drape un-skinned through the
  bones that carry it, so those bones bring it back to the drape in that pose,
- the bones that carry it when nothing else moves it: the body's skin weights where it lies on
  the body, else a blend. The cape: the chest down to the pelvis, never the legs or the arms (a
  cape is not a sleeve: arms move it only by pushing into it, through their colliders). The
  skirt: the pelvis, and lower down partly the thigh of its side (the shins' weights go to the
  thighs, the arms' to the pelvis),
- how free it is: pinned at the collar or waistband, then a maximum distance from its carried
  position that grows towards the hem (the cape's more where it lay on an arm, which it falls
  away from when the arm swings off),
- how much wind reaches it.

Coordinates are written as glTF/game bind space (Y up, the character facing +Z); texture
coordinates are metres on the flat garment, so a fabric tiles at its real size.
"""
import argparse
import json
import math
import sys
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Vector
from mathutils.bvhtree import BVHTree

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
parser = argparse.ArgumentParser()
parser.add_argument("--body", required=True)
parser.add_argument("--out", required=True, help="folder for test_*.cloth.json")
parser.add_argument("--garments", default="skirt,cape")
parser.add_argument("--pose", default="Idle:0", help="ACTION:FRAME the garments are draped in")
parser.add_argument("--subdivide", type=int, default=4)
parser.add_argument("--frames", type=int, default=100)
parser.add_argument("--cape-frames", type=int, help="frames for the cape alone (to look at its start)")
parser.add_argument("--thickness", type=float, default=0.004)
parser.add_argument("--standoff", type=float, default=0.012, help="how far a drape keeps off what it lies on")
parser.add_argument("--cape-columns", type=int, default=15)
parser.add_argument("--cape-rows", type=int, default=13)
parser.add_argument("--spread", type=float, default=110.0, help="half the cape's span round the neck, degrees")
parser.add_argument("--skirt-columns", type=int, default=20)
parser.add_argument("--skirt-rows", type=int, default=7)
parser.add_argument("--skirt-gather", type=float, default=1.5, help="cloth round the skirt per waist round")
parser.add_argument("--skirt-arms", type=float, default=15.0, help="degrees the arms swing out for the skirt")
parser.add_argument("--mirror", action="store_true", help="make the cape's drape symmetric")
parser.add_argument("--blend", help="also save the draped scene here")
args = parser.parse_args(argv)
garments = [g.strip() for g in args.garments.split(",") if g.strip()]

GAME_AXES = Matrix(((1, 0, 0, 0), (0, 0, 1, 0), (0, -1, 0, 0), (0, 0, 0, 1)))
LEGS = ("thigh", "leg_", "foot", "toes")
ARMS = ("arm", "hand", "thumb", "index", "middle", "ring", "pinky")  # anywhere in the name


def to_game(v):
    """Blender (Z up, facing -Y) to game bind space (Y up, facing +Z)."""
    return (v[0], v[2], -v[1])


def to_blender(v):
    return Vector((v[0], -v[2], v[1]))


def smoothstep(low, high, x):
    t = min(max((x - low) / (high - low), 0.0), 1.0)
    return t * t * (3.0 - 2.0 * t)


def is_arm(name):
    return any(part in name for part in ARMS)


bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=args.body)
armature = next(o for o in bpy.data.objects if o.type == "ARMATURE")
body = next(o for o in bpy.data.objects if o.type == "MESH" and o.vertex_groups)

# Hold the body still in the chosen pose while the garments drape.
action_name, pose_frame = args.pose.rsplit(":", 1)
animation = armature.animation_data
for track in animation.nla_tracks:
    track.mute = True


def pose(arms_out=0.0):
    """The chosen pose, with the arms swung out sideways by `arms_out` degrees about the
    shoulders (the rig is flat: every bone of an arm turns on its own)."""
    animation.action = bpy.data.actions[action_name]
    bpy.context.scene.frame_set(int(pose_frame))
    animation.action = None
    bpy.context.view_layer.update()
    if arms_out:
        for side, sign in ((".l", -1.0), (".r", 1.0)):
            pivot = armature.pose.bones["arm_stretch" + side].head.copy()
            turn = (
                Matrix.Translation(pivot)
                @ Matrix.Rotation(math.radians(arms_out) * sign, 4, "Y")
                @ Matrix.Translation(-pivot)
            )
            bones = [b for b in armature.pose.bones if b.name.endswith(side) and is_arm(b.name)]
            targets = {b.name: turn @ b.matrix for b in bones}
            for bone in sorted(bones, key=lambda b: len(b.parent_recursive)):
                bone.matrix = targets[bone.name]
                bpy.context.view_layer.update()
    return bpy.context.evaluated_depsgraph_get()


depsgraph = pose()


def joint(name):
    return to_game(armature.matrix_world @ armature.pose.bones[name].head)


def deform(name):
    """A bone's skinning transform in the pose, world space: pose times inverse bind."""
    world = armature.matrix_world
    bone = armature.pose.bones[name]
    return world @ bone.matrix @ bone.bone.matrix_local.inverted() @ world.inverted()


def game_deform(name):
    """`deform` in game axes, as an affine's four columns."""
    m = GAME_AXES @ deform(name) @ GAME_AXES.inverted()
    return [round(m[row][col], 6) for col in range(4) for row in range(3)]


neck = joint("subneck_twist_1.x")
pelvis = joint("root.x")
knee = joint("leg_stretch.l")
print(f"neck {neck} pelvis {pelvis} knee {knee}")

# The posed body's vertices, game axes, and which are arms (by their strongest bone).
group_names = {g.index: g.name for g in body.vertex_groups}
posed = body.evaluated_get(depsgraph).to_mesh()
body_points = np.array([to_game(body.matrix_world @ v.co) for v in posed.vertices])
body.evaluated_get(depsgraph).to_mesh_clear()
arm_vertex = np.array(
    [
        bool(v.groups) and is_arm(group_names.get(max(v.groups, key=lambda g: g.weight).group, ""))
        for v in body.data.vertices
    ]
)


def collider(obj, thickness=0.006, two_sided=False):
    """Makes `obj` collide with cloth. Collision is one-sided by default (cloth goes to where
    the faces point): right for the body, not for an open garment whose faces point either way."""
    modifier = obj.modifiers.new("Collision", "COLLISION")
    obj.collision.thickness_outer = thickness
    obj.collision.cloth_friction = 5.0
    obj.collision.use_culling = not two_sided
    return modifier


def drape(name, positions, faces, pins, colliders, fit=None, frames=None):
    """Runs Blender's cloth on a sheet (game-axis positions) with `pins` held, against
    `colliders`; `fit` moves pinned vertices to new places (game axes, by index) over the first
    40 frames, the cloth keeping its own lengths, so a wide waistband gathers it. Leaves it as a
    static draped mesh and returns its positions, game axes."""
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([to_blender(p) for p in positions], [], faces)
    mesh.update()
    sheet = bpy.data.objects.new(name, mesh)
    bpy.context.collection.objects.link(sheet)
    pin = sheet.vertex_groups.new(name="pin")
    pin.add(list(pins), 1.0, "REPLACE")
    if fit:
        sheet.shape_key_add(name="Basis")
        key = sheet.shape_key_add(name="fit")
        for i, p in fit.items():
            key.data[i].co = to_blender(p)
        key.value = 0.0
        key.keyframe_insert("value", frame=1)
        key.value = 1.0
        key.keyframe_insert("value", frame=40)
    cloth = sheet.modifiers.new("Cloth", "CLOTH")
    cloth.settings.vertex_group_mass = "pin"
    cloth.settings.quality = 10
    cloth.settings.mass = 0.4
    cloth.settings.tension_stiffness = 20.0
    cloth.settings.compression_stiffness = 20.0
    cloth.settings.shear_stiffness = 10.0
    cloth.settings.bending_stiffness = 1.0
    cloth.settings.air_damping = 3.0
    cloth.collision_settings.distance_min = args.standoff
    cloth.collision_settings.use_self_collision = True
    cloth.collision_settings.self_distance_min = 0.005
    cloth.collision_settings.collision_quality = 6
    frames = frames or args.frames
    cloth.point_cache.frame_start = 1
    cloth.point_cache.frame_end = frames
    for obj in colliders:
        if "Collision" not in obj.modifiers:
            collider(obj)
    scene = bpy.context.scene
    scene.frame_start = 1
    scene.frame_end = frames
    for frame in range(1, frames + 1):
        scene.frame_set(frame)
        if frame % 25 == 0:
            print(f"{name}: draped frame {frame}/{frames}")
    graph = bpy.context.evaluated_depsgraph_get()
    evaluated = sheet.evaluated_get(graph).to_mesh()
    draped = np.array([to_game(v.co) for v in evaluated.vertices], dtype=np.float64)
    sheet.evaluated_get(graph).to_mesh_clear()
    # A fresh static mesh of the drape.
    bpy.data.objects.remove(sheet)
    still = bpy.data.meshes.new(name)
    still.from_pydata([to_blender(p) for p in draped], [], faces)
    still.update()
    sheet = bpy.data.objects.new(name, still)
    bpy.context.collection.objects.link(sheet)
    scene.frame_set(1)
    return sheet, draped


def grid_faces(rows, columns, wrap):
    across = columns if wrap else columns - 1
    faces = []
    for r in range(rows - 1):
        for c in range(across):
            a, b = r * columns + c, r * columns + (c + 1) % columns
            faces.append((a, b, b + columns, a + columns))
    return faces


def thick_mesh(fine, rows, columns, wrap, grid, uvs, seam_u):
    """The drape (rows × columns, game axes) given thickness: an outer shell, an inner one
    towards the body and a rim round the open edges. A wrapped sheet's first column is
    repeated at the end with the texture's seam value (`seam_u`), grid column `columns / k`."""
    shape = fine.reshape(rows, columns, 3)
    if wrap:
        du = np.roll(shape, -1, axis=1) - np.roll(shape, 1, axis=1)
    else:
        du = np.gradient(shape, axis=1)
    dv = np.gradient(shape, axis=0)
    normals = np.cross(du, dv)
    normals /= np.linalg.norm(normals, axis=2, keepdims=True) + 1e-12
    # The sheet is one surface: turn it as a whole to face away from the body's axis. (Turned
    # vertex by vertex, fold walls edge-on to that direction came out facing either way, and
    # the inner shell crossed the outer one there.)
    away = shape.copy()
    away[..., 0] -= pelvis[0]
    away[..., 1] = 0.0
    away[..., 2] -= pelvis[2]
    if np.sum(normals * away) < 0.0:
        normals = -normals
    grid = np.array(grid).reshape(rows, columns, 2)
    uvs = np.array(uvs).reshape(rows, columns, 2)
    if wrap:
        shape = np.concatenate([shape, shape[:, :1]], axis=1)
        normals = np.concatenate([normals, normals[:, :1]], axis=1)
        seam = grid[:, :1].copy()
        seam[..., 0] = grid[0, -1, 0] + (grid[0, -1, 0] - grid[0, -2, 0])
        grid = np.concatenate([grid, seam], axis=1)
        seam_uv = uvs[:, :1].copy()
        seam_uv[..., 0] = seam_u
        uvs = np.concatenate([uvs, seam_uv], axis=1)
        columns += 1
    outer = shape.reshape(-1, 3)
    normals = normals.reshape(-1, 3)
    inner = outer - normals * args.thickness
    grid = grid.reshape(-1, 2)
    uvs = uvs.reshape(-1, 2)
    n = len(outer)
    positions = [outer, inner]
    all_normals = [normals, -normals]
    all_grid = [grid, grid]
    all_uvs = [uvs, uvs]
    indices = []
    for r in range(rows - 1):
        for c in range(columns - 1):
            a = r * columns + c
            quad = (a, a + columns, a + columns + 1, a + 1)
            indices += [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]
            q = [i + n for i in quad]
            indices += [q[0], q[2], q[1], q[0], q[3], q[2]]
    edges = [(c, c + 1) for c in range(columns - 1)]  # collar or waistband
    last = (rows - 1) * columns
    edges += [(last + c + 1, last + c) for c in range(columns - 1)]  # hem
    if not wrap:
        edges += [((r + 1) * columns, r * columns) for r in range(rows - 1)]
        edges += [(r * columns + columns - 1, (r + 1) * columns + columns - 1) for r in range(rows - 1)]
    base = 2 * n
    rim = ([], [], [], [])
    for a, b in edges:
        side = np.cross(outer[b] - outer[a], normals[a] + normals[b])
        side /= np.linalg.norm(side) + 1e-12
        first = base + len(rim[0])
        for v, k in ((outer[a], a), (outer[b], b), (inner[b], b), (inner[a], a)):
            rim[0].append(v)
            rim[1].append(side)
            rim[2].append(grid[k])
            rim[3].append(uvs[k])
        indices += [first, first + 1, first + 2, first, first + 2, first + 3]
    for target, extra in zip((positions, all_normals, all_grid, all_uvs), rim):
        target.append(np.array(extra).reshape(-1, target[0].shape[1]))
    return {
        "positions": np.concatenate(positions).round(5).tolist(),
        "normals": np.concatenate(all_normals).round(4).tolist(),
        "uvs": np.concatenate(all_uvs).round(4).tolist(),
        "grid": np.concatenate(all_grid).round(4).tolist(),
        "indices": indices,
    }


bvh = BVHTree.FromObject(body, depsgraph)


def surface_weights(polygon, remap):
    """The skin weights of a body face, averaged over its corners, each bone renamed by
    `remap` (None drops it); and the share that was the arms'."""
    total = {}
    arm = 0.0
    corners = body.data.polygons[polygon].vertices
    for v in corners:
        for g in body.data.vertices[v].groups:
            name = group_names.get(g.group)
            if not name or g.weight <= 0.0:
                continue
            w = g.weight / len(corners)
            if is_arm(name):
                arm += w
            name = remap(name)
            if name:
                total[name] = total.get(name, 0.0) + w
    return total, arm


def top(weights, count=4):
    best = sorted(weights.items(), key=lambda w: -w[1])[:count]
    s = sum(w for _, w in best)
    return [[name, w / s] for name, w in best] if s > 0 else []


def particle(p, weights, pinned, max_distance, goal, exposure):
    bones = top(weights)
    # Un-skinned: the blend of its bones' transforms, inverted.
    blend = sum((deform(name) * w for name, w in bones[1:]), deform(bones[0][0]) * bones[0][1])
    rest = to_game(blend.inverted() @ to_blender(p))
    return {
        "rest": [float(x) for x in rest],
        "drape": [float(x) for x in p],
        "bones": bones,
        "pinned": pinned,
        "max_distance": max_distance,
        "goal": goal,
        "exposure": exposure,
    }


def write(name, columns, rows, wrap, particles, mesh, pose_transforms=None):
    path = Path(args.out) / f"test_{name}.cloth.json"
    result = {
        "version": 1,
        "columns": columns,
        "rows": rows,
        "wrap": wrap,
        "thickness": args.thickness,
        # Each bone's skinning transform in the pose draped in, so the game can tell which of
        # its colliders the drape rests against.
        "pose": pose_transforms or {bone.name: game_deform(bone.name) for bone in armature.pose.bones},
        "particles": particles,
        "mesh": mesh,
    }
    path.write_text(json.dumps(result, separators=(",", ":")))
    print(
        f"wrote {path}: {len(particles)} particles, {len(mesh['positions'])} vertices, "
        f"{len(mesh['indices']) // 3} triangles"
    )


k = args.subdivide
colliders = [body]
skirt_object = None

if "skirt" in garments:
    # A gathered skirt: a tube of cloth half again as round as the waist (and clear of the hips
    # and legs, arms left out), from the waist to below the knees; its waistband is drawn in to
    # the waist while it drapes, so the cloth falls in folds over the hips.
    columns, rows = args.skirt_columns, args.skirt_rows
    fine_columns, fine_rows = columns * k, (rows - 1) * k + 1
    trunk = body_points[~arm_vertex]
    band = trunk[np.abs(trunk[:, 1] - pelvis[1]) < 0.06]
    centre = (float(band[:, 0].mean()), float(band[:, 2].mean()))
    top_y, hem_y = pelvis[1] + 0.05, knee[1] - 0.08
    heights = [top_y + (hem_y - top_y) * r / (fine_rows - 1) for r in range(fine_rows)]
    angles = [2.0 * math.pi * (c + 0.5) / fine_columns for c in range(fine_columns)]
    directions = np.array([(math.sin(a), math.cos(a)) for a in angles])
    offsets = trunk[:, [0, 2]] - np.array(centre)

    def support(y):
        near = np.abs(trunk[:, 1] - y) < 0.015
        return (offsets[near] @ directions.T).max(axis=0) if near.any() else np.zeros(fine_columns)

    waist = support(top_y) + 0.01
    widest = max(float(support(y).max()) for y in heights)
    radius = max(args.skirt_gather * float(waist.mean()), widest + 0.05)
    print(f"skirt: waist {waist.mean():.3f} m round the axis, cloth radius {radius:.3f} m")
    positions, grid = [], []
    for r, y in enumerate(heights):
        for c, (s, z) in enumerate(directions):
            positions.append((centre[0] + s * radius, y, centre[1] + z * radius))
            grid.append((c / k, r / k))
    fit = {c: (centre[0] + s * waist[c], top_y, centre[1] + z * waist[c]) for c, (s, z) in enumerate(directions)}
    # Metres round the flat skirt, a whole number round so a fabric meets itself at the seam,
    # and down it.
    around = max(round(2.0 * math.pi * radius), 1)
    uvs = [(around * c / fine_columns, top_y - y) for y in heights for c in range(fine_columns)]
    depsgraph = pose(args.skirt_arms)
    skirt_pose = {bone.name: game_deform(bone.name) for bone in armature.pose.bones}
    skirt_object, fine = drape(
        "skirt", positions, grid_faces(fine_rows, fine_columns, True), range(fine_columns), [body], fit
    )
    depsgraph = pose()

    def skirt_bones(name):
        if is_arm(name):
            return "root.x"
        for leg in ("leg_", "foot", "toes"):
            if name.startswith(leg):
                return "thigh_stretch" + name[-2:]
        return name

    particles = []
    for r in range(rows):
        t = r / (rows - 1)
        for c in range(columns):
            p = fine[r * k * fine_columns + c * k]
            # The pelvis, handing over to the thigh of its side lower down.
            toward_left = smoothstep(-0.12, 0.12, p[0])
            legs = 0.45 * t
            weights = {
                "root.x": 1.0 - legs,
                "thigh_stretch.l": legs * toward_left,
                "thigh_stretch.r": legs * (1.0 - toward_left),
            }
            _, _, polygon, distance = bvh.find_nearest(to_blender(p))
            # Where it lies on the body, half its carriers are the body's under it.
            near = 0.5 * (1.0 - smoothstep(0.03, 0.08, distance if distance is not None else 1.0))
            if near > 0.0 and polygon is not None:
                weights = {name: w * (1.0 - near) for name, w in weights.items()}
                for name, w in surface_weights(polygon, skirt_bones)[0].items():
                    weights[name] = weights.get(name, 0.0) + w * near
            particles.append(
                particle(
                    p,
                    weights,
                    pinned=r == 0,
                    max_distance=0.0 if r == 0 else 0.02 + 0.3 * t,
                    goal=0.0,
                    exposure=0.6,
                )
            )
    write(
        "skirt",
        columns,
        rows,
        True,
        particles,
        thick_mesh(fine, fine_rows, fine_columns, True, grid, uvs, float(around)),
        skirt_pose,
    )
    collider(skirt_object, thickness=0.015, two_sided=True)
    colliders.append(skirt_object)
    # For the cape's drape only: a ring closing the waistband to the body, sloping up into it so
    # cloth slides off outward, or the cape slides down the back into the skirt.
    rim = fine[:fine_columns]
    inside = [(centre[0] + (p[0] - centre[0]) * 0.5, p[1] + 0.04, centre[1] + (p[2] - centre[1]) * 0.5) for p in rim]
    lid_mesh = bpy.data.meshes.new("waistband_lid")
    lid_mesh.from_pydata(
        [to_blender(p) for p in list(rim) + inside],
        [],
        [(c, (c + 1) % fine_columns, fine_columns + (c + 1) % fine_columns, fine_columns + c) for c in range(fine_columns)],
    )
    lid = bpy.data.objects.new("waistband_lid", lid_mesh)
    bpy.context.collection.objects.link(lid)
    collider(lid, thickness=0.015, two_sided=True)
    colliders.append(lid)

if "cape" in garments:
    # A collar round the neck, a slope out over the shoulders, then straight down to below the
    # knees, open at the front, clear of the arms hanging at the sides.
    columns, rows = args.cape_columns, args.cape_rows
    fine_columns, fine_rows = (columns - 1) * k + 1, (rows - 1) * k + 1
    spread = math.radians(args.spread)
    collar = 0.09
    collar_y = neck[1] - 0.01
    droop = math.radians(25.0)
    hem_y = knee[1] - 0.15
    slope_out = (0.31 - collar, 0.24 - collar * 0.9)  # across, behind
    slope_length = slope_out[0] / math.cos(droop)
    length = slope_length + (collar_y - slope_length * math.sin(droop) - hem_y)
    # Over a skirt, each column hangs 4 cm clear of it to start with.
    widen = np.ones(fine_columns)
    if skirt_object is not None:
        cloth = np.array([to_game(v.co) for v in skirt_object.data.vertices])[:, [0, 2]] - np.array([0.0, neck[2]])
        for c in range(fine_columns):
            theta = -spread + 2.0 * spread * c / (fine_columns - 1)
            out = np.array([math.sin(theta), -math.cos(theta)])
            reach = float((cloth @ out).max()) + 0.04
            end = math.hypot(math.sin(theta) * (collar + slope_out[0]), math.cos(theta) * (collar * 0.9 + slope_out[1]))
            widen[c] = max(1.0, reach / end)
    positions, grid, uvs = [], [], []
    for r in range(fine_rows):
        s = r / (fine_rows - 1) * length
        down = min(s, slope_length)
        for c in range(fine_columns):
            theta = -spread + 2.0 * spread * c / (fine_columns - 1)
            sx, kz = math.sin(theta), math.cos(theta)
            f = down / slope_length
            rx = collar + ((collar + slope_out[0]) * widen[c] - collar) * f
            rz = collar * 0.9 + ((collar * 0.9 + slope_out[1]) * widen[c] - collar * 0.9) * f
            y = collar_y - down * math.sin(droop) - max(s - slope_length, 0.0)
            positions.append((sx * rx, y, neck[2] - kz * rz))
            grid.append((c / k, r / k))
        # The flat cape: metres along the row from its middle, and down from the collar.
        row = np.array(positions[-fine_columns:])
        along = np.concatenate([[0.0], np.cumsum(np.linalg.norm(np.diff(row, axis=0), axis=1))])
        uvs += [(float(a - along[fine_columns // 2]), s) for a in along]
    pins = range(fine_columns)
    _, fine = drape("cape", positions, grid_faces(fine_rows, fine_columns, False), pins, colliders, frames=args.cape_frames)
    # Cloth settles a little differently on each side; for a symmetric pose, keep the half that
    # hangs lower, more relaxed, and mirror it.
    if args.mirror:
        shape = fine.reshape(fine_rows, fine_columns, 3)
        middle = fine_columns // 2
        right_low = shape[:, :middle, 1].mean() <= shape[:, middle + 1 :, 1].mean()
        for c in range(middle + 1):
            keep, mirror = (c, fine_columns - 1 - c) if right_low else (fine_columns - 1 - c, c)
            shape[:, mirror] = shape[:, keep] * np.array([-1.0, 1.0, 1.0])
        shape[:, middle, 0] = 0.0
        fine = shape.reshape(-1, 3)

    def cape_bones(name):
        if any(name.startswith(leg) for leg in LEGS):
            return "root.x"
        if is_arm(name):
            return "spine_05.x"
        return name

    particles = []
    for r in range(rows):
        t = r / (rows - 1)
        for c in range(columns):
            p = fine[r * k * fine_columns + c * k]
            theta = -spread + 2.0 * spread * c / (columns - 1)
            edge_share = smoothstep(0.8, 1.0, abs(theta) / spread)
            _, _, polygon, distance = bvh.find_nearest(to_blender(p))
            # Down the back the chest hands over to the pelvis.
            fallback = {"spine_05.x": 1.0 - 0.6 * t, "root.x": 0.6 * t}
            near = 1.0 - smoothstep(0.03, 0.08, distance if distance is not None else 1.0)
            weights = {name: w * (1.0 - near) for name, w in fallback.items()}
            on_arm = 0.0
            if near > 0.0 and polygon is not None:
                surface, arm = surface_weights(polygon, cape_bones)
                for name, w in surface.items():
                    weights[name] = weights.get(name, 0.0) + w * near
                on_arm = arm * near
            pinned = r == 0
            free = smoothstep(0.08, 1.0, t)
            particles.append(
                particle(
                    p,
                    weights,
                    pinned=pinned,
                    max_distance=0.0 if pinned else max(0.02 + 0.55 * free**1.2, 0.2 * on_arm),
                    goal=0.0 if pinned else 6.0 * (1.0 - free) * (1.0 - on_arm),
                    exposure=0.0 if pinned else max(0.1 + 0.9 * smoothstep(0.15, 1.0, t), 0.6 * edge_share),
                )
            )
    write("cape", columns, rows, False, particles, thick_mesh(fine, fine_rows, fine_columns, False, grid, uvs, 0.0))

if args.blend:
    bpy.ops.wm.save_as_mainfile(filepath=args.blend)
