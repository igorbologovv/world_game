"""Hero Oak — procedural, textured, multi-LOD deciduous tree for Blender & Bevy.

Blender (4.x):
    blender -b --python build_hero_oak.py -- --output ./hero_oak
Standalone (when Blender is unavailable, requires trimesh and Pillow):
    python build_hero_oak.py --output ./hero_oak

Generates self-contained GLB files (Z-up in Blender; Y-up in glTF), and a
Blender .blend project when executed in Blender. All assets are original.
"""
from __future__ import annotations

import argparse
import math
import os
import random
import struct
import sys
import zlib
from dataclasses import dataclass, field
from pathlib import Path

PI = math.pi
TAU = 2.0 * PI
DEFAULT_SEED = 17829


def vadd(a, b): return (a[0]+b[0], a[1]+b[1], a[2]+b[2])
def vsub(a, b): return (a[0]-b[0], a[1]-b[1], a[2]-b[2])
def vmul(a, k): return (a[0]*k, a[1]*k, a[2]*k)
def dot(a, b): return a[0]*b[0]+a[1]*b[1]+a[2]*b[2]
def cross(a, b): return (a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0])
def norm(a): return math.sqrt(dot(a,a))
def normalize(a): return vmul(a, 1.0/max(1e-9, norm(a)))
def lerp(a, b, t): return vadd(vmul(a, 1-t),vmul(b,t))
def clamp(x,a,b): return min(b,max(a,x))
def rand_unit(r):
    z=r.uniform(-1.0,1.0); p=r.uniform(0,TAU); u=math.sqrt(max(0,1-z*z))
    return (u*math.cos(p),u*math.sin(p),z)

def png_rgba(path:Path,w:int,h:int,pixels:bytes):
    """Write PNG with no non-Blender dependencies."""
    def chunk(t,data):
        return struct.pack('>I',len(data))+t+data+struct.pack('>I',zlib.crc32(t+data)&0xffffffff)
    body=b''.join(b'\0'+pixels[y*w*4:(y+1)*w*4] for y in range(h))
    path.write_bytes(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',w,h,8,6,0,0,0))+
                     chunk(b'IDAT',zlib.compress(body,9))+chunk(b'IEND',b''))

def hashf(x): return ((math.sin(x*74.786)+math.sin(x*12.389+1.17))*0.25+0.5)

def bark_texture(path:Path,n:int=384):
    pixels=bytearray(n*n*4)
    for y in range(n):
        for x in range(n):
            u=x/n; v=y/n
            # Vertical furrows curve around knotted bark rather than running straight.
            warp=0.022*math.sin(16*v+u*12)+0.014*math.sin(33*v+u*31)
            grain=(math.sin((u+warp)*190+math.sin(v*47)*1.8)*0.60+
                   math.sin((u+warp)*405+v*14)*0.19+
                   math.sin((u+warp)*62+v*30)*0.21)
            cracks=max(0.0, -grain-0.13)*0.65
            speckle=(hashf(x*0.17+y*0.71)-0.5)*0.19
            brightness=clamp(0.65+grain*0.16-cracks*0.60+speckle,0.24,1.06)
            i=(y*n+x)*4
            pixels[i]=int(clamp(106*brightness,0,255));pixels[i+1]=int(clamp(83*brightness,0,255))
            pixels[i+2]=int(clamp(61*brightness,0,255));pixels[i+3]=255
    png_rgba(path,n,n,bytes(pixels))

def leaf_texture(path:Path,n:int=384):
    """Leaf bouquet with independent pointed lobes, vein shading and clear holes.

    Most of the card is transparent; pairs of rotated cards make a visually
    volumetric crown without modelling thousands of micro-leaves.
    """
    rng=random.Random(93217)
    leaves=[]
    # Multiple overlapping layers: some backlit yellow-green leaflets,
    # deep-green shade leaflets, and tiny light flashes in the gaps.
    for k in range(28):
        cx=rng.uniform(-0.82,0.82); cy=rng.uniform(-0.78,0.78)
        length=rng.uniform(.24,.46); width=length*rng.uniform(.34,.58)
        angle=rng.uniform(0,TAU)
        tint=rng.uniform(.68,1.27)
        leaves.append((cx,cy,length,width,angle,tint))
    pixels=bytearray(n*n*4)
    for y in range(n):
        py=1-2*(y+0.5)/n
        for x in range(n):
            px=2*(x+0.5)/n-1
            color=None;alpha=0
            for cx,cy,length,width,angle,tint in leaves:
                dx=px-cx;dy=py-cy
                cs=math.cos(angle);ss=math.sin(angle)
                t=(dx*cs+dy*ss)/length
                s=(-dx*ss+dy*cs)/width
                if abs(t)>1:continue
                # Oak-like pointed ovate leaf, with small shallow serrations.
                edge=(1-abs(t)**2)**0.67 * (1+0.07*math.cos(t*18))
                if abs(s)>edge: continue
                vein=max(0,1-abs(s)*13)*.24
                rim=(abs(s)/max(.05,edge))*.10
                sun=0.06*math.sin((t+s)*19+cx*17)
                value=clamp(tint*(0.83+vein+sun-rim),.38,1.35)
                # Smooth alpha on outer 1-2 texels, enough for MSAA to help.
                alpha=int(255*clamp((edge-abs(s))*n*0.10,0.0,1.0))
                color=(int(clamp(65*value,0,255)),int(clamp(142*value,0,255)),int(clamp(49*value,0,255)))
            if color is not None:
                i=(y*n+x)*4;pixels[i:i+4]=bytes((*color,alpha))
    png_rgba(path,n,n,bytes(pixels))

def distant_texture(path:Path,n:int=384):
    """Colorful broad-canopy silhouette plus trunk for LOD3 cross-cards."""
    rng=random.Random(99171)
    lobes=[(rng.uniform(-0.76,.76),rng.uniform(-0.20,.74),rng.uniform(.16,.39)) for _ in range(65)]
    pixels=bytearray(n*n*4)
    for y in range(n):
        py=1-2*(y+.5)/n
        for x in range(n):
            px=2*(x+.5)/n-1
            color=None
            if abs(px)<(0.06+0.025*max(0,-py)) and py < 0.40:
                color=(90,68,47)
            for lx,ly,r in lobes:
                d=math.hypot((px-lx)*1.03, (py-ly)*1.20)
                if d<r:
                    t=0.66+0.22*math.sin((lx*9+ly*21)*3)+0.1*math.sin(px*45+py*23)
                    color=(int(clamp(37*t,0,255)),int(clamp(111*t,0,255)),int(clamp(36*t,0,255)))
            if color:
                i=(y*n+x)*4;pixels[i:i+4]=bytes((*color,255))
    png_rgba(path,n,n,bytes(pixels))

@dataclass
class MeshData:
    vertices:list=field(default_factory=list)
    triangles:list=field(default_factory=list)
    uvs:list=field(default_factory=list)

    def vertex(self,p,uv=(0.0,0.0)):
        idx=len(self.vertices);self.vertices.append(p);self.uvs.append(uv);return idx

    def quad(self, a,b,c,d,uvs=None):
        if uvs is None: uvs=((0,0),(1,0),(1,1),(0,1))
        ids=[self.vertex(p,uv) for p,uv in zip((a,b,c,d),uvs)]
        self.triangles.extend([(ids[0],ids[1],ids[2]),(ids[0],ids[2],ids[3])])

    def cylinder_path(self,positions,radii,sides=7,uv_repeat=3.0):
        if len(positions)<2:return
        old=len(self.vertices)
        total=0.0
        for j,(p,r) in enumerate(zip(positions,radii)):
            tangent=normalize(vsub(positions[min(j+1,len(positions)-1)],positions[max(j-1,0)]))
            ref=(1,0,0) if abs(tangent[0]) < .80 else (0,1,0)
            side=normalize(cross(tangent,ref));other=normalize(cross(side,tangent))
            if j:total+=norm(vsub(p,positions[j-1]))
            for k in range(sides+1):
                a=TAU*k/sides
                d=vadd(vmul(side,math.cos(a)),vmul(other,math.sin(a)))
                # Flared uneven bark gives organic variation at close range.
                uneven=1+0.065*math.sin(a*3+j*1.7)+0.035*math.sin(a*5-j*1.4)
                self.vertex(vadd(p,vmul(d,r*uneven)),(k/sides,total/uv_repeat))
        stride=sides+1
        for j in range(len(positions)-1):
            row=old+j*stride
            for k in range(sides):
                a=row+k;b=a+stride
                self.triangles.extend([(a,a+1,b),(a+1,b+1,b)])

    def leaf_card(self,center,half_width,half_height,axis,flip=False):
        sideways=normalize((math.cos(axis),math.sin(axis),0.0))
        up=(0,0,1)
        w=vmul(sideways,half_width);h=vmul(up,half_height)
        corners=(vsub(vsub(center,w),h),vadd(vsub(center,h),w),vadd(vadd(center,w),h),vadd(vsub(center,w),h))
        self.quad(*corners,uvs=((0,1),(1,1),(1,0),(0,0)))
        if flip: self.quad(corners[1],corners[0],corners[3],corners[2],uvs=((1,1),(0,1),(0,0),(1,0)))


def build_geometry(lod:int,seed:int):
    rng=random.Random(seed)
    wood=MeshData();foliage=MeshData()
    if lod==3:
        # A distant 4-triangle silhouette. GLB texture embeds trunk + crown.
        for a in (0.0,PI/2):
            foliage.leaf_card((0.0,0.0,10.85),10.6,10.85,a,flip=True)
        return wood,foliage

    height=16.6
    segments=({0:12,1:8,2:5})[lod]
    sides=({0:11,1:7,2:5})[lod]
    path=[];rads=[]
    for j in range(segments+1):
        t=j/segments
        x=0.23*math.sin(3.4*t+seed*.005)*t+0.38*t*t
        y=0.31*math.sin(4.7*t+1.5)*t
        z=height*t
        path.append((x,y,z))
        radius=0.67*(1-t)**.84+0.095
        if t<.16: radius*=1+2.2*(.16-t)**.8
        rads.append(radius)
    wood.cylinder_path(path,rads,sides)

    # Root buttresses bend along terrain and make the trunk feel anchored.
    if lod<=1:
        for j in range(7 if lod==0 else 5):
            a=TAU*j/(7 if lod==0 else 5)+rng.uniform(-.22,.22)
            d=(math.cos(a),math.sin(a),0)
            s=(0.2*d[0],0.2*d[1],.85)
            bend=(d[0]*.75,d[1]*.75,.21)
            end=(d[0]*rng.uniform(1.4,2.6),d[1]*rng.uniform(1.4,2.6),-.16)
            wood.cylinder_path([s,bend,end],[.32,.24,.035],max(4,sides-3))

    # Major branches spiral around the trunk, with asymmetric upward tropism.
    tiers={0:5,1:4,2:3}[lod]
    per_tier={0:5,1:4,2:3}[lod]
    level={0:3,1:2,2:1}[lod]
    leaf_locations=0

    def leaf_bouquet(pos,spread,count):
        nonlocal leaf_locations
        for _ in range(count):
            point=vadd(pos,vmul(rand_unit(rng),rng.uniform(.25,1.0)*spread))
            # Sparse leaf cards even at the crown interior. The alpha texture
            # itself contains ~20 painterly leaves and opaque partial overlap.
            half_w=rng.uniform(.75,1.35)*(1.0 if lod==0 else 1.35 if lod==1 else 2.4)
            half_h=half_w*rng.uniform(.68,1.05)
            a=rng.uniform(0,TAU)
            foliage.leaf_card(point,half_w,half_h,a,flip=False)
            if lod<=1: foliage.leaf_card(point,half_w*.88,half_h*.96,a+PI/2,flip=False)
            leaf_locations+=1

    def grow(start,direction,length,radius,depth,index):
        direction=normalize(direction)
        n={0:5,1:3,2:2}[lod] if depth>0 else 2
        pts=[start]
        for seg in range(1,n+1):
            t=seg/n
            wobble=(.14*math.sin(t*7.7+index*1.9),.16*math.sin(t*5.1+index*2.4),.10*t)
            p=vadd(start,vmul(direction,length*t))
            p=vadd(p,vmul(wobble,length*t*.35))
            pts.append(p)
        radii=[max(.018,radius*(1-(j/n)*.85)**1.5) for j in range(n+1)]
        if depth>=1 or lod==0: wood.cylinder_path(pts,radii,max(4,sides-depth))
        else: wood.cylinder_path(pts,radii,4)

        if depth==0:
            leaf_bouquet(pts[-1],length*.52+0.4,6 if lod==0 else 5 if lod==1 else 3)
            return

        # Additional child stems sprout sideways with upward curvature.
        child_count=2 if lod==0 else 2 if lod==1 else 2
        for child in range(child_count):
            t=rng.uniform(.52,.87)
            si=min(int(t*n),n-1)
            origin=lerp(pts[si],pts[si+1],t*n-si)
            angle=rng.uniform(0,TAU)
            outward=(math.cos(angle),math.sin(angle),rng.uniform(.18,.56))
            axis=normalize(vadd(vmul(direction,.53),vmul(outward,.47)))
            grow(origin,axis,length*rng.uniform(.44,.71),radius*.52,depth-1,index*5+child+1)
        if depth<=1:
            leaf_bouquet(pts[-1],length*.40+.3,3 if lod==0 else 2)

    for tier in range(tiers):
        t=.31+.54*(tier+0.5)/tiers
        anchor_index=min(int(t*segments),segments-1)
        anchor=lerp(path[anchor_index],path[anchor_index+1],t*segments-anchor_index)
        for k in range(per_tier):
            phi=TAU*(k/per_tier+t*.47)+rng.uniform(-.23,.23)
            spread=rng.uniform(.87,1.19)
            length=(7.4-2.25*t)*spread
            z_gain=.27+.40*t+rng.uniform(-.13,.16)
            d=normalize((math.cos(phi),math.sin(phi),z_gain))
            grow(anchor,d,length,.24*(1.0-t*.50),level,k+tier*13)

    # Off-center secondary crown shoots create asymmetry, avoiding identical balls.
    if lod==0:
        for q in range(7):
            t=rng.uniform(.63,.98)
            pos=lerp(path[min(int(t*segments),segments-1)],path[min(int(t*segments),segments-1)+1],(t*segments)%1)
            angle=rng.uniform(0,TAU)
            grow(pos,(math.cos(angle)*.55,math.sin(angle)*.55,.75),rng.uniform(2.4,4.0),.08,1,300+q)
    return wood,foliage


def blender_mesh(name,data,material):
    import bpy
    mesh=bpy.data.meshes.new(name)
    mesh.from_pydata(data.vertices,[],data.triangles)
    mesh.update()
    uv_layer=mesh.uv_layers.new(name='UVMap')
    for face in mesh.polygons:
        for loop_id in face.loop_indices:
            i=mesh.loops[loop_id].vertex_index
            uv_layer.data[loop_id].uv=data.uvs[i]
        face.use_smooth=True
    mesh.materials.append(material)
    obj=bpy.data.objects.new(name,mesh)
    bpy.context.collection.objects.link(obj)
    return obj


def blender_material(name,texture,roughness,leaves=False):
    import bpy
    material=bpy.data.materials.new(name)
    material.use_nodes=True
    nodes=material.node_tree.nodes
    shader=nodes.get('Principled BSDF')
    shader.inputs['Roughness'].default_value=roughness
    shader.inputs['Metallic'].default_value=0.0
    image_node=nodes.new('ShaderNodeTexImage')
    image_node.image=bpy.data.images.load(str(texture),check_existing=True)
    material.node_tree.links.new(image_node.outputs['Color'],shader.inputs['Base Color'])
    if leaves:
        material.node_tree.links.new(image_node.outputs['Alpha'],shader.inputs['Alpha'])
        material.use_backface_culling=False
        if hasattr(material,'surface_render_method'):
            material.surface_render_method='DITHERED'
        elif hasattr(material,'blend_method'):
            material.blend_method='CLIP'
            material.alpha_threshold=.46
    return material


def export_blender(output:Path,seed:int):
    import bpy
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.object.delete(use_global=False)
    bark=blender_material('Aged_Oak_Bark',output/'bark.png',.98)
    leaves=blender_material('Sunlit_Oak_Foliage',output/'leaves.png',.79,True)
    silhouette=blender_material('Oak_Distant_Silhouette',output/'impostor.png',.90,True)
    for lod in range(4):
        wood,foliage=build_geometry(lod,seed)
        objects=[]
        if wood.vertices:objects.append(blender_mesh('oak_lod%d_wood'%lod,wood,bark))
        objects.append(blender_mesh('oak_lod%d_leaves'%lod,foliage,silhouette if lod==3 else leaves))
        bpy.ops.object.select_all(action='DESELECT')
        for obj in objects: obj.select_set(True)
        bpy.context.view_layer.objects.active=objects[0]
        bpy.ops.export_scene.gltf(filepath=str(output/f'oak_lod{lod}.glb'),export_format='GLB',
                                  use_selection=True,export_yup=True,export_animations=False,
                                  export_materials='EXPORT',export_texcoords=True)
        for obj in objects: obj.hide_set(lod!=0)
        print(f'LOD{lod}: {sum(len(d.data.polygons) for d in objects)} triangles')
    bpy.ops.wm.save_as_mainfile(filepath=str(output/'hero_oak.blend'))


def export_trimesh(output:Path,seed:int):
    """Produces the same generated geometry and a usable GLB without Blender.
    This fallback deliberately does not produce an editable .blend file.
    """
    import numpy as np
    import trimesh
    from PIL import Image
    from trimesh.visual.texture import TextureVisuals
    from trimesh.visual.material import PBRMaterial

    textures=[Image.open(output/f).convert('RGBA') for f in ('bark.png','leaves.png','impostor.png')]
    materials=[PBRMaterial(name='Aged_Oak_Bark',baseColorTexture=textures[0],roughnessFactor=.98,metallicFactor=0),
               PBRMaterial(name='Sunlit_Oak_Foliage',baseColorTexture=textures[1],roughnessFactor=.80,
                           metallicFactor=0,alphaMode='MASK',alphaCutoff=.48,doubleSided=True),
               PBRMaterial(name='Oak_Distant_Silhouette',baseColorTexture=textures[2],roughnessFactor=.95,
                           metallicFactor=0,alphaMode='MASK',alphaCutoff=.48,doubleSided=True)]
    def mesh(data,material):
        # Blender Z-up -> glTF Y-up, same physical rotation as Blender exporter.
        coords=np.asarray(data.vertices,dtype=np.float32)
        coords=np.stack([coords[:,0],coords[:,2],-coords[:,1]],axis=1)
        faces=np.asarray(data.triangles,dtype=np.int64)
        uvs=np.asarray(data.uvs,dtype=np.float32)
        return trimesh.Trimesh(vertices=coords,faces=faces,process=False,
                        visual=TextureVisuals(uv=uvs,material=material))
    for lod in range(4):
        wood,foliage=build_geometry(lod,seed)
        scene=trimesh.Scene()
        if wood.vertices: scene.add_geometry(mesh(wood,materials[0]),node_name=f'wood_lod{lod}')
        scene.add_geometry(mesh(foliage,materials[2 if lod==3 else 1]),node_name=f'leaves_lod{lod}')
        dst=output/f'oak_lod{lod}.glb'
        scene.export(dst)
        print(f'LOD{lod}: {len(wood.triangles)+len(foliage.triangles):>6} tris, {dst.stat().st_size/1024:.0f} KiB')


def main():
    args=sys.argv[sys.argv.index('--')+1:] if '--' in sys.argv else sys.argv[1:]
    parser=argparse.ArgumentParser()
    parser.add_argument('--output',default='hero_oak',help='Output directory')
    parser.add_argument('--seed',type=int,default=DEFAULT_SEED)
    opts=parser.parse_args(args)
    out=Path(opts.output).expanduser().resolve();out.mkdir(parents=True,exist_ok=True)
    bark_texture(out/'bark.png')
    leaf_texture(out/'leaves.png')
    distant_texture(out/'impostor.png')
    try:
        import bpy
    except ImportError:
        export_trimesh(out,opts.seed)
    else:
        export_blender(out,opts.seed)

if __name__=='__main__': main()