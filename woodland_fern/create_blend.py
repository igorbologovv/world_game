"""Create an editable Blender .blend from the exported LOD0 GLB.

Run from project root:
    blender -b --python woodland_fern/create_blend.py -- --asset-dir ./woodland_fern
"""
import argparse
import sys
from pathlib import Path

import bpy
from mathutils import Vector


def main():
    cli = sys.argv[sys.argv.index('--') + 1:] if '--' in sys.argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument('--asset-dir', default='./woodland_fern')
    args = parser.parse_args(cli)
    folder = Path(args.asset_dir).expanduser().resolve()
    glb = folder / 'woodland_fern_lod0.glb'
    if not glb.exists():
        raise FileNotFoundError(f'Missing fern model: {glb}')
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(glb))
    # In Blender the imported glTF +Y-up coordinates become Blender +Z-up.
    for obj in bpy.context.scene.objects:
        if obj.type == 'MESH':
            obj.select_set(True)
    scene = bpy.context.scene
    scene.render.resolution_x = 1400
    scene.render.resolution_y = 1000
    scene.render.resolution_percentage = 100
    # Useful studio camera for a Blender preview or further modeling.
    bpy.ops.object.camera_add(location=(1.7, -2.6, 1.25))
    cam = bpy.context.object
    target = Vector((0.0, 0.0, 0.36))
    cam.rotation_euler = (target - cam.location).to_track_quat('-Z', 'Y').to_euler()
    cam.data.type = 'ORTHO'
    cam.data.ortho_scale = 2.5
    scene.camera = cam
    bpy.ops.object.light_add(type='AREA', location=(-1.8, -1.8, 3.0))
    light = bpy.context.object
    light.data.energy = 440.0
    light.data.shape = 'DISK'
    light.data.size = 3.0
    light.rotation_euler = (target - light.location).to_track_quat('-Z', 'Y').to_euler()
    for screen in bpy.data.screens:
        for area in screen.areas:
            if area.type == 'VIEW_3D':
                area.spaces.active.shading.type = 'MATERIAL'
                area.spaces.active.region_3d.view_distance = 2.5
    output=folder/'woodland_fern.blend'
    bpy.ops.wm.save_as_mainfile(filepath=str(output))
    print('Saved', output)


if __name__ == '__main__':
    main()
