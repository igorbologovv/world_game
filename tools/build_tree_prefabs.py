"""Pack standalone tree LODs with shared textures. Requires Blender and Pillow.

blender -b --python-exit-code 1 --python tools/build_tree_prefabs.py

The woodland input is the pair of Bark/Leaves exports from
extract_woodland_prefabs.py (use --auto-trees 2). Pass --woodland-input if
those exports are in a different directory.
"""

import argparse
import json
import struct
import sys
from pathlib import Path

import bpy
from mathutils import Matrix, Vector
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
DESTINATION = ROOT / "assets/models/vegetation"
MAPS = ("baseColor", "normal", "metallicRoughness")


def read_glb(path):
    with path.open("rb") as stream:
        magic, version, _, length, kind = struct.unpack("<5I", stream.read(20))
        assert (magic, version, kind) == (0x46546C67, 2, 0x4E4F534A), path
        document = json.loads(stream.read(length))
        length, kind = struct.unpack("<2I", stream.read(8))
        assert kind == 0x004E4942, path
        return document, stream.read(length)


def write_glb(path, document, buffer):
    data = json.dumps(document, separators=(",", ":")).encode()
    data += b" " * (-len(data) % 4)
    with path.open("wb") as stream:
        stream.write(struct.pack("<5I", 0x46546C67, 2, 28 + len(data) + len(buffer), len(data), 0x4E4F534A))
        stream.write(data)
        stream.write(struct.pack("<2I", len(buffer), 0x004E4942))
        stream.write(buffer)


def textures_for(document, bark, leaves, cutoff, bark_normal_uv=0):
    document["images"] = []
    document["textures"] = []
    document["samplers"] = [{"magFilter": 9729, "minFilter": 9987}]

    def texture(uri):
        index = len(document["images"])
        document["images"].append({"uri": uri})
        document["textures"].append({"source": index, "sampler": 0})
        return {"index": index}

    for material in document["materials"]:
        is_leaf = material["name"] == "Leaves"
        maps = leaves if is_leaf else bark
        pbr = {
            "baseColorTexture": texture(maps["baseColor"]),
            "roughnessFactor": 1.0 if "metallicRoughness" in maps else 0.82,
            "metallicFactor": 0.0,
        }
        if "metallicRoughness" in maps:
            pbr["metallicRoughnessTexture"] = texture(maps["metallicRoughness"])
        material["pbrMetallicRoughness"] = pbr
        if "normal" in maps:
            material["normalTexture"] = texture(maps["normal"])
            if not is_leaf and bark_normal_uv:
                material["normalTexture"]["texCoord"] = bark_normal_uv
        material["doubleSided"] = True
        if is_leaf:
            material["alphaMode"] = "MASK"
            material["alphaCutoff"] = cutoff


def save_map(image, directory, name, kind):
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{name}_{kind}.png"
    image.save(path, optimize=True)
    return f"textures/{path.name}"


def resized_maps(source, prefix, kinds, destination, name, size):
    result = {}
    for kind in kinds:
        file = next(source.glob(f"{prefix}_{kind}.*"))
        with Image.open(file) as image:
            mode = "RGBA" if kind == "baseColor" and "A" in image.mode else "RGB"
            image = image.convert(mode)
            image.thumbnail(size, Image.Resampling.LANCZOS)
            result[kind] = save_map(image, destination, name, kind)
    return result


def atlas(source, slots, kinds, destination, name, rgba=False):
    result = {}
    for kind in kinds:
        mode = "RGBA" if rgba and kind == "baseColor" else "RGB"
        image = Image.new(mode, (1024, 1024))
        for material, (x, y, width, height, repeats) in slots.items():
            file = next(source.glob(f"{material}_{kind}.*"))
            with Image.open(file) as tile:
                tile = tile.convert(mode).resize((width, height), Image.Resampling.LANCZOS)
            # Repeat bark vertically and extend edge pixels into the mip gutters.
            strip = Image.new(mode, (width, height * repeats))
            for i in range(repeats):
                strip.paste(tile, (0, i * height))
            end = height * repeats
            image.paste(strip, (x, y))
            for src, box in [
                ((0, 0, 1, end), (x - 4, y, x, y + end)),
                ((width - 1, 0, width, end), (x + width, y, x + width + 4, y + end)),
                ((0, 0, width, 1), (x, y - 4, x + width, y)),
                ((0, end - 1, width, end), (x, y + end, x + width, y + end + 4)),
            ]:
                image.paste(strip.crop(src).resize((box[2] - box[0], box[3] - box[1])), box)
        result[kind] = save_map(image, destination, name, kind)
    return result


def remap_uv(mesh, slot):
    x, y, width, height, repeats = slot
    # Blender's UV origin is lower-left. glTF and Pillow use upper-left.
    for corner in mesh.uv_layers[0].uv:
        u, v = corner.vector
        corner.vector = (
            (x + u * width) / 1024,
            1 - (y + (1 - v) * height) / 1024,
        )


def mesh_group(objects, material, ratio, level, normalize, slots):
    bpy.ops.object.select_all(action="DESELECT")
    group = []
    for original in objects:
        clone = original.copy()
        clone.data = original.data.copy()
        bpy.context.collection.objects.link(clone)
        clone.parent = None
        clone.matrix_world = Matrix.Identity(4)
        clone.data.transform(normalize @ original.matrix_world)
        source_material = clone.data.materials[0].name
        if source_material in slots:
            remap_uv(clone.data, slots[source_material])
        clone.data.materials[0] = material
        clone.select_set(True)
        bpy.context.view_layer.objects.active = clone
        if ratio < 1:
            decimate = clone.modifiers.new(f"LOD{level}", "DECIMATE")
            decimate.ratio = ratio
            bpy.ops.object.modifier_apply(modifier=decimate.name)
        group.append(clone)
    for clone in group:
        clone.select_set(True)
    if len(group) > 1:
        bpy.ops.object.join()
    return bpy.context.view_layer.objects.active


def build_tree(source, destination, height, bark_maps, leaf_maps, cutoff, slots, ratios, bark_normal_uv=0):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(source))
    material_groups = {"Bark": [], "Leaves": []}
    for obj in bpy.context.scene.objects:
        if obj.type == "MESH":
            name = obj.data.materials[0].name
            leaves = name == "Tree_2Mat" if source.parent.name == "coconut_palm" else name != "bark8"
            material_groups["Leaves" if leaves else "Bark"].append(obj)
    if not all(material_groups.values()):
        raise ValueError(f"Expected bark and foliage in {source}")

    def points(objects):
        return [obj.matrix_world @ v.co for obj in objects for v in obj.data.vertices]

    bark = points(material_groups["Bark"])
    leaves = points(material_groups["Leaves"])
    floor = min(v.z for v in bark)
    span = max(v.z for v in bark + leaves) - floor
    roots = [v for v in bark if v.z <= floor + span * 0.015]
    center = sum(roots, Vector()) / len(roots)
    center.z = floor
    normalize = Matrix.Scale(height / span, 4) @ Matrix.Translation(-center)
    materials = {name: bpy.data.materials.new(name) for name in material_groups}
    for level, (wood_ratio, leaf_ratio) in enumerate(ratios):
        wood = mesh_group(material_groups["Bark"], materials["Bark"], wood_ratio, level, normalize, slots)
        foliage = mesh_group(material_groups["Leaves"], materials["Leaves"], leaf_ratio, level, normalize, slots)
        wood.select_set(True)
        path = destination / f"tree_lod{level}.glb"
        bpy.ops.export_scene.gltf(filepath=str(path), export_format="GLB", use_selection=True)
        for obj in (wood, foliage):
            bpy.data.objects.remove(obj, do_unlink=True)
        document, buffer = read_glb(path)
        textures_for(document, bark_maps, leaf_maps, cutoff, bark_normal_uv)
        write_glb(path, document, buffer)


def pack_woodland(source, exports, destination):
    """Externalize the shared maps and normalize geometry to the oak LOD heights."""
    textures = destination / "textures"
    bark = resized_maps(source, "Bark", MAPS, textures, "bark", (1024, 1024))
    leaves = resized_maps(source, "Leaves", MAPS[:2], textures, "leaves", (1024, 1024))
    for variant, height in enumerate((14.82, 15.64)):
        size = None
        name = f"woodland_tree_{variant:02}"
        for level in range(3):
            document, buffer = read_glb(exports / name / f"tree_lod{level}.glb")
            positions = {
                primitive["attributes"]["POSITION"]
                for mesh in document["meshes"] for primitive in mesh["primitives"]
            }
            accessors = [document["accessors"][index] for index in positions]
            if size is None:
                size = max(accessor["max"][1] for accessor in accessors)
            scale = height / size
            # Blender writes separate, tightly packed float32 position views.
            images = {image["bufferView"] for image in document["images"]}
            geometry = bytearray()
            views = []
            remap = {}
            for index, view in enumerate(document["bufferViews"]):
                if index in images:
                    continue
                start = view.get("byteOffset", 0)
                length = view["byteLength"]
                remap[index] = len(views)
                views.append({**view, "byteOffset": len(geometry)})
                geometry.extend(buffer[start:start + length])
                geometry.extend(bytes(-len(geometry) % 4))
            for accessor in document["accessors"]:
                accessor["bufferView"] = remap[accessor["bufferView"]]
            for accessor in accessors:
                view = views[accessor["bufferView"]]
                start = view["byteOffset"] + accessor.get("byteOffset", 0)
                assert view.get("byteStride", 12) == 12 and accessor["componentType"] == 5126
                for vertex in range(accessor["count"]):
                    offset = start + vertex * 12
                    values = struct.unpack_from("<3f", geometry, offset)
                    struct.pack_into("<3f", geometry, offset, *(v * scale for v in values))
                for key in ("min", "max"):
                    accessor[key] = [v * scale for v in accessor[key]]
            document["bufferViews"] = views
            document["buffers"][0]["byteLength"] = len(geometry)
            textures_for(document, bark, leaves, 0.42)
            write_glb(destination / f"{name}_lod{level}.glb", document, geometry)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=ROOT)
    parser.add_argument("--woodland-input", type=Path)
    options = parser.parse_args(sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else [])
    sources = options.source_root

    palm = DESTINATION / "coconut_palm"
    palm_source = sources / "coconut_palm"
    palm_slots = {
        "Tree_0Mat": (4, 4, 500, 100, 10),
        "Tree_1Mat": (516, 4, 244, 500, 1),
        "Tree_3Mat": (772, 4, 244, 500, 1),
        "Tree_4Mat": (516, 516, 500, 500, 1),
    }
    bark = atlas(palm_source / "textures", palm_slots, MAPS, palm / "textures", "bark")
    leaves = resized_maps(palm_source / "textures", "Tree_2Mat", MAPS, palm / "textures", "leaves", (1024, 1024))
    build_tree(palm_source / "scene.gltf", palm, 12.0, bark, leaves, 0.25811078,
               palm_slots, ((0.62, 1), (0.25, 0.50), (0.11, 0.22)))

    broadleaf = DESTINATION / "tree_part9"
    broadleaf_source = sources / "tree_part9"
    names = ("tree4_leaf_small2", "tree5_leaf_small3", "tree4_leaf_small1", "trees_bushes_0001_a_1")
    leaf_slots = {name: (4 + (i % 2) * 512, 4 + (i // 2) * 512, 504, 504, 1)
                  for i, name in enumerate(names)}
    bark = resized_maps(broadleaf_source / "textures", "bark8", MAPS[:2], broadleaf / "textures", "bark", (1024, 1024))
    leaves = atlas(broadleaf_source / "textures", leaf_slots, MAPS[:1], broadleaf / "textures", "leaves", rgba=True)
    build_tree(broadleaf_source / "scene.gltf", broadleaf, 25.82, bark, leaves, 0.70,
               leaf_slots, ((0.10, 0.19), (0.05, 0.10), (0.02, 0.045)), bark_normal_uv=1)

    woodland = DESTINATION / "woodland"
    pack_woodland(sources / "low_poly_tree_scene_free/textures",
                  options.woodland_input or sources / "assets/models/vegetation/woodland", woodland)


if __name__ == "__main__":
    main()
