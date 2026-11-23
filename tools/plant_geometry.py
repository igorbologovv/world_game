"""Indexed plant geometry and a single-primitive glTF 2.0 binary writer."""

import json
import math
import struct
from dataclasses import dataclass
from pathlib import Path


ORIGIN = (0.0, 0.0, 0.0)
UP = (0.0, 1.0, 0.0)
RIGHT = (1.0, 0.0, 0.0)
MAX_TRIANGLES = 2400
COMPACT_TRIANGLES = 128
Vertex = struct.Struct("<6f4B")


def add(a, b):
    return tuple(x + y for x, y in zip(a, b))


def sub(a, b):
    return tuple(x - y for x, y in zip(a, b))


def scale(v, factor):
    return tuple(x * factor for x in v)


def mix(a, b, t):
    return add(scale(a, 1 - t), scale(b, t))


def cross(a, b):
    x, y, z = a
    u, v, w = b
    return (y * w - z * v, z * u - x * w, x * v - y * u)


def unit(v):
    return scale(v, 1 / math.hypot(*v))


def axis(angle):
    return (math.cos(angle), 0.0, math.sin(angle))


def frame(forward, hint=UP):
    side = cross(forward, hint)
    if math.hypot(*side) < 0.001:
        side = cross(forward, RIGHT)
    side = unit(side)
    return side, cross(side, forward)


def linear_rgb(srgb):
    def linear(channel):
        return channel / 12.92 if channel <= 0.04045 else ((channel + 0.055) / 1.055) ** 2.4

    return tuple(linear(channel / 255) for channel in bytes.fromhex(srgb))


@dataclass(frozen=True)
class PathCurve:
    start: tuple
    end: tuple
    bend: tuple = ORIGIN

    def at(self, t):
        return add(mix(self.start, self.end, t), scale(self.bend, 4 * t * (1 - t)))

    def tangent(self, t):
        return unit(add(sub(self.end, self.start), scale(self.bend, 4 - 8 * t)))


# Stations run from attachment point to tip. Each pair gives distance and half-width.
PROFILES = {
    "needle": ((0, 0), (0.24, 1), (0.66, 0.48), (1, 0)),
    "ribbon": ((0, 0), (0.23, 0.85), (0.56, 1), (0.81, 0.50), (1, 0)),
    "strap": ((0, 0), (0.18, 0.76), (0.56, 1), (0.85, 0.76), (1, 0)),
    "lance": ((0, 0), (0.31, 1), (0.65, 0.68), (1, 0)),
    "oval": ((0, 0), (0.23, 0.72), (0.51, 1), (0.82, 0.74), (1, 0)),
    "heart": ((0, 0), (0.12, 0.80), (0.36, 1), (0.71, 0.55), (1, 0)),
    "lobed": ((0, 0), (0.26, 1), (0.43, 0.58), (0.60, 0.78), (0.76, 0.37), (1, 0)),
    "fan": ((0, 0), (0.37, 0.49), (0.70, 0.94), (0.87, 1), (1, 0.68)),
    "spoon": ((0, 0), (0.27, 0.34), (0.66, 1), (0.89, 0.75), (1, 0)),
    "succulent": ((0, 0), (0.18, 0.71), (0.57, 1), (0.85, 0.63), (1, 0)),
    "petal": ((0, 0), (0.22, 0.62), (0.70, 1), (0.92, 0.66), (1, 0)),
}


class Mesh:
    def __init__(self):
        self.positions = []
        self.colors = []
        self.indices = []
        self.parts = []

    @property
    def triangle_count(self):
        return len(self.indices) // 3

    def append(self, positions, faces, colors):
        offset = len(self.positions)
        first = len(self.indices)
        self.positions.extend(positions)
        self.colors.extend(colors)
        self.indices.extend(offset + index for face in faces for index in face)
        self.parts.append((offset, len(self.positions), first, len(self.indices), False))

    def merge(self, mesh):
        offset = len(self.positions)
        first = len(self.indices)
        self.positions.extend(mesh.positions)
        self.colors.extend(mesh.colors)
        self.indices.extend(offset + index for index in mesh.indices)
        self.parts.extend((a + offset, b + offset, c + first, d + first, True)
                          for a, b, c, d, _ in mesh.parts)

    def compact(self):
        """Preserve broad silhouettes and blooms while thinning repeated parts."""
        ground = min(point[1] for point in self.positions)
        height = max(point[1] for point in self.positions) - ground
        sectors = [[] for _ in range(8)]
        roots = []
        for part in self.parts:
            a, b, _, _, bloom = part
            bounds = [(min(values), max(values)) for values in zip(*self.positions[a:b])]
            x, y, z = (hi - lo for lo, hi in bounds)
            cx, _, cz = ((lo + hi) * 0.5 for lo, hi in bounds)
            sector = int(4 * math.atan2(cz, cx) / math.tau + 4) % 4 + 4 * bloom
            score = x * y + z * y + x * z * 0.55 + y * y * 0.08
            rooted = bounds[1][0] < ground + height * 0.04
            sectors[sector].append((score * (1.5 if rooted else 1.0), part))
            if rooted:
                roots.append((score, part))
        for sector in sectors:
            sector.sort()

        compact = Mesh()

        def keep(part):
            a, b, c, d, _ = part
            if compact.triangle_count + (d - c) // 3 <= COMPACT_TRIANGLES:
                indices = iter(index - a for index in self.indices[c:d])
                compact.append(self.positions[a:b], zip(indices, indices, indices), self.colors[a:b])

        _, root = max(roots)
        keep(root)
        while any(sectors):
            for sector in sectors:
                if sector:
                    _, part = sector.pop()
                    if part != root:
                        keep(part)
        return compact

    def tube(self, path, rings, sides, color):
        """Build a closed tapered stem, stalk or bud along a curve."""
        hint = axis(0)
        if abs(path.tangent(0.5)[0]) > 0.7:
            hint = (0.0, 0.0, 1.0)
        ring_points = [
            (math.cos(math.tau * j / sides), math.sin(math.tau * j / sides))
            for j in range(sides)
        ]
        positions = []
        colors = []
        for t, radius in rings:
            center = path.at(t)
            forward = path.tangent(t)
            side, _ = frame(forward, hint)
            around = cross(forward, side)
            for x, y in ring_points:
                positions.append(add(center, scale(add(scale(side, x), scale(around, y)), radius)))
                colors.append(scale(color, 0.78 + 0.20 * t + 0.09 * x))

        faces = []
        for row in range(len(rings) - 1):
            lower = row * sides
            for j in range(sides):
                a, b = lower + j, lower + (j + 1) % sides
                faces.extend(((a, b, a + sides), (b, b + sides, a + sides)))
        top = (len(rings) - 1) * sides
        faces.extend((0, j + 1, j) for j in range(1, sides - 1))
        faces.extend((top, top + j, top + j + 1) for j in range(1, sides - 1))
        self.append(positions, faces, colors)

    def bud(self, root, direction, length, width, color):
        """Build a closed floret or seed using an eight-triangle spindle."""
        forward = unit(direction)
        side, normal = frame(forward)
        center = add(root, scale(forward, length * 0.41))
        positions = [root, add(root, scale(forward, length))]
        points = (side, scale(normal, -1), scale(side, -1), normal)
        positions.extend(add(center, scale(point, width)) for point in points)
        faces = []
        for i in range(4):
            a, b = i + 2, (i + 1) % 4 + 2
            faces.extend(((0, b, a), (a, b, 1)))
        tones = (0.68, 1.10, 0.92, 1.06, 0.82, 0.95)
        self.append(positions, faces, [scale(color, tone) for tone in tones])

    def blade(self, root, direction, size, shape, arch, fold, color, hint=UP, compact=False):
        """Build a curved leaf, frond segment or petal with a shaded midrib."""
        length, width = size
        forward = unit(direction)
        side, normal = frame(forward, hint)
        rows = []
        positions = []
        colors = []
        faces = []

        profile = PROFILES[shape]
        if compact and shape == "succulent":
            profile = (profile[0], profile[2], profile[-1])
        elif compact and len(profile) > 4:
            profile = (profile[0], profile[1], profile[-2], profile[-1])
        for t, breadth in profile:
            center = add(root, scale(forward, length * t))
            lift = length * arch * t * (1 - t)
            center = add(center, scale(normal, lift))
            if breadth == 0:
                rows.append((len(positions),) * 3)
                positions.append(center)
                colors.append(scale(color, 0.73 + 0.29 * t))
                continue
            half_width = width * breadth
            left = len(positions)
            rows.append((left, left + 1, left + 2))
            positions.extend((
                sub(center, scale(side, half_width)),
                add(center, scale(normal, fold * half_width * 0.52)),
                add(center, scale(side, half_width)),
            ))
            colors.extend(scale(color, tone + 0.18 * t) for tone in (0.79, 1.03, 0.87))

        for a, b in zip(rows, rows[1:]):
            for i in range(2):
                if a[i] != a[i + 1]:
                    faces.append((a[i], a[i + 1], b[i]))
                if b[i] != b[i + 1]:
                    faces.append((a[i + 1], b[i + 1], b[i]))

        if shape == "succulent":
            underside = {index: index for index in range(len(positions))}
            underside_faces = []
            for left, middle, right in rows:
                if left != right:
                    center = mix(positions[left], positions[right], 0.5)
                    keel = scale(normal, -math.dist(positions[left], center) * 0.46)
                    underside[middle] = len(positions)
                    positions.append(add(center, keel))
                    colors.append(scale(colors[middle], 0.75))
            for a, b, c in faces:
                underside_faces.append((underside[a], underside[c], underside[b]))
            faces.extend(underside_faces)
        self.append(positions, faces, colors)

    def normals(self):
        normals = [[0.0, 0.0, 0.0] for _ in self.positions]
        for a, b, c in zip(*[iter(self.indices)] * 3):
            normal = cross(sub(self.positions[b], self.positions[a]), sub(self.positions[c], self.positions[a]))
            if math.hypot(*normal) == 0:
                raise ValueError("degenerate plant triangle")
            for index in (a, b, c):
                for axis_index, value in enumerate(normal):
                    normals[index][axis_index] += value
        return (unit(normal) for normal in normals)


def float32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def export_glb(path: Path, mesh: Mesh, name: str):
    """Write a standalone, Y-up GLB with its pivot at ground level."""
    return mesh.triangle_count, write_glb(path, [(name, mesh, ground_level(mesh))])


def ground_level(mesh):
    return min(point[1] for point in mesh.positions)


def write_glb(path: Path, models):
    """Pack named meshes with a common opaque, double-sided material."""
    document = {
        "asset": {"version": "2.0", "generator": "world-vegetation/build_plant_assets.py"},
        "scene": 0,
        "scenes": [{"nodes": []}],
        "nodes": [],
        "meshes": [],
        "materials": [{
            "name": "Plant",
            "doubleSided": True,
            "pbrMetallicRoughness": {"metallicFactor": 0, "roughnessFactor": 0.88},
        }],
        "buffers": [],
        "bufferViews": [],
        "accessors": [],
    }
    binary = bytearray()
    for name, mesh, ground in models:
        node = len(document["nodes"])
        document["scenes"][0]["nodes"].append(node)
        document["nodes"].append({"name": name, "mesh": node})
        document["meshes"].append({"name": name, "primitives": [append_geometry(document, binary, name, mesh, ground)]})
    document["buffers"].append({"byteLength": len(binary)})
    metadata = json.dumps(document, separators=(",", ":"), allow_nan=False).encode("utf-8")
    metadata += b" " * (-len(metadata) % 4)
    header = struct.pack("<4sIII4s", b"glTF", 2, 28 + len(metadata) + len(binary), len(metadata), b"JSON")
    data = header + metadata + struct.pack("<I4s", len(binary), b"BIN\0") + binary
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.is_file() or path.read_bytes() != data:
        path.write_bytes(data)
    return len(data)


def append_geometry(document, binary, name, mesh, ground):
    triangle_count = mesh.triangle_count
    vertex_count = len(mesh.positions)
    if not 0 < triangle_count <= MAX_TRIANGLES or not 0 < vertex_count < 65536:
        raise ValueError(f"{name}: mesh exceeds the plant geometry budget ({triangle_count} triangles)")

    positions = [(x, y - ground, z) for x, y, z in mesh.positions]
    vertex_offset = len(binary)
    for position, normal, color in zip(positions, mesh.normals(), mesh.colors, strict=True):
        rgb = (round(255 * min(1.0, max(0.0, channel))) for channel in color)
        binary.extend(Vertex.pack(*position, *normal, *rgb, 255))
    index_offset = len(binary)
    indices = struct.pack(f"<{len(mesh.indices)}H", *mesh.indices)
    binary.extend(indices)
    binary.extend(b"\0" * (-len(binary) % 4))

    view = len(document["bufferViews"])
    vertex_view = {"buffer": 0}
    if vertex_offset:
        vertex_view["byteOffset"] = vertex_offset
    vertex_view.update(byteLength=index_offset - vertex_offset, byteStride=Vertex.size, target=34962)
    document["bufferViews"].extend([
        vertex_view,
        {"buffer": 0, "byteOffset": index_offset, "byteLength": len(indices), "target": 34963},
    ])
    accessor = len(document["accessors"])
    document["accessors"].extend([
            {"bufferView": view, "componentType": 5126, "count": vertex_count, "type": "VEC3",
             "min": [float32(min(values)) for values in zip(*positions)],
             "max": [float32(max(values)) for values in zip(*positions)]},
            {"bufferView": view, "byteOffset": 12, "componentType": 5126, "count": vertex_count, "type": "VEC3"},
            {"bufferView": view, "byteOffset": 24, "componentType": 5121, "count": vertex_count,
             "type": "VEC4", "normalized": True},
            {"bufferView": view + 1, "componentType": 5123, "count": len(mesh.indices), "type": "SCALAR"},
    ])
    return {
        "attributes": {"POSITION": accessor, "NORMAL": accessor + 1, "COLOR_0": accessor + 2},
        "indices": accessor + 3,
        "material": 0,
    }
