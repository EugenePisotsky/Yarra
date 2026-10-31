"""Clips moved from the bought body's rig onto MPFB's game-engine skeleton.

The source is female_main's locomotion rig, an Auto-Rig Pro export. Each game-engine bone takes
the world-space turn of one source bone, measured from a reference pose: the target posed so its
limbs point where the source's do at rest. The two rests differ (MPFB holds its arms lower and its
fingers curled), and turning both from poses that agree keeps the limbs where the source puts
them. Only turns carry over, and the pelvis's rise and sway scaled by leg length: the target's own
bone lengths place its joints.

The spines differ more: the source has five bones and a short neck where the target has three and
a long one, so a bone's turn would swing a longer lever and carry the head and shoulders too far.
Each target spine bone and the neck therefore keep the source's twist but aim at where the next
joint belongs: the point of the source's spine at that joint's rest height, plus how far the
joint stands from that point at rest. The pelvis keeps the source's turn, or the feet would slide
with no IK to hold them. export_glb.py calls it.
"""
from collections import namedtuple

import bpy
from mathutils import Matrix

SIDES = ("l", "r")

FINGERS = ("index", "middle", "ring", "pinky")

# A target bone turned in the reference pose so that the line from its head to `toward`'s head
# runs along the source's line between the joints `source`; with `across`, a second pair of
# target and source joints settles the turn about that line (the palm's facing).
Aim = namedtuple("Aim", "bone toward source across", defaults=(None,))


def source_bones():
    """Each game-engine bone and the source bone whose turn it takes."""
    bones = {
        "pelvis": "root.x",
        "spine_01": "spine_02.x",
        "spine_02": "spine_03.x",
        # The chest carries the shoulders and the neck, so it takes the upper chest's turn.
        "spine_03": "spine_05.x",
        "neck_01": "neck.x",
        "head": "head.x",
    }

    for s in SIDES:
        bones |= {
            f"clavicle_{s}": f"shoulder.{s}",
            f"upperarm_{s}": f"arm_stretch.{s}",
            f"lowerarm_{s}": f"forearm_stretch.{s}",
            f"hand_{s}": f"hand.{s}",
            f"thumb_01_{s}": f"thumb1.{s}",
            f"thumb_02_{s}": f"c_thumb2.{s}",
            f"thumb_03_{s}": f"c_thumb3.{s}",
            f"thigh_{s}": f"thigh_stretch.{s}",
            f"calf_{s}": f"leg_stretch.{s}",
            f"foot_{s}": f"foot.{s}",
            f"ball_{s}": f"toes_01.{s}",
        }

        for finger in FINGERS:
            bones |= {
                f"{finger}_01_{s}": f"{finger}1.{s}",
                f"{finger}_02_{s}": f"c_{finger}2.{s}",
                f"{finger}_03_{s}": f"c_{finger}3.{s}",
            }

    return bones


# The target's spine bones, feet to skull, and the source's spine joints the same way; the source's
# spine runs on to its head bone's tail.
TARGET_SPINE = ("spine_01", "spine_02", "spine_03", "neck_01", "head")
SOURCE_SPINE = ("spine_02.x", "spine_03.x", "spine_04.x", "spine_05.x", "neck.x", "head.x")


def aims():
    """The limbs' aims, each bone after its parent. The source's upper arm starts at its twist
    bone's head, its thigh at the thigh twist's."""
    out = []

    for s in SIDES:
        out += [
            Aim(f"thigh_{s}", f"calf_{s}", (f"thigh_twist.{s}", f"leg_stretch.{s}")),
            Aim(f"calf_{s}", f"foot_{s}", (f"leg_stretch.{s}", f"foot.{s}")),
            Aim(f"foot_{s}", f"ball_{s}", (f"foot.{s}", f"toes_01.{s}")),
            Aim(f"upperarm_{s}", f"lowerarm_{s}", (f"c_arm_twist_offset.{s}", f"forearm_stretch.{s}")),
            Aim(f"lowerarm_{s}", f"hand_{s}", (f"forearm_stretch.{s}", f"hand.{s}")),
            Aim(
                f"hand_{s}",
                f"middle_01_{s}",
                (f"hand.{s}", f"middle1.{s}"),
                ((f"index_01_{s}", f"pinky_01_{s}"), (f"index1.{s}", f"pinky1.{s}")),
            ),
            Aim(f"thumb_01_{s}", f"thumb_02_{s}", (f"thumb1.{s}", f"c_thumb2.{s}")),
            Aim(f"thumb_02_{s}", f"thumb_03_{s}", (f"c_thumb2.{s}", f"c_thumb3.{s}")),
        ]

        for finger in FINGERS:
            out += [
                Aim(f"{finger}_01_{s}", f"{finger}_02_{s}", (f"{finger}1.{s}", f"c_{finger}2.{s}")),
                Aim(f"{finger}_02_{s}", f"{finger}_03_{s}", (f"c_{finger}2.{s}", f"c_{finger}3.{s}")),
            ]

    return out


def across(vector, axis):
    return vector - axis * vector.dot(axis)


def reference_pose(target, source):
    """The target's bones' world rotations once its limbs point like the source's at rest."""
    joints = {bone.name: source.matrix_world @ bone.head_local for bone in source.data.bones}

    def head(name):
        return target.matrix_world @ target.pose.bones[name].head

    for aim in aims():
        bpy.context.view_layer.update()
        bone = target.pose.bones[aim.bone]
        world = target.matrix_world @ bone.matrix
        pivot = world.translation
        line = joints[aim.source[1]] - joints[aim.source[0]]
        turn = (head(aim.toward) - pivot).rotation_difference(line)

        if aim.across:
            (a, b), (c, d) = aim.across
            axis = line.normalized()
            have = across(turn @ (head(b) - head(a)), axis)
            turn = have.rotation_difference(across(joints[d] - joints[c], axis)) @ turn

        about = Matrix.Translation(pivot) @ turn.to_matrix().to_4x4() @ Matrix.Translation(-pivot)
        bone.matrix = target.matrix_world.inverted() @ about @ world

    bpy.context.view_layer.update()
    return {bone.name: (target.matrix_world @ bone.matrix).to_quaternion() for bone in target.pose.bones}


def parents_first(armature):
    return sorted(armature.data.bones, key=lambda bone: len(bone.parent_recursive))


def key_rate(action, fps):
    """The rate the action was sampled at: its keys' spacing, as frames of the scene's fps."""
    curve = next(c for c in action.layers[0].strips[0].channelbags[0].fcurves if len(c.keyframe_points) > 2)
    first, second = (point.co[0] for point in curve.keyframe_points[:2])
    return round(fps / (second - first))


class Retarget:
    def __init__(self, source, target):
        self.source, self.target = source, target
        self.map = source_bones()
        self.order = parents_first(target)
        self.rest = {bone.name: bone.matrix_local.copy() for bone in target.data.bones}
        self.source_rest = {
            bone.name: (source.matrix_world @ bone.matrix_local).to_quaternion() for bone in source.data.bones
        }

        for bone in target.pose.bones:
            bone.rotation_mode = "QUATERNION"

        self.reference = reference_pose(target, source)

        for bone in target.pose.bones:
            bone.matrix_basis = Matrix.Identity(4)

        bpy.context.view_layer.update()
        hip = target.matrix_world @ target.data.bones["thigh_l"].head_local
        source_hip = source.matrix_world @ source.data.bones["thigh_twist.l"].head_local
        self.stride = hip.z / source_hip.z
        self.pelvis = target.matrix_world @ target.data.bones["pelvis"].head_local
        self.source_root = source.matrix_world @ source.data.bones["root.x"].head_local
        self.spine = {}

        for name, toward in zip(TARGET_SPINE, TARGET_SPINE[1:]):
            joint = target.matrix_world @ target.data.bones[name].head_local
            next_joint = target.matrix_world @ target.data.bones[toward].head_local
            spot = self.spine_spot(toward)
            self.spine[name] = ((next_joint - joint).normalized(), spot, next_joint - self.spine_point(spot, rest=True))

    def spine_spot(self, name):
        """Where on the source's spine the target joint's rest height falls: a segment and the
        share along it."""
        height = (self.target.matrix_world @ self.target.data.bones[name].head_local).z
        joints = [self.spine_ends(index, rest=True)[0].z for index in range(len(SOURCE_SPINE))]
        joints.append(self.spine_ends(len(SOURCE_SPINE) - 1, rest=True)[1].z)

        for index in range(len(SOURCE_SPINE)):
            if height <= joints[index + 1] or index == len(SOURCE_SPINE) - 1:
                return index, (height - joints[index]) / (joints[index + 1] - joints[index])

    def spine_ends(self, index, rest=False):
        """A segment of the source's spine: a bone's head and the next bone's, or the last's tail."""
        source = self.source
        bones = source.data.bones if rest else source.pose.bones
        head = "head_local" if rest else "head"
        tail = "tail_local" if rest else "tail"
        start = source.matrix_world @ getattr(bones[SOURCE_SPINE[index]], head)

        if index + 1 < len(SOURCE_SPINE):
            return start, source.matrix_world @ getattr(bones[SOURCE_SPINE[index + 1]], head)

        return start, source.matrix_world @ getattr(bones[SOURCE_SPINE[index]], tail)

    def spine_point(self, spot, rest=False):
        start, end = self.spine_ends(spot[0], rest)
        return start.lerp(end, spot[1])

    def pose(self):
        """The target's bones' matrix_basis for the source's current pose, and their heads'
        world positions."""
        source, target = self.source, self.target
        turns = {name: (source.matrix_world @ bone.matrix).to_quaternion() for name, bone in source.pose.bones.items()}
        world_turn = target.matrix_world.to_quaternion()
        to_target = target.matrix_world.inverted()
        posed, rotations, basis = {}, {}, {}

        for bone in self.order:
            name, parent = bone.name, bone.parent

            if parent:
                base = posed[parent.name] @ self.rest[parent.name].inverted() @ self.rest[name]
            else:
                base = self.rest[name]

            if name in self.map:
                origin = self.map[name]
                rotation = turns[origin] @ self.source_rest[origin].inverted() @ self.reference[name]
            elif parent:
                rotation = rotations[parent.name] @ self.reference[parent.name].inverted() @ self.reference[name]
            else:
                rotation = self.reference[name]

            head = base.translation

            if name == "pelvis":
                root = source.matrix_world @ source.pose.bones["root.x"].head
                head = to_target @ (self.pelvis + (root - self.source_root) * self.stride)

            if name in self.spine:
                line, spot, offset = self.spine[name]
                have = rotation @ self.reference[name].inverted() @ line
                wanted = self.spine_point(spot) + offset - target.matrix_world @ head
                rotation = have.rotation_difference(wanted.normalized()) @ rotation

            rotations[name] = rotation
            posed[name] = Matrix.Translation(head) @ (world_turn.inverted() @ rotation).to_matrix().to_4x4()
            basis[name] = base.inverted() @ posed[name]

        heads = {name: target.matrix_world @ matrix.translation for name, matrix in posed.items()}
        return basis, heads

    def bake(self, clip, source_action):
        """Keys the source action onto the target as an action named `clip`, at the rate the
        source was sampled at. Returns each bone head's lowest height over the clip."""
        scene = bpy.context.scene
        fps = scene.render.fps / scene.render.fps_base
        rate = key_rate(source_action, fps)
        animation = self.source.animation_data

        for track in animation.nla_tracks:
            track.mute = True

        animation.action = source_action
        start, end = source_action.frame_range
        count = round((end - start) / fps * rate)
        keys = {bone.name: [] for bone in self.order}
        lowest = {}

        for index in range(count + 1):
            frame = start + index * fps / rate
            scene.frame_set(int(frame), subframe=frame - int(frame))
            basis, heads = self.pose()

            for name, matrix in basis.items():
                rotation = matrix.to_quaternion()

                if keys[name] and keys[name][-1][0].dot(rotation) < 0:
                    rotation.negate()

                keys[name].append((rotation, matrix.translation))
                lowest[name] = min(lowest.get(name, heads[name].z), heads[name].z)

        write_action(self.target, clip, keys, rate)
        print(f"RETARGETED {clip} {count + 1} frames at {rate} fps")
        return rate, lowest


def write_action(target, clip, keys, rate):
    """The keys as a new action, stashed on the target's NLA where glTF's exporter finds it. Keys
    are frames at `rate`; the export sets the scene to it."""
    action = bpy.data.actions.new(clip)
    animation = target.animation_data or target.animation_data_create()
    animation.action = action

    for name, frames in keys.items():
        channels = [("rotation_quaternion", [rotation for rotation, _ in frames])]

        if name == "pelvis":
            channels.append(("location", [location for _, location in frames]))

        for channel, values in channels:
            for index in range(len(values[0])):
                curve = action.fcurve_ensure_for_datablock(
                    target, f'pose.bones["{name}"].{channel}', index=index, group_name=name
                )
                curve.keyframe_points.add(len(values))
                points = [c for frame, value in enumerate(values) for c in (frame, value[index])]
                curve.keyframe_points.foreach_set("co", points)
                curve.update()

    track = animation.nla_tracks.new()
    track.name = clip
    track.strips.new(clip, 0, action)
    animation.action = None


def retarget(source, target, clips):
    """Bakes the named clips of the source armature onto the target; returns their rate and each
    bone's lowest height over all of them."""
    sources = {}

    # The baked actions take the clips' names.
    for name in clips:
        sources[name] = bpy.data.actions[name]
        sources[name].name = f"source {name}"

    retarget = Retarget(source, target)
    rates, lowest = set(), {}

    for name in clips:
        rate, low = retarget.bake(name, sources[name])
        rates.add(rate)
        lowest = {bone: min(lowest.get(bone, z), z) for bone, z in low.items()}

    if len(rates) != 1:
        raise ValueError(f"clips sampled at different rates: {sorted(rates)}")

    return rates.pop(), lowest
