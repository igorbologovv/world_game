"""Pack the supplied Low Poly Tree Scene Free for Bevy.

Run from the repository root with NumPy and Pillow installed.
"""

import io
import json
import struct
from collections import Counter
from collections import defaultdict
from pathlib import Path

import numpy as np
from PIL import Image


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "low_poly_tree_scene_free"
DESTINATION = ROOT / "assets/models/starter/low_poly_tree_scene"
COMPONENTS = {5121: "u1", 5123: "<u2", 5125: "<u4", 5126: "<f4"}
WIDTHS = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}
ATTRIBUTES = ("POSITION", "NORMAL", "TANGENT", "TEXCOORD_0")
GROUND = 2
BED_LEVEL = -4.0


def read_accessor(source, buffer, index):
    accessor = source["accessors"][index]
    view = source["bufferViews"][accessor["bufferView"]]
    dtype = np.dtype(COMPONENTS[accessor["componentType"]])
    width = WIDTHS[accessor["type"]]
    return np.ndarray(
        (accessor["count"], width), dtype, buffer=buffer,
        offset=view.get("byteOffset", 0) + accessor.get("byteOffset", 0),
        strides=(view.get("byteStride", width * dtype.itemsize), dtype.itemsize),
    )


def primitives(source, node_index, parent):
    node = source["nodes"][node_index]
    local = np.array(node.get("matrix", np.eye(4).ravel())).reshape(4, 4).T
    world = parent @ local
    if "mesh" in node:
        for primitive in source["meshes"][node["mesh"]]["primitives"]:
            yield world, primitive
    for child in node.get("children", []):
        yield from primitives(source, child, world)


def normalize(vectors):
    return vectors / np.maximum(np.linalg.norm(vectors, axis=1, keepdims=True), 1e-10)


def baked_meshes(source, buffer):
    groups = defaultdict(list)
    for node in source["scenes"][source["scene"]]["nodes"]:
        for transform, primitive in primitives(source, node, np.eye(4)):
            attrs = {
                name: read_accessor(source, buffer, index)
                for name, index in primitive["attributes"].items()
                if name in ATTRIBUTES
            }
            basis = transform[:3, :3]
            mirror = np.linalg.det(basis) < 0
            attrs["POSITION"] = attrs["POSITION"] @ basis.T + transform[:3, 3]
            attrs["NORMAL"] = normalize(attrs["NORMAL"] @ np.linalg.inv(basis))
            if "TANGENT" in attrs:
                tangent = attrs["TANGENT"]
                xyz = normalize(tangent[:, :3] @ basis.T)
                attrs["TANGENT"] = np.column_stack((xyz, tangent[:, 3] * (-1 if mirror else 1)))
            indices = read_accessor(source, buffer, primitive["indices"]).reshape(-1, 3)
            if mirror:
                indices = indices[:, [0, 2, 1]]
            # The detached specimen trees and grass samples sit beside the
            # pond. Put them on the same terrain datum as the pond bed.
            if attrs["POSITION"][:, 0].min() > 30:
                attrs["POSITION"][:, 1] += BED_LEVEL
            groups[primitive["material"]].append((attrs, indices.ravel()))
    return groups


def soil_edge(ground):
    attrs, indices = ground
    points = attrs["POSITION"]
    weld = [tuple(point.round(4)) for point in points]
    counted = Counter()
    edges = []
    for a, b, c in indices.reshape(-1, 3):
        for first, last in ((a, b), (b, c), (c, a)):
            key = tuple(sorted((weld[first], weld[last])))
            edges.append((first, last, key))
            counted[key] += 1

    positions, normals, triangles = [], [], []
    for first, last, key in edges:
        if counted[key] != 1:
            continue
        a, b = points[first], points[last]
        bottom_a = [a[0], -9.0, a[2]]
        bottom_b = [b[0], -9.0, b[2]]
        normal = np.cross(a - b, bottom_a - b)
        normal /= np.linalg.norm(normal)
        start = len(positions)
        positions.extend((b, a, bottom_a, bottom_b))
        normals.extend([normal] * 4)
        triangles.extend((start, start + 1, start + 2, start, start + 2, start + 3))
    return ({"POSITION": np.array(positions), "NORMAL": np.array(normals)}, np.array(triangles))


def pack(source_path=SOURCE, destination=DESTINATION):
    source = json.loads((source_path / "scene.gltf").read_bytes())
    groups = baked_meshes(source, (source_path / source["buffers"][0]["uri"]).read_bytes())
    positions = np.concatenate([a["POSITION"] for group in groups.values() for a, _ in group])
    center = (positions.min(axis=0) + positions.max(axis=0)) / 2
    center[1] = 0  # Preserve the authored pond and ground elevations.
    output = {
        key: source[key] for key in ("asset", "extensionsUsed", "materials", "samplers", "textures")
    }
    groups[len(output["materials"])].append(soil_edge(groups[GROUND][0]))
    output["materials"].append({
        "name": "Scene soil edge",
        "pbrMetallicRoughness": {
            "baseColorFactor": [0.135, 0.085, 0.043, 1], "metallicFactor": 0, "roughnessFactor": 1,
        },
    })
    output.update(accessors=[], bufferViews=[], images=[], meshes=[], nodes=[], scene=0)
    binary = bytearray()

    def add_view(data, target=None):
        offset = len(binary)
        binary.extend(data)
        binary.extend(bytes(-len(binary) % 4))
        view = {"buffer": 0, "byteOffset": offset, "byteLength": len(data)}
        if target is not None:
            view["target"] = target
        output["bufferViews"].append(view)
        return len(output["bufferViews"]) - 1

    def add_accessor(array, component, vector_type, bounds=False):
        view = add_view(array.astype(COMPONENTS[component]).tobytes(), 34962 if component == 5126 else 34963)
        accessor = {"bufferView": view, "componentType": component, "count": len(array), "type": vector_type}
        if bounds:
            accessor.update(min=array.min(axis=0).tolist(), max=array.max(axis=0).tolist())
        output["accessors"].append(accessor)
        return len(output["accessors"]) - 1

    for material, group in sorted(groups.items()):
        # Static objects with the same material can share a draw call after
        # baking their transforms. All original triangles and UV0 are retained.
        attributes = {}
        for name in group[0][0]:
            values = np.concatenate([attrs[name] for attrs, _ in group]).astype("<f4")
            if name == "POSITION":
                values -= center
            attributes[name] = add_accessor(values, 5126, f"VEC{values.shape[1]}", name == "POSITION")
        parts, offset = [], 0
        for attrs, indices in group:
            parts.append(indices.astype("<u4") + offset)
            offset += len(attrs["POSITION"])
        primitive = {
            "attributes": attributes, "material": material,
            "indices": add_accessor(np.concatenate(parts), 5125, "SCALAR"),
        }
        name = output["materials"][material]["name"]
        output["nodes"].append({"mesh": len(output["meshes"]), "name": name})
        output["meshes"].append({"primitives": [primitive], "name": name})

    for image in source["images"]:
        path = source_path / image["uri"]
        pixels = path.read_bytes()
        if path.suffix.lower() in (".jpeg", ".jpg"):
            # PNG is available in the client's current Bevy feature set.
            converted = io.BytesIO()
            with Image.open(io.BytesIO(pixels)) as original:
                original.save(converted, format="PNG")
            pixels = converted.getvalue()
        output["images"].append({"bufferView": add_view(pixels), "mimeType": "image/png", "name": path.stem})

    output["buffers"] = [{"byteLength": len(binary)}]
    output["scenes"] = [{"name": source["asset"]["extras"]["title"], "nodes": list(range(len(output["nodes"])))}]
    encoded = json.dumps(output, separators=(",", ":")).encode()
    encoded += b" " * (-len(encoded) % 4)
    destination.mkdir(parents=True, exist_ok=True)
    with (destination / "scene.glb").open("wb") as packed:
        packed.write(struct.pack("<4sII", b"glTF", 2, 28 + len(encoded) + len(binary)))
        packed.write(struct.pack("<I4s", len(encoded), b"JSON"))
        packed.write(encoded)
        packed.write(struct.pack("<I4s", len(binary), b"BIN\0"))
        packed.write(binary)
    (destination / "LICENSE.txt").write_bytes((source_path / "license.txt").read_bytes())


if __name__ == "__main__":
    pack()
