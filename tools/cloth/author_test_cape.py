"""Author the cloth lab's test cape in Blender: drape a cape over the body with Blender's cloth
simulation, so it rests in real folds, and write what the game needs.

It is draped in a pose of the body's animations (`--pose`, the idle's first frame: arms down),
not the bind pose: draped over the bind pose's raised arms a cape tents out from them.

    /Applications/Blender.app/Contents/MacOS/Blender -b --factory-startup \
        --python tools/cloth/author_test_cape.py -- \
        --body assets/local/characters/female_main/female_main_locomotion.glb \
        --out assets/local/characters/female_main/test_cape.cloth.json

The cape is a grid. The game simulates a coarse grid of particles (`--columns` x `--rows`); the
mesh it draws is that grid subdivided `--subdivide` times, draped here and given thickness. Each
drawn vertex records where it lies on the coarse grid, so the game skins it to the four particles
around it and the folds draped here ride on the simulation.

Per particle the script writes:
- where it lies as draped, and its bind-space rest position: the drape un-skinned through the
  bones that carry it, so those bones bring it back to the drape in that pose,
- the bones that carry it when nothing else moves it: the body's skin weights where the cape
  lies on the body, else the chest blending to the pelvis down the back; never the legs' or the
  arms' (a cape is not a sleeve: arms move it only by pushing into it, through their colliders),

- how free it is: pinned at the collar, then a maximum distance from its carried position that
  grows towards the hem, and more where it lay on an arm (it falls in when the arm swings away),
- how much wind reaches it: none at the collar, all at the hem.

Coordinates are written as glTF/game bind space (Y up, the character facing +Z); texture
coordinates are metres on the flat cape, so a fabric tiles at its real size.
"""
import argparse
import json
import math
import sys

import bpy
import numpy as np
from mathutils import Matrix, Vector
from mathutils.bvhtree import BVHTree

argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
parser = argparse.ArgumentParser()
parser.add_argument("--body", required=True)
parser.add_argument("--out", required=True)
parser.add_argument("--columns", type=int, default=15)
parser.add_argument("--rows", type=int, default=13)
parser.add_argument("--subdivide", type=int, default=4)
parser.add_argument("--spread", type=float, default=110.0, help="half the cape's span round the neck, degrees")
parser.add_argument("--frames", type=int, default=100)
parser.add_argument("--thickness", type=float, default=0.004)
parser.add_argument("--standoff", type=float, default=0.012, help="how far the drape keeps off the skin")
parser.add_argument("--pose", default="Idle:0", help="ACTION:FRAME the cape is draped in")
parser.add_argument("--mirror", action="store_true", help="make the drape symmetric")
parser.add_argument("--blend", help="also save the draped scene here")
args = parser.parse_args(argv)


def to_game(v):
    """Blender (Z up, facing -Y) to game bind space (Y up, facing +Z)."""
    return (v[0], v[2], -v[1])


def to_blender(v):
    return Vector((v[0], -v[2], v[1]))


def smoothstep(low, high, x):
    t = min(max((x - low) / (high - low), 0.0), 1.0)
    return t * t * (3.0 - 2.0 * t)


bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.ops.import_scene.gltf(filepath=args.body)
armature = next(o for o in bpy.data.objects if o.type == "ARMATURE")
body = next(o for o in bpy.data.objects if o.type == "MESH" and o.vertex_groups)
# Hold the body still in the chosen pose while the cape drapes.
action_name, pose_frame = args.pose.rsplit(":", 1)
animation = armature.animation_data
for track in animation.nla_tracks:
    track.mute = True
animation.action = bpy.data.actions[action_name]
bpy.context.scene.frame_set(int(pose_frame))
animation.action = None
bpy.context.view_layer.update()


def joint(name):
    return to_game(armature.matrix_world @ armature.pose.bones[name].head)


def deform(name):
    """A bone's skinning transform in the pose, world space: pose times inverse bind."""
    world = armature.matrix_world
    bone = armature.pose.bones[name]
    return world @ bone.matrix @ bone.bone.matrix_local.inverted() @ world.inverted()


GAME_AXES = Matrix(((1, 0, 0, 0), (0, 0, 1, 0), (0, -1, 0, 0), (0, 0, 0, 1)))


def game_deform(name):
    """`deform` in game axes, as an affine's four columns."""
    m = GAME_AXES @ deform(name) @ GAME_AXES.inverted()
    return [round(m[row][col], 6) for col in range(4) for row in range(3)]


neck = joint("subneck_twist_1.x")
pelvis = joint("root.x")
knee = joint("leg_stretch.l")
print(f"neck {neck} pelvis {pelvis} knee {knee}")

# The cape before draping: a collar round the neck, a slope out over the shoulders, then
# straight down to below the knees, open at the front, clear of the arms hanging at the sides.
k = args.subdivide
fine_columns = (args.columns - 1) * k + 1
fine_rows = (args.rows - 1) * k + 1
spread = math.radians(args.spread)
collar = 0.09
collar_y = neck[1] - 0.01
droop = math.radians(25.0)
hem_y = knee[1] - 0.15
slope_out = (0.31 - collar, 0.24 - collar * 0.9)  # across, behind
slope_length = slope_out[0] / math.cos(droop)
length = slope_length + (collar_y - slope_length * math.sin(droop) - hem_y)
positions = []
grid = []
uvs = []
for r in range(fine_rows):
    s = r / (fine_rows - 1) * length
    down = min(s, slope_length)
    for c in range(fine_columns):
        theta = -spread + 2.0 * spread * c / (fine_columns - 1)
        sx, kz = math.sin(theta), math.cos(theta)
        f = down / slope_length
        rx = collar + slope_out[0] * f
        rz = collar * 0.9 + slope_out[1] * f
        y = collar_y - down * math.sin(droop) - max(s - slope_length, 0.0)
        positions.append(to_blender((sx * rx, y, neck[2] - kz * rz)))
        grid.append((c / k, r / k))
    # The flat cape: metres along the row from its middle, and down from the collar.
    row = np.array([to_game(v) for v in positions[-fine_columns:]])
    along = np.concatenate([[0.0], np.cumsum(np.linalg.norm(np.diff(row, axis=0), axis=1))])
    uvs += [(float(a - along[fine_columns // 2]), s) for a in along]

mesh = bpy.data.meshes.new("cape")
faces = []
for r in range(fine_rows - 1):
    for c in range(fine_columns - 1):
        a = r * fine_columns + c
        faces.append((a, a + 1, a + fine_columns + 1, a + fine_columns))
mesh.from_pydata(positions, [], faces)
mesh.update()
cape = bpy.data.objects.new("cape", mesh)
bpy.context.collection.objects.link(cape)
pin = cape.vertex_groups.new(name="pin")
pin.add(list(range(fine_columns)), 1.0, "REPLACE")

cloth = cape.modifiers.new("Cloth", "CLOTH")
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
cloth.collision_settings.collision_quality = 4
cloth.point_cache.frame_start = 1
cloth.point_cache.frame_end = args.frames
collision = body.modifiers.new("Collision", "COLLISION")
body.collision.thickness_outer = 0.006
body.collision.cloth_friction = 5.0
scene = bpy.context.scene
scene.frame_start = 1
scene.frame_end = args.frames
for frame in range(1, args.frames + 1):
    scene.frame_set(frame)
    if frame % 20 == 0:
        print(f"draped frame {frame}/{args.frames}")

depsgraph = bpy.context.evaluated_depsgraph_get()
draped = cape.evaluated_get(depsgraph).to_mesh()
fine = np.array([to_game(v.co) for v in draped.vertices], dtype=np.float64)
cape.evaluated_get(depsgraph).to_mesh_clear()

# Cloth settles a little differently on each side (one side catches on an arm); for a pose that
# is symmetric, keep the half that hangs lower, more relaxed, and mirror it.
if args.mirror:
    shape = fine.reshape(fine_rows, fine_columns, 3)
    middle = fine_columns // 2
    right_low = shape[:, :middle, 1].mean() <= shape[:, middle + 1 :, 1].mean()
    for c in range(middle + 1):
        keep, mirror = (c, fine_columns - 1 - c) if right_low else (fine_columns - 1 - c, c)
        shape[:, mirror] = shape[:, keep] * np.array([-1.0, 1.0, 1.0])
    shape[:, middle, 0] = 0.0
    fine = shape.reshape(-1, 3)
mesh.vertices.foreach_set("co", np.array([to_blender(p) for p in fine]).ravel())
cape.modifiers.remove(cloth)
if args.blend:
    bpy.ops.wm.save_as_mainfile(filepath=args.blend)

# Shading normals of the draped sheet, facing away from the body's upright axis.
shape = fine.reshape(fine_rows, fine_columns, 3)
du = np.gradient(shape, axis=1)
dv = np.gradient(shape, axis=0)
normals = np.cross(du, dv)
normals /= np.linalg.norm(normals, axis=2, keepdims=True) + 1e-12
away = shape.copy()
away[..., 0] -= 0.0
away[..., 1] = 0.0
away[..., 2] -= pelvis[2]
flip = np.sum(normals * away, axis=2) < 0.0
normals[flip] *= -1.0
normals = normals.reshape(-1, 3)

# Thickness: an inner shell (towards the body) and a rim round the edge.
outer = fine
inner = fine - normals * args.thickness
n = len(fine)
out_positions = np.concatenate([outer, inner])
out_normals = np.concatenate([normals, -normals])
out_grid = grid + grid
out_uvs = uvs + uvs
indices = []
for r in range(fine_rows - 1):
    for c in range(fine_columns - 1):
        a = r * fine_columns + c
        quad = (a, a + fine_columns, a + fine_columns + 1, a + 1)
        indices += [quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]
        q = [i + n for i in quad]
        indices += [q[0], q[2], q[1], q[0], q[3], q[2]]
# Rim: each boundary edge as a quad between the shells, with its own vertices facing out.
edge = []
edge += [(c, c + 1) for c in range(fine_columns - 1)]  # collar
last = (fine_rows - 1) * fine_columns
edge += [(last + c + 1, last + c) for c in range(fine_columns - 1)]  # hem
edge += [((r + 1) * fine_columns, r * fine_columns) for r in range(fine_rows - 1)]  # one side
edge += [(r * fine_columns + fine_columns - 1, (r + 1) * fine_columns + fine_columns - 1) for r in range(fine_rows - 1)]
rim_positions, rim_normals, rim_grid, rim_uvs = [], [], [], []
for a, b in edge:
    side = np.cross(outer[b] - outer[a], normals[a] + normals[b])
    side /= np.linalg.norm(side) + 1e-12
    base = len(out_positions) + len(rim_positions)
    for v in (outer[a], outer[b], inner[b], inner[a]):
        rim_positions.append(v)
        rim_normals.append(side)
    rim_grid += [grid[a], grid[b], grid[b], grid[a]]
    rim_uvs += [uvs[a], uvs[b], uvs[b], uvs[a]]
    indices += [base, base + 1, base + 2, base, base + 2, base + 3]
out_positions = np.concatenate([out_positions, np.array(rim_positions)])
out_normals = np.concatenate([out_normals, np.array(rim_normals)])
out_grid = out_grid + rim_grid
out_uvs += rim_uvs

# Particles: the coarse grid's vertices of the drape.
bvh = BVHTree.FromObject(body, depsgraph)
body_mesh = body.data
group_names = {g.index: g.name for g in body.vertex_groups}
legs = ("thigh", "leg_", "foot", "toes")
arms = ("arm", "hand", "thumb", "index", "middle", "ring", "pinky")  # anywhere in the name


def body_weights(polygon):
    """The skin weights of a body face, averaged over its corners, the legs' given to the pelvis
    and the arms' to the chest; and the share that was the arms'."""
    total = {}
    arm = 0.0
    corners = body_mesh.polygons[polygon].vertices
    for v in corners:
        for g in body_mesh.vertices[v].groups:
            name = group_names.get(g.group)
            if name and g.weight > 0.0:
                w = g.weight / len(corners)
                if name.startswith(legs):
                    name = "root.x"
                elif any(part in name for part in arms):
                    name = "spine_05.x"
                    arm += w
                total[name] = total.get(name, 0.0) + w
    return total, arm


def top(weights, count=4):
    best = sorted(weights.items(), key=lambda w: -w[1])[:count]
    s = sum(w for _, w in best)
    return [[name, w / s] for name, w in best] if s > 0 else []


particles = []
for r in range(args.rows):
    t = r / (args.rows - 1)
    for c in range(args.columns):
        p = fine[r * k * fine_columns + c * k]  # as draped
        theta = -spread + 2.0 * spread * c / (args.columns - 1)
        edge_share = smoothstep(0.8, 1.0, abs(theta) / spread)
        location, _, polygon, distance = bvh.find_nearest(to_blender(p))
        # Down the back the chest hands over to the pelvis.
        fallback = {"spine_05.x": 1.0 - 0.6 * t, "root.x": 0.6 * t}
        near = 1.0 - smoothstep(0.03, 0.08, distance if distance is not None else 1.0)
        weights = dict((name, w * (1.0 - near)) for name, w in fallback.items())
        on_arm = 0.0
        if near > 0.0 and polygon is not None:
            surface, arm = body_weights(polygon)
            for name, w in surface.items():
                weights[name] = weights.get(name, 0.0) + w * near
            on_arm = arm * near
        pinned = r == 0
        free = smoothstep(0.08, 1.0, t)
        bones = top(weights)
        # Un-skinned: the blend of its bones' transforms, inverted.
        blend = sum((deform(name) * w for name, w in bones[1:]), deform(bones[0][0]) * bones[0][1])
        rest = to_game(blend.inverted() @ to_blender(p))
        particles.append(
            {
                "rest": [float(x) for x in rest],
                "drape": [float(x) for x in p],
                "bones": bones,
                "pinned": pinned,
                "max_distance": 0.0 if pinned else max(0.02 + 0.55 * free**1.2, 0.2 * on_arm),
                "goal": 0.0 if pinned else 6.0 * (1.0 - free) * (1.0 - on_arm),
                "exposure": 0.0 if pinned else max(0.1 + 0.9 * smoothstep(0.15, 1.0, t), 0.6 * edge_share),
            }
        )

result = {
    "version": 1,
    "columns": args.columns,
    "rows": args.rows,
    "thickness": args.thickness,
    # Each bone's skinning transform in the pose draped in, so the game can tell which of its
    # colliders the drape rests against.
    "pose": {bone.name: game_deform(bone.name) for bone in armature.pose.bones},
    "particles": particles,
    "mesh": {
        "positions": out_positions.round(5).tolist(),
        "normals": out_normals.round(4).tolist(),
        "uvs": np.array(out_uvs).round(4).tolist(),
        "grid": np.array(out_grid).round(4).tolist(),
        "indices": indices,
    },
}
with open(args.out, "w") as f:
    json.dump(result, f, separators=(",", ":"))
print(
    f"wrote {args.out}: {len(particles)} particles, {len(out_positions)} vertices, "
    f"{len(indices) // 3} triangles"
)
