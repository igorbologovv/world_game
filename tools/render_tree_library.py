"""Render the exported tree library in Blender Cycles for visual inspection.

Run Blender in background with --python-exit-code 1 and this file. Arguments
after -- are optional tree slugs. Outputs are written to target/tree-library-studio.
"""

import sys
from pathlib import Path

import bpy
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "assets/models/vegetation/library"
OUTPUT = ROOT / "target/tree-library-studio"


def render(model, name):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(model))
    positions = [
        obj.matrix_world @ vertex.co
        for obj in bpy.context.scene.objects if obj.type == "MESH"
        for vertex in obj.data.vertices
    ]
    high = max(point.z for point in positions)
    width = max(Vector((p.x, p.y)).length for p in positions) * 2

    floor = bpy.data.materials.new("Neutral studio ground")
    floor.diffuse_color = (.16, .17, .14, 1)
    bpy.ops.mesh.primitive_plane_add(size=1500)
    bpy.context.object.data.materials.append(floor)

    scene = bpy.context.scene
    scene.world = bpy.data.worlds.new("Studio")
    scene.world.use_nodes = True
    background = scene.world.node_tree.nodes["Background"]
    background.inputs[0].default_value = (.28, .40, .58, 1)
    background.inputs[1].default_value = .52
    focus = Vector((0, 0, high * .48))
    for location, power, size in [((-.7, -1., 1.45), 3200, 5), ((.5, .9, 1.05), 1600, 7)]:
        bpy.ops.object.light_add(type="AREA", location=Vector(location) * high)
        lamp = bpy.context.object
        lamp.data.energy = power * (high / 10) ** 2
        lamp.data.shape = "DISK"
        lamp.data.size = size
        lamp.rotation_euler = (focus - lamp.location).to_track_quat("-Z", "Y").to_euler()

    bpy.ops.object.camera_add(location=Vector((1.45, -2.1, .98)) * high)
    camera = bpy.context.object
    camera.rotation_euler = (focus - camera.location).to_track_quat("-Z", "Y").to_euler()
    camera.data.type = "ORTHO"
    camera.data.ortho_scale = max(high * 1.18, width * 1.20)
    scene.camera = camera
    scene.render.engine = "CYCLES"
    scene.cycles.samples = 12
    scene.cycles.use_denoising = True
    scene.render.resolution_x = 480
    scene.render.resolution_y = 512
    scene.render.resolution_percentage = 100
    scene.render.filepath = str(OUTPUT / f"{name}.png")
    bpy.ops.render.render(write_still=True)


if __name__ == "__main__":
    requested = set(sys.argv[sys.argv.index("--") + 1:]) if "--" in sys.argv else set()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for model in sorted(ASSETS.glob("*/tree_0_lod0.glb")):
        if not requested or model.parent.name in requested:
            render(model, model.parent.name)
