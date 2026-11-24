"""Rebuild authored tree LOD1/2 from the canonical, normalized LOD0 GLBs.

Run with blender -b --python-exit-code 1 --python tools/build_tree_lods.py.
"""

import copy
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

import bpy

sys.dont_write_bytecode = True
sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_tree_prefabs import read_glb, write_glb


MODELS = (
    ("vegetation/coconut_palm/tree", "Leaves", ((0.40, 0.96), (0.18, 0.91))),
    ("vegetation/tree_part9/tree", "Leaves", ((0.42, 0.95), (0.22, 0.86))),
    ("vegetation/woodland/woodland_tree_00", "Leaves", ((0.42, 0.97), (0.20, 0.88))),
    ("vegetation/woodland/woodland_tree_01", "Leaves", ((0.42, 0.97), (0.20, 0.88))),
    # The oak's leaf cards already have two triangles each. Simplify branches
    # below their canopy and preserve the shape and translucency of the cards.
    ("hero_oak/oak", "Sunlit_Oak_Foliage", ((0.42, 1.0), (0.29, 1.0))),
)


def simplify(source, output, leaf_material, ratios):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(source), merge_vertices=True)
    for obj in bpy.context.scene.objects:
        if obj.type != "MESH":
            continue
        material, = obj.data.materials
        ratio = ratios[1] if material.name == leaf_material else ratios[0]
        if ratio < 1.0:
            bpy.context.view_layer.objects.active = obj
            modifier = obj.modifiers.new("Simplify LOD0 topology", "DECIMATE")
            modifier.ratio = ratio
            modifier.use_collapse_triangulate = True
            bpy.ops.object.modifier_apply(modifier=modifier.name)
    bpy.ops.export_scene.gltf(filepath=str(output), export_format="GLB",
                              export_normals=True, export_texcoords=True)


def preserve_materials(original, original_buffer, result, result_buffer):
    """Use the unchanged LOD0 material definitions, samplers, and image bytes."""
    geometry = bytearray()
    views = []

    def copy_view(document, buffer, index):
        old = document["bufferViews"][index]
        start = old.get("byteOffset", 0)
        views.append({**old, "byteOffset": len(geometry)})
        geometry.extend(buffer[start:start + old["byteLength"]])
        geometry.extend(bytes(-len(geometry) % 4))
        return len(views) - 1

    remap = {}
    for accessor in result["accessors"]:
        index = accessor["bufferView"]
        if index not in remap:
            remap[index] = copy_view(result, result_buffer, index)
        accessor["bufferView"] = remap[index]

    material_by_name = {material["name"]: i for i, material in enumerate(original["materials"])}
    for mesh in result["meshes"]:
        for primitive in mesh["primitives"]:
            name = result["materials"][primitive["material"]]["name"]
            primitive["material"] = material_by_name[name]

    for key in ("materials", "images", "textures", "samplers", "extensionsUsed", "extensionsRequired"):
        result.pop(key, None)
        if key in original:
            result[key] = copy.deepcopy(original[key])
    for image in result["images"]:
        if "bufferView" in image:
            image["bufferView"] = copy_view(original, original_buffer, image["bufferView"])
    result["bufferViews"] = views
    result["buffers"] = [{"byteLength": len(geometry)}]
    return geometry


def triangle_count(document):
    return sum(document["accessors"][primitive["indices"]]["count"] // 3
               for mesh in document["meshes"] for primitive in mesh["primitives"])


def main():
    root = Path(__file__).resolve().parents[1] / "assets/models"
    with TemporaryDirectory(prefix="tree-lods-") as directory:
        temporary = Path(directory) / "tree.glb"
        for name, leaves, ratios in MODELS:
            source = root / f"{name}_lod0.glb"
            original, buffer = read_glb(source)
            counts = [triangle_count(original)]
            for level, ratio in enumerate(ratios, start=1):
                simplify(source, temporary, leaves, ratio)
                result, result_buffer = read_glb(temporary)
                geometry = preserve_materials(original, buffer, result, result_buffer)
                counts.append(triangle_count(result))
                if counts[-1] >= counts[-2]:
                    raise ValueError(f"{name}: LOD{level} did not reduce the triangle count")
                write_glb(root / f"{name}_lod{level}.glb", result, geometry)
            print(f"{name}: {counts}")


if __name__ == "__main__":
    main()
