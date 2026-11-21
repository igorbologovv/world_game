"""Extract reusable vegetation prefabs from the ORIGINAL woodland glTF.

Run with Blender, not the already flattened assets/models/starter/.../scene.glb:
    blender -b --python tools/extract_woodland_prefabs.py -- --list
    blender -b --python tools/extract_woodland_prefabs.py -- --roots Tree.001 Tree.002

A tree assembled from many leaf/trunk objects must be exported as one group.
The inspector prints object names, materials and bounds so a human can select
real root nodes. No ground, rocks, water or pond base is exported implicitly.
"""

import argparse
import json
import re
import sys
from pathlib import Path

import bpy
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SOURCE = ROOT / "low_poly_tree_scene_free" / "scene.gltf"
DEFAULT_OUTPUT = ROOT / "assets" / "models" / "vegetation" / "woodland"
EXCLUDE = re.compile(r"(terrain|ground|floor|pond|water|lake|river|soil|base|island|rock|stone|cliff|plane)", re.I)


def args():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    p.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    p.add_argument("--list", action="store_true", help="Inspect actual imported object names; never export")
    p.add_argument("--water-report", action="store_true", help="Inspect pond/water source materials and their textures")
    p.add_argument("--roots", nargs="+", help="Explicit root names to export (as printed by --list)")
    p.add_argument("--auto-trees", type=int, default=0, help="Export this many tallest distinct Bark+Leaves pairs")
    p.add_argument("--lod-ratios", type=float, nargs=3, default=(1.0, 0.45, 0.18))
    return p.parse_args(sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else [])


def objects_under(root):
    result = [root]
    for child in root.children:
        result.extend(objects_under(child))
    return result


def bounds(meshes):
    pts = [obj.matrix_world @ Vector(v) for obj in meshes for v in obj.bound_box]
    lo = Vector(tuple(min(p[k] for p in pts) for k in range(3)))
    hi = Vector(tuple(max(p[k] for p in pts) for k in range(3)))
    return lo, hi


def inspect():
    roots = []
    for obj in bpy.data.objects:
        if obj.type != "MESH" and not any(child.type == "MESH" for child in objects_under(obj)):
            continue
        meshes = [x for x in objects_under(obj) if x.type == "MESH"]
        if not meshes:
            continue
        materials = sorted({slot.material.name for m in meshes for slot in m.material_slots if slot.material})
        lo, hi = bounds(meshes)
        roots.append({
            "name": obj.name,
            "parent": obj.parent.name if obj.parent else None,
            "mesh_count": len(meshes),
            "materials": materials,
            "center_xz": [round((lo[0] + hi[0]) / 2, 2), round((lo[2] + hi[2]) / 2, 2)],
            "size": [round(hi[i] - lo[i], 2) for i in range(3)],
            "excluded_by_name": bool(EXCLUDE.search(obj.name)),
        })
    return roots


def export_one(name, roots, output, ratios):
    group = [obj for root in roots for obj in objects_under(root)]
    meshes = list({obj.name: obj for obj in group if obj.type == "MESH"}.values())
    if not meshes:
        raise ValueError(f"{name!r} has no meshes")
    if EXCLUDE.search(name):
        raise ValueError(f"{name!r} looks like terrain/pond/ground: refusing to export")
    material_names = {slot.material.name for m in meshes for slot in m.material_slots if slot.material}
    if any(EXCLUDE.search(mat) for mat in material_names):
        raise ValueError(f"{name!r} contains ground/water materials {sorted(material_names)}; choose smaller roots")
    low, high = bounds(meshes)
    if high.y - low.y < 0.5:
        raise ValueError(f"{name!r} is too flat to be a tree")
    position = Vector(((high.x + low.x) / 2, low.y, (high.z + low.z) / 2))
    safe = re.sub(r"[^a-z0-9_-]+", "_", name.lower()).strip("_")
    destination = output / safe
    destination.mkdir(parents=True, exist_ok=True)
    for level, ratio in enumerate(ratios):
        bpy.ops.object.select_all(action="DESELECT")
        clones = []
        for original in meshes:
            clone = original.copy()
            clone.data = original.data.copy()
            bpy.context.collection.objects.link(clone)
            clone.parent = None
            clone.matrix_world = original.matrix_world.copy()
            clone.matrix_world.translation -= position
            clones.append(clone)
            clone.select_set(True)
            if level and 0 < ratio < 1:
                decimate = clone.modifiers.new(f"Woodland LOD{level}", "DECIMATE")
                decimate.ratio = ratio
        bpy.context.view_layer.objects.active = clones[0]
        filepath = destination / f"tree_lod{level}.glb"
        bpy.ops.export_scene.gltf(
            filepath=str(filepath), export_format="GLB",
            use_selection=True, export_apply=True,
            export_materials="EXPORT",
        )
        for obj in clones:
            bpy.data.objects.remove(obj, do_unlink=True)
    return {"source_root": name, "asset_dir": str(destination.relative_to(ROOT)), "height_m": high.y-low.y, "lods": len(ratios)}




def automatic_tree_pairs(limit):
    # The original Sketchfab source contains paired Bark/Leaves meshes as
    # separate objects. Do NOT infer prefab trees from flattened scene.glb.
    bark = [obj for obj in bpy.data.objects
            if re.match(r"^Bark(?:\\.\\d+)?(?:_\\d+)?$", obj.name)]
    leaves = [obj for obj in bpy.data.objects
              if re.match(r"^Leaves(?:\\.\\d+)?(?:_\\d+)?$", obj.name)]
    def center(root):
        group = [o for o in objects_under(root) if o.type == "MESH"]
        if not group:
            return None
        lo, hi = bounds(group)
        return (lo + hi) / 2, hi.y - lo.y
    trees = [(obj, *center(obj)) for obj in bark if center(obj)]
    trees.sort(key=lambda item: (-item[2], item[0].name))
    unused = {obj.name: obj for obj in leaves}
    result = []
    for trunk, point, height in trees:
        if len(result) == limit:
            break
        if height < 2.0 or not unused:
            continue
        candidates = []
        for candidate in unused.values():
            info = center(candidate)
            if info is None:
                continue
            leaf_center, _ = info
            delta = (leaf_center - point)
            candidates.append((delta.x * delta.x + delta.z * delta.z, candidate.name, candidate))
        if not candidates:
            break
        dist, _, crown = min(candidates, key=lambda item: (item[0], item[1]))
        if dist > max(3.0, height * 0.55) ** 2:
            continue
        unused.pop(crown.name)
        result.append((trunk, crown))
    if len(result) < limit:
        print(f"WARNING: only {len(result)} valid Bark/Leaves pairs found (requested {limit})")
    if not result:
        raise RuntimeError("No paired tree objects found; run --list to inspect imported names")
    return result

def water_report():
    report = []
    water_objects = [obj for obj in bpy.data.objects if re.search(r"(water|pond|lake|river)", obj.name, re.I)]
    water_materials = {slot.material.name for obj in water_objects for slot in obj.material_slots if slot.material}
    for mat in bpy.data.materials:
        if mat.name not in water_materials and not re.search(r"(water|pond|lake|river|liquid)", mat.name, re.I):
            continue
        nodes = mat.node_tree.nodes if mat.use_nodes and mat.node_tree else []
        textures = []
        for node in nodes:
            if node.type == "TEX_IMAGE" and node.image:
                textures.append({
                    "image": node.image.name,
                    "source": node.image.source,
                    "path": bpy.path.abspath(node.image.filepath),
                    "size": list(node.image.size),
                })
        report.append({
            "material": mat.name,
            "blend_method": str(getattr(mat, "surface_render_method", "unknown")),
            "textures": textures,
            "note": "Static glTF material data is not a dynamic river shader.",
        })
    return report

def main():
    options = args()
    if not options.source.exists():
        raise FileNotFoundError(options.source)
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    bpy.ops.import_scene.gltf(filepath=str(options.source))
    if options.water_report:
        print(json.dumps({"water_materials": water_report()}, indent=2))
        return
    if options.auto_trees > 0:
        pairs = automatic_tree_pairs(options.auto_trees)
        done = [export_one(f"woodland_tree_{i:02}", [bark, leaves], options.output, options.lod_ratios)
                for i, (bark, leaves) in enumerate(pairs)]
        print(json.dumps(done, indent=2))
        return
    if options.list or not options.roots:
        print(json.dumps(inspect(), indent=2))
        if not options.list:
            raise SystemExit("Choose actual tree roots with --roots; refusing to guess or export the pond.")
        return
    if any(r <= 0 or r > 1 for r in options.lod_ratios):
        raise ValueError("LOD ratios must be in (0,1]")
    done = [export_one(name, [bpy.data.objects[name]], options.output, options.lod_ratios)
            for name in options.roots]
    print(json.dumps(done, indent=2))
    print("Review exports in Blender before using as runtime vegetation assets.")


if __name__ == "__main__":
    main()
