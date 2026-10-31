"""Parts of an exported character that MakeHuman's own do not make good enough. The brows are not
parts at all: the skin paints them (skin_layers_bake.py).

The lashes: MakeHuman's are thick cut-out cards. Strands take their place, each a tapered ribbon
rooted on the lid's edge, fanned, curled, gathered in loose clumps, over a soft dark strip along the
lid that reads as the line of their roots: strands alone thin to pale fuzz at any distance. All are
bound to the triangle of skin at their root so that lid sliders carry them, and draw with soft edges,
which a hair this fine needs (eye_bake.py).

The eyes: MakeHuman's are one cut-out texture on one surface. Three surfaces of revolution take
their place, fitted by MakeHuman's own eye vertices so they follow the same sliders: the sclera and
a recessed iris, opaque, and over them the wet layer with the cornea's bulge, drawn with blending
(eye_bake.py). Each eye's sclera map is turned so that its nasal side faces the nose.
"""
import json
from collections import namedtuple

import bpy
import numpy as np

from eye_shape import IRIS_SHARE
from head_layout import island_at, uv_islands
from kit import coordinates, fitting, in_gltf, part_name
from mpfb_services import ROOT

Lashes = namedtuple("Lashes", "count lengths peak fan curl")

# The lashes of each lid: how many, their lengths at the inner corner, the longest and the outer
# corner, metres; how far along the lid the longest stand; how far they lean out of the lid's plane
# and how far they curl; the ribbon's width at the root and the tip, metres.
UPPER_LASHES = Lashes(count=85, lengths=(0.0022, 0.0100, 0.0085), peak=0.7, fan=0.16, curl=0.5)
LOWER_LASHES = Lashes(count=45, lengths=(0.0016, 0.0060, 0.0050), peak=0.65, fan=0.6, curl=0.12)
LASH_WIDTH = (0.00072, 0.00028)

# The strip along each lid: its sections, half width at its middle and ends, metres, and how far off
# the lid's rim it stands.
LASH_LINE_SECTIONS = 22
UPPER_LINE, LOWER_LINE = (0.00075, 0.00028), (0.00030, 0.00014)
LASH_LINE_LIFT = 0.00012

# How far forward of the socket's edge a root stands, on the lid's rim, metres.
LASH_ROOT_FORWARD = 0.0011

# The ribbon's sections, how many clumps a lid's lashes gather in and how far towards its clump a
# strand's tip leans (a share of the distance), how much a length varies, and the seed.
LASH_SECTIONS = 4
LASH_CLUMPS, LASH_GATHER = 15, 0.55
LASH_JITTER = 0.16
LASH_SEED = 7

# Round the eye, and from the front: the sclera's reach, the wet layer's, the iris's recess behind
# the limbus and its slope to the pupil, how far the wet layer stands off the sclera and how far the
# cornea rises over it (shares of the eye's radius, its rings' angles in degrees).
EYE_SEGMENTS = 48
SCLERA_REACH = 120.0
WET_REACH = 80.0
IRIS_RECESS, IRIS_SLOPE = 0.012, 0.025
WET_LIFT, CORNEA_RISE = 0.012, 0.05

# The iris reaches this far under the sclera, and how much of its texture's width its circle fills.
IRIS_TUCK = 1.05
IRIS_MAP = 0.49

SCLERA, IRIS, WET, LASH = "eye_sclera", "eye_iris", "eye_wet", "eye_lash"

# The face looks along +Z in glTF's axes.
FORWARD = np.array([0.0, 0.0, 1.0])


def dominant_group(part):
    """The vertex group holding most of the part's weight."""
    names = {group.index: group.name for group in part.vertex_groups}
    totals = {}

    for vertex in part.data.vertices:
        for element in vertex.groups:
            totals[element.group] = totals.get(element.group, 0.0) + element.weight

    return names[max(totals, key=totals.get)]


def to_blender(points):
    """glTF's Y-up axes to Blender's Z-up (kit.to_gltf undone)."""
    return np.c_[points[:, 0], -points[:, 2], points[:, 1]]


def to_local(obj, gltf_points):
    matrix = np.array(obj.matrix_world)
    return (to_blender(gltf_points) - matrix[:3, 3]) @ np.linalg.inv(matrix[:3, :3]).T


def sizes(base, fit):
    return np.array([abs(base[a, axis] - base[b, axis]) / length for axis, (a, b, length) in enumerate(fit["scales"])])


def triangles(mesh):
    out = []

    for polygon in mesh.polygons:
        vertices = list(polygon.vertices)
        out.extend((vertices[0], vertices[i], vertices[i + 1]) for i in range(1, len(vertices) - 1))

    return np.array(out)


def nearest_on_triangles(point, a, b, c):
    """Each triangle's closest point to `point`, as barycentric weights of the best one."""
    ab, ac = b - a, c - a
    normal = np.cross(ab, ac)
    area = np.maximum((normal * normal).sum(1), 1e-18)
    to_point = point - a
    v = (np.cross(to_point, ac) * normal).sum(1) / area
    w = (np.cross(ab, to_point) * normal).sum(1) / area
    v, w = np.clip(v, 0, 1), np.clip(w, 0, 1)
    over = v + w > 1
    v[over], w[over] = v[over] / (v + w)[over], w[over] / (v + w)[over]
    closest = a + ab * v[:, None] + ac * w[:, None]
    best = np.argmin(((closest - point) ** 2).sum(1))
    return best, np.array([1 - v[best] - w[best], v[best], w[best]]), closest[best]


def sphere(points):
    """The least-squares sphere through the points: its centre and radius."""
    system = np.c_[2 * points, np.ones(len(points))]
    solution = np.linalg.lstsq(system, (points * points).sum(1), rcond=None)[0]
    centre = solution[:3]
    return centre, float(np.sqrt(solution[3] + centre @ centre))


def lathe(rings, uv_of, pole=None, facing=None):
    """A surface of revolution about +Z: `rings` of (radial, z), `uv_of(ring, angle)` per vertex, and
    a vertex closing the last ring at `pole`. Returns points, faces and per-vertex UVs."""
    angles = np.linspace(0.0, 2 * np.pi, EYE_SEGMENTS, endpoint=False)
    points, uvs = [], []

    for index, (radial, height) in enumerate(rings):
        for angle in angles:
            points.append((radial * np.cos(angle), radial * np.sin(angle), height))
            uvs.append(uv_of(index, angle))

    faces = []

    for ring in range(len(rings) - 1):
        for j in range(EYE_SEGMENTS):
            k = (j + 1) % EYE_SEGMENTS
            first, second = ring * EYE_SEGMENTS, (ring + 1) * EYE_SEGMENTS
            faces.append((first + j, second + j, second + k, first + k))

    if pole is not None:
        centre = len(points)
        last = (len(rings) - 1) * EYE_SEGMENTS
        points.append((0.0, 0.0, pole))
        uvs.append(uv_of(None, 0.0))
        faces.extend((last + j, centre, last + (j + 1) % EYE_SEGMENTS) for j in range(EYE_SEGMENTS))

    points = np.array(points)
    a, b, c = (points[list(faces[0][i] for i in (0, 1, 2))])
    normal = np.cross(b - a, c - a)

    if normal @ facing(a) < 0:
        faces = [tuple(reversed(face)) for face in faces]

    return points, faces, np.array(uvs)


def eye_surfaces(table, nasal):
    """The three surfaces at unit radius, the cornea towards +Z, `nasal` (+1 or -1) the direction of
    the nose along X: per surface its name, points, faces and per-vertex UVs."""
    limbus = np.arcsin(IRIS_SHARE)

    def turned(angle):
        return angle if nasal > 0 else np.pi - angle

    sclera_angles = np.radians(np.concatenate([
        np.linspace(np.degrees(limbus), 40.0, 5),
        np.linspace(50.0, SCLERA_REACH, 8),
    ]))

    def sclera_uv(ring, angle):
        radius = np.interp(np.degrees(sclera_angles[ring]), table["angles"], table["uv_radius"])
        return (0.5 + radius * np.cos(turned(angle)), 0.5 + radius * np.sin(angle))

    sclera = lathe([(np.sin(a), np.cos(a)) for a in sclera_angles], sclera_uv, facing=lambda p: p)
    rim = np.cos(limbus) - IRIS_RECESS
    shares = np.linspace(1.0, 0.04, 7)

    def iris_uv(ring, angle):
        share = 0.0 if ring is None else shares[ring]
        return (0.5 + IRIS_MAP * share * np.cos(angle), 0.5 + IRIS_MAP * share * np.sin(angle))

    iris = lathe(
        [(IRIS_SHARE * IRIS_TUCK * t, rim - IRIS_SLOPE * (1 - t)) for t in shares],
        iris_uv,
        pole=rim - IRIS_SLOPE,
        facing=lambda p: np.array([0.0, 0.0, 1.0]),
    )
    wet_angles = np.radians(np.concatenate([
        np.linspace(WET_REACH, np.degrees(limbus) + 6, 8),
        np.linspace(np.degrees(limbus) + 2, 2.0, 8),
    ]))

    def lifted(angle):
        over = np.clip(1 - angle / (limbus * 1.25), 0, 1)
        return 1 + WET_LIFT + CORNEA_RISE * over * over * (3 - 2 * over)

    def wet_uv(ring, angle):
        if ring is None:
            return (0.5, 0.5)

        share = lifted(wet_angles[ring]) * np.sin(wet_angles[ring]) / 2
        return (0.5 + share * np.cos(angle), 0.5 + share * np.sin(angle))

    wet = lathe(
        [(lifted(a) * np.sin(a), lifted(a) * np.cos(a)) for a in wet_angles],
        wet_uv,
        pole=lifted(0.0),
        facing=lambda p: p,
    )
    return [(SCLERA, *sclera), (IRIS, *iris), (WET, *wet)]


def stand_in_material(name):
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    nodes = material.node_tree.nodes
    texture = nodes.new("ShaderNodeTexImage")
    texture.image = bpy.data.images.new(f"{name}_stand_in", 4, 4)
    material.node_tree.links.new(texture.outputs["Color"], nodes["Principled BSDF"].inputs["Base Color"])
    return material


def replace_eyes(part, base, model_path):
    """Puts the model's eye where each of MakeHuman's stands, keeps MakeHuman's skinning, and returns
    the fitting: each new vertex follows the nearest of MakeHuman's eye vertices."""
    fit = fitting(part)
    size = sizes(base, fit)
    old = in_gltf(part, coordinates(part.data.vertices))
    groups = {group.index: group.name for group in part.vertex_groups}
    weights_by_side = {}

    for side, chosen in (("right", old[:, 0] < 0), ("left", old[:, 0] >= 0)):
        totals = {}

        for vertex in np.flatnonzero(chosen):
            for element in part.data.vertices[int(vertex)].groups:
                totals[element.group] = totals.get(element.group, 0.0) + element.weight

        weights_by_side[side] = groups[max(totals, key=totals.get)]

    table = np.load(model_path)
    positions, faces, uvs, materials, sides = [], [], [], [], []
    names = (SCLERA, IRIS, WET)

    for side, chosen in (("right", old[:, 0] < 0), ("left", old[:, 0] >= 0)):
        centre, radius = sphere(old[chosen])
        print(f"FIT eye {side}: centre {np.round(centre, 4)}, radius {radius * 1000:.1f} mm")

        for slot, (_, points, polygons, uv) in enumerate(eye_surfaces(table, -np.sign(centre[0]))):
            first = len(positions)
            positions.extend(centre + points * radius)
            sides.extend([side] * len(points))

            for polygon in polygons:
                faces.append([first + int(i) for i in polygon])
                uvs.append(uv[list(polygon)])
                materials.append(slot)

    positions = np.array(positions)
    mesh = bpy.data.meshes.new("eyes")
    mesh.from_pydata(to_local(part, positions).tolist(), [], faces)
    mesh.update()
    layer = mesh.uv_layers.new(name="UVMap")
    layer.data.foreach_set("uv", np.concatenate(uvs).ravel())
    mesh.polygons.foreach_set("material_index", materials)
    mesh.polygons.foreach_set("use_smooth", [True] * len(faces))

    for name in names:
        mesh.materials.append(stand_in_material(name))

    # The groups' names live with the mesh: the new one starts without them.
    part.data = mesh

    for name in set(weights_by_side.values()):
        group = part.vertex_groups.new(name=name)
        members = [i for i, side in enumerate(sides) if weights_by_side[side] == name]
        group.add(members, 1.0, "REPLACE")

    nearest = np.array([np.argmin(((old - point) ** 2).sum(1)) for point in positions])
    triples = np.array(fit["vertices"])[nearest]
    weights = np.array(fit["weights"])[nearest]
    anchored = (weights[:, :, None] * base[triples]).sum(1)
    fit.update(
        vertices=triples.tolist(),
        weights=weights.round(7).tolist(),
        offsets=np.round((positions - anchored) / size, 7).tolist(),
    )
    print(f"FIT eyes: {len(positions)} vertices over {len(names)} surfaces, both eyes")
    return fit


def lid_chains(basemesh, base, skin_path, centres):
    """Each eye's lids, each a chain of points inner corner to outer corner along its edge: where the
    head's skin joins the socket's inside. Returns [(upper, lower), ...] in the order of `centres`."""
    skin = json.loads((ROOT / skin_path).read_text())
    mesh = basemesh.data
    uv = np.empty(len(mesh.loops) * 2)
    mesh.uv_layers.active.data.foreach_get("uv", uv)
    uv = uv.reshape(-1, 2)
    starts = np.empty(len(mesh.polygons), dtype=np.int32)
    totals = np.empty(len(mesh.polygons), dtype=np.int32)
    loops = np.empty(len(mesh.loops), dtype=np.int32)
    mesh.polygons.foreach_get("loop_start", starts)
    mesh.polygons.foreach_get("loop_total", totals)
    mesh.loops.foreach_get("vertex_index", loops)
    islands = uv_islands(uv, starts, totals)
    head = loops[island_at(uv, islands, skin["head_uv_point"])]
    edge = []

    for point in skin["cavity_uv_points"]:
        edge.append(np.intersect1d(head, loops[island_at(uv, islands, point)]))

    edge = base[np.concatenate(edge)]
    chains = []

    for centre, radius in centres:
        points = edge[np.linalg.norm(edge - centre, axis=1) < radius * 1.6]
        across = np.abs(points[:, 0])
        inner, outer = points[np.argmin(across)], points[np.argmax(across)]
        share = (across - across.min()) / (across.max() - across.min())
        chord = inner[1] + (outer[1] - inner[1]) * share
        upper = points[(points[:, 1] >= chord) | (share < 0.001) | (share > 0.999)]
        lower = points[(points[:, 1] <= chord) | (share < 0.001) | (share > 0.999)]
        by_across = [lid[np.argsort(np.abs(lid[:, 0]))] for lid in (upper, lower)]
        chains.append(tuple(by_across))

    return chains


def along(chain, share):
    """Points at these shares of the chain's length, and its direction there."""
    arc = np.r_[0.0, np.cumsum(np.linalg.norm(np.diff(chain, axis=0), axis=1))]
    at = share * arc[-1]
    points = np.stack([np.interp(at, arc, chain[:, axis]) for axis in range(3)], -1)
    ahead = np.stack([np.interp(at + 0.0008, arc, chain[:, axis]) for axis in range(3)], -1)
    behind = np.stack([np.interp(at - 0.0008, arc, chain[:, axis]) for axis in range(3)], -1)
    return points, ahead - behind, arc[-1]


def strands(chain, centre, spec, rng):
    """A lid's lashes: their points, the roots they bind by, faces and UVs. The root is at V = 0."""
    shares = (np.arange(spec.count) + rng.uniform(0.15, 0.85, spec.count)) / spec.count
    roots, tangents, length = along(chain, shares)
    tangents /= np.maximum(np.linalg.norm(tangents, axis=1, keepdims=True), 1e-9)
    low, high, end = spec.lengths
    rise = np.minimum(shares / spec.peak, 1.0)
    fall = np.maximum((shares - spec.peak) / (1 - spec.peak), 0.0)
    lengths = (low + (high - low) * rise**0.7 + (end - high) * fall) * rng.uniform(1 - LASH_JITTER, 1 + LASH_JITTER, spec.count)
    gather = (np.round(shares * LASH_CLUMPS) / LASH_CLUMPS - shares) * LASH_GATHER * length
    points, anchors, faces, uvs = [], [], [], []

    for root, tangent, reach, lean in zip(roots, tangents, lengths, gather):
        root = root + FORWARD * LASH_ROOT_FORWARD
        fan = root - centre
        fan[2] = 0.0
        fan /= np.linalg.norm(fan)
        out = FORWARD + fan * spec.fan
        out /= np.linalg.norm(out)
        width = tangent + fan * 0.7
        width /= np.linalg.norm(width)
        first = len(points)

        for section in range(LASH_SECTIONS):
            t = section / (LASH_SECTIONS - 1)
            centre_line = root + reach * (t * out + t * t * spec.curl * fan) + tangent * lean * t * t
            half = (LASH_WIDTH[0] + (LASH_WIDTH[1] - LASH_WIDTH[0]) * t) / 2
            points.extend((centre_line - width * half, centre_line + width * half))
            anchors.extend((root, root))
            uvs.extend(((0.0, t), (1.0, t)))

        faces.extend(
            (first + 2 * s, first + 2 * s + 1, first + 2 * s + 3, first + 2 * s + 2)
            for s in range(LASH_SECTIONS - 1)
        )

    return points, anchors, faces, uvs


def lash_line(chain, centre, widths):
    """The soft strip along a lid, in a strand's terms: all of it at the root's end of the texture."""
    shares = np.linspace(0.015, 0.985, LASH_LINE_SECTIONS)
    roots, _, _ = along(chain, shares)
    points, anchors, uvs = [], [], []

    for root, share in zip(roots, shares):
        root = root + FORWARD * LASH_ROOT_FORWARD
        fan = root - centre
        fan[2] = 0.0
        fan /= np.linalg.norm(fan)
        across = FORWARD * 0.6 + fan
        across /= np.linalg.norm(across)
        half = widths[1] + (widths[0] - widths[1]) * np.sin(np.pi * share)
        lifted = root + (FORWARD + fan) * LASH_LINE_LIFT
        points.extend((lifted - across * half, lifted + across * half))
        anchors.extend((root, root))
        uvs.extend(((0.0, 0.0), (1.0, 0.0)))

    faces = [(2 * s, 2 * s + 1, 2 * s + 3, 2 * s + 2) for s in range(LASH_LINE_SECTIONS - 1)]
    return points, anchors, faces, uvs


def grow_lashes(basemesh, part, base, chains, centres):
    """Strands and lash lines in place of MakeHuman's card; returns the fitting, each vertex on the
    skin triangle nearest its root."""
    fit = fitting(part)
    size = sizes(base, fit)
    group = dominant_group(part)
    rng = np.random.default_rng(LASH_SEED)
    points, anchors, faces, uvs = [], [], [], []

    def gather(more):
        first = len(points)
        points.extend(more[0])
        anchors.extend(more[1])
        faces.extend(tuple(first + i for i in face) for face in more[2])
        uvs.extend(more[3])

    for (upper, lower), (centre, _) in zip(chains, centres):
        gather(strands(upper, centre, UPPER_LASHES, rng))
        gather(strands(lower, centre, LOWER_LASHES, rng))
        gather(lash_line(upper, centre, UPPER_LINE))
        gather(lash_line(lower, centre, LOWER_LINE))

    positions = np.array(points)
    mesh = bpy.data.meshes.new("lashes")
    mesh.from_pydata(to_local(part, positions).tolist(), [], faces)
    mesh.update()
    loops = np.array([vertex for face in faces for vertex in face])
    mesh.uv_layers.new(name="UVMap").data.foreach_set("uv", np.array(uvs)[loops].ravel())
    mesh.polygons.foreach_set("use_smooth", [True] * len(faces))
    mesh.materials.append(stand_in_material(LASH))
    part.data = mesh
    part.vertex_groups.clear()
    part.vertex_groups.new(name=group).add(list(range(len(positions))), 1.0, "REPLACE")
    body = triangles(basemesh.data)
    near = np.linalg.norm(base[body].mean(1)[:, None] - np.array([c for c, _ in centres])[None], axis=2).min(1) < 0.04
    body = body[near]
    a, b, c = base[body[:, 0]], base[body[:, 1]], base[body[:, 2]]
    roots, of_root = np.unique(np.array(anchors), axis=0, return_inverse=True)
    bound = [nearest_on_triangles(root, a, b, c)[:2] for root in roots]
    triples = np.array([body[best] for best, _ in bound])[of_root]
    shares = np.array([barycentric for _, barycentric in bound])[of_root]
    anchored = (shares[:, :, None] * base[triples]).sum(1)
    fit.update(
        vertices=triples.tolist(),
        weights=shares.round(7).tolist(),
        offsets=np.round((positions - anchored) / size, 7).tolist(),
    )
    print(f"FIT lashes: {len(positions)} vertices, {len(faces)} quads, both eyes")
    return fit


def apply(basemesh, parts, base, eye_model, skin_path):
    """The fittings of the parts placed here, by part name."""
    fits = {}
    named = {part_name(part): part for part in parts if part != basemesh}

    if eye_model:
        name = next(name for name in named if name.startswith("eyes/"))
        eyes = in_gltf(named[name], coordinates(named[name].data.vertices))
        centres = [sphere(eyes[side]) for side in (eyes[:, 0] < 0, eyes[:, 0] >= 0)]
        fits[name] = replace_eyes(named[name], base, eye_model)
        chains = lid_chains(basemesh, base, skin_path, centres)
        name = next(name for name in named if name.startswith("eyelashes/"))
        fits[name] = grow_lashes(basemesh, named[name], base, chains, centres)

    return fits
