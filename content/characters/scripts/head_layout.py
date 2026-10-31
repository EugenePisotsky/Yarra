"""The head's own texture layout: its vertices placed on a scanned face's image.

MakeHuman's atlas gives the head one island that spends its room on the scalp and neck: a mouth
gets 66 texels of a 2K atlas. Here a head has its own texture, laid out as the scan is: a skin
file (content/characters/skins/) pairs points of the head's atlas island with where they lie on
the scan, a thin-plate spline carries every island point onto the scan, and the scan fills the
middle of the texture. What falls outside the scan (scalp, the back of the neck) is squeezed into
the margin around it, axis by axis, so the layout stays unfolded.

Both lid margins pair with the closed eyes' slit, and both edges of the lips' gap with the lips'
line: the sockets and the gap are unseen, and take no room. Runs in Blender (export_glb.py) and in
plain Python (skin_bake.py), so it needs only numpy.
"""
import numpy as np

# The texture's size in texels, and the share of its width the scan's width takes.
TEXTURE = 4096
SCAN_SHARE = 0.86

# How far past the scan a point reaches its margin's middle, in the scan's texels.
SQUEEZE = 1800.0

# The spline's give, so that nearby pairs that disagree a little do not make it ripple.
SMOOTHING = 2e-4

# UV units to spline units, and how many points one evaluation takes at a time.
SPLINE_UNITS = 4.096

BATCH = 20000


def _kernel(squared):
    with np.errstate(divide="ignore", invalid="ignore"):
        return np.nan_to_num(squared * np.log(squared))


class Layout:
    def __init__(self, skin):
        pairs = np.array(skin["pairs"], dtype=np.float64)
        self.points = pairs[:, :2] * SPLINE_UNITS
        self.size = np.array(skin["scan_size"], dtype=np.float64)
        count = len(self.points)
        distances = ((self.points[:, None] - self.points[None]) ** 2).sum(-1)
        affine = np.c_[np.ones(count), self.points]
        system = np.block([
            [_kernel(distances) + SMOOTHING * np.eye(count), affine],
            [affine.T, np.zeros((3, 3))],
        ])
        self.weights = np.linalg.solve(system, np.r_[pairs[:, 2:], np.zeros((3, 2))])
        self.scale = TEXTURE * SCAN_SHARE / self.size[0]
        self.origin = (TEXTURE - self.size * self.scale) / 2

    def scan(self, uv):
        """Atlas island UVs to scan pixels."""
        points = np.asarray(uv, dtype=np.float64) * SPLINE_UNITS
        out = np.empty_like(points)

        for start in range(0, len(points), BATCH):
            batch = points[start : start + BATCH]
            squared = ((batch[:, None] - self.points[None]) ** 2).sum(-1)
            out[start : start + BATCH] = (
                _kernel(squared) @ self.weights[: len(self.points)]
                + np.c_[np.ones(len(batch)), batch] @ self.weights[len(self.points) :]
            )

        return out

    def texels(self, scan):
        """Scan pixels to texels of the head's texture (y down, as images run)."""
        out = np.empty_like(scan)

        for axis in range(2):
            size, margin = self.size[axis], self.origin[axis] / self.scale
            value = scan[:, axis]
            over = np.maximum(value - size, 0.0)
            under = np.maximum(-value, 0.0)
            squeezed = np.clip(value, 0.0, size) + margin * (
                (1.0 - np.exp(-over / SQUEEZE)) - (1.0 - np.exp(-under / SQUEEZE))
            )
            out[:, axis] = self.origin[axis] + squeezed * self.scale

        return out

    def texture_uv(self, uv):
        """Atlas island UVs to the head texture's UVs."""
        texels = self.texels(self.scan(uv)) / TEXTURE
        return np.c_[texels[:, 0], 1.0 - texels[:, 1]]


def uv_islands(uv, starts, totals):
    """Each loop's UV island, as a number. Loops of one vertex split along a seam share an island
    only through polygons, so islands join by polygon over equal UVs."""
    keys = np.round(uv * 20000).astype(np.int64)
    _, ids = np.unique(keys[:, 0] * 100000 + keys[:, 1], return_inverse=True)
    parent = np.arange(ids.max() + 1)

    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]

        return a

    for start, total in zip(starts, totals):
        root = find(ids[start])

        for index in range(1, total):
            parent[find(ids[start + index])] = root

    return np.array([find(i) for i in range(len(parent))])[ids]


def island_at(uv, islands, point):
    """Which loops belong to the UV island under `point`."""
    nearest = np.argmin(((uv - np.asarray(point)) ** 2).sum(-1))
    return islands == islands[nearest]
