#!/usr/bin/env python3
"""Deterministic, botanical procedural woodland fern for Bevy (Y-up GLBs).

Dependencies: numpy, trimesh, Pillow.  Run: python build_woodland_fern.py --output ./woodland_fern
No Blender needed to export.  create_blend.py imports GLBs into Blender for editing.
"""
import argparse
import math
import random
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import trimesh
from PIL import Image, ImageDraw, ImageFont
from trimesh.visual.material import PBRMaterial

UP=np.array([0.0,1.0,0.0]); X=np.array([1.0,0.0,0.0]); Z=np.array([0.0,0.0,1.0])
SEED=80327


def norm(v):
    d=np.linalg.norm(v)
    return v/d if d>1e-10 else v


def mix(a,b,t):return a*(1-t)+b*t


def smoothstep(a,b,x):
    t=max(0.,min(1.,(x-a)/(b-a)))
    return t*t*(3-2*t)

class MeshBuffer:
    def __init__(self):
        self.vertices=[];self.faces=[];self.colors=[]
    def vertex(self,p,color):
        i=len(self.vertices);self.vertices.append(np.asarray(p,float));self.colors.append(tuple(int(max(0,min(255,x))) for x in color));return i
    def tri(self,a,b,c):
        self.faces.append((a,b,c))
    def quad(self,a,b,c,d):
        self.faces.extend([(a,b,c),(a,c,d)])
    def shape(self):
        if not self.faces:return None
        mesh=trimesh.Trimesh(vertices=np.asarray(self.vertices),faces=np.asarray(self.faces),process=False)
        mesh.visual.vertex_colors=np.array(self.colors,dtype=np.uint8)
        return mesh
    @property
    def count(self):return len(self.faces)

@dataclass
class Frond:
    angle:float
    length:float
    phase:float
    lean:float
    leafiness:float
    curl:float

    def point(self,u):
        u=max(0.,min(1.,u))
        d=np.array([math.cos(self.angle),0,math.sin(self.angle)])
        cross=np.array([-d[2],0,d[0]])
        r=self.length*(0.81*u**1.14 + .025*u)
        h=0.012 + self.length*(self.lean*math.sin(math.pi * .91 * u**.88) + 0.07*u-0.025*u*u)
        wiggle= .028*self.length*math.sin(u*math.pi*1.6+self.phase)*u*u
        return d*r + cross*wiggle + UP*h
    def tangent(self,u):return norm(self.point(min(.9999,u+.002))-self.point(max(.0001,u-.002)))


def fronds_for_seed(seed):
    rng=random.Random(seed)
    specs=[]
    # Spiral arrangement rather than mirrored spokes; outer leaves arch low, young leaves stand taller.
    for j in range(14):
        angle=j*math.pi*(3-math.sqrt(5)) + rng.uniform(-.14,.14)
        if j<8:
            length=rng.uniform(.87,1.12)
            lean=rng.uniform(.53,.64)
        elif j<12:
            length=rng.uniform(.65,.89)
            lean=rng.uniform(.64,.78)
        else:
            length=rng.uniform(.37,.56)
            lean=rng.uniform(.76,.88)
        specs.append(Frond(angle,length,rng.uniform(0,2*math.pi),lean,rng.uniform(.88,1.12),rng.uniform(-1.,1.)))
    return specs


def color_leaf(rng,frond_idx,pinna_idx,s,t):
    # slightly warmer new growth, desaturated mature fronds, individual variation
    tone=rng.uniform(.85,1.13)
    young=frond_idx>=12
    greens=np.array([53,113,55] if not young else [99,149,67],float)
    greens*=tone
    shade=(.90 + 0.18*s) * (.92 + 0.14*(1-t))
    c=greens*shade
    return [int(np.clip(v,0,255)) for v in c]+[255]


def stem_tube(mesh,points,radii,col,rings=5):
    frames=[]
    for i,p in enumerate(points):
        tangent=norm(points[min(i+1,len(points)-1)]-points[max(i-1,0)])
        # stable frame approximated by world sideways
        axis=norm(np.cross(tangent,UP))
        if np.linalg.norm(axis)<.01:axis=X
        second=norm(np.cross(tangent,axis))
        frames.append((axis,second))
    ridx=[]
    for i,p in enumerate(points):
        axis,second=frames[i];row=[]
        for j in range(rings):
            a=2*math.pi*j/rings
            clr=[int(v*(0.82+0.14*math.sin(a)**2)) for v in col[:3]]+[255]
            row.append(mesh.vertex(p+radii[i]*(math.cos(a)*axis+math.sin(a)*second),clr))
        ridx.append(row)
    for i in range(len(ridx)-1):
        for j in range(rings):
            n=(j+1)%rings
            mesh.quad(ridx[i][j],ridx[i][n],ridx[i+1][n],ridx[i+1][j])


def pinna_pos(frond,u,side,v,pinna_len):
    c=frond.point(u)
    tangent=frond.tangent(u)
    lateral=norm(np.cross(UP,tangent))
    # Side branches turn slightly toward frond tip; arch down at their ends.
    direction=norm(lateral*side*0.93 + tangent*.27)
    cu=c + direction*(pinna_len*v)
    return cu + UP*(pinna_len*(.14*math.sin(math.pi*v)-.14*v*v))


def pinna_length(frond,u,jitter):
    x=(u-.065)/.92
    if x<=0 or x>=1:return 0
    env=max(0,math.sin(math.pi*x))**.85
    return frond.length*(.207*env*(1-.12*u)*frond.leafiness*jitter)


def add_leaflet(leaf_mesh,origin,direction,wing,width,length,color,rng,detail=2):
    """Lightweight 4-triangle curved pinnule with a colored central vein."""
    direction=norm(direction); wing=norm(wing)
    near=origin
    center=origin + direction*(length*.49) + UP*.002
    center += wing*(.0015*rng.uniform(.6,1.2))
    tip=origin+direction*length + UP*(-.004*length/.045)
    w=width*rng.uniform(.90,1.07)
    left=center-wing*w+UP*.0005
    right=center+wing*w+UP*.0005
    dark=[int(x*.83) for x in color[:3]]+[255]
    pale=[int(x*.96) for x in color[:3]]+[255]
    start=leaf_mesh.vertex(near,color)
    mid=leaf_mesh.vertex(center+UP*.0018,color)
    l=leaf_mesh.vertex(left,dark)
    r=leaf_mesh.vertex(right,pale)
    end=leaf_mesh.vertex(tip,pale)
    leaf_mesh.tri(start,l,mid)
    leaf_mesh.tri(start,mid,r)
    leaf_mesh.tri(l,end,mid)
    leaf_mesh.tri(mid,end,r)


def build(level=0,seed=SEED):
    rng=random.Random(seed)
    wood=MeshBuffer();foliage=MeshBuffer()
    settings={
        0:dict(fronds=14,pairs=20,pinnules=6,micro=True,stem_sections=24,tube_rings=5),
        1:dict(fronds=11,pairs=12,pinnules=0,micro=False,stem_sections=15,tube_rings=4),
        2:dict(fronds=8,pairs=7,pinnules=0,micro=False,stem_sections=8,tube_rings=3),
    }[level]
    cfg=settings
    specs=fronds_for_seed(seed)[:cfg['fronds']]
    # Low, asymmetric stems forming a common crown
    for fi,frond in enumerate(specs):
        N=cfg['stem_sections'];p=[frond.point(k/N) for k in range(N+1)]
        stem_tube(wood,p,[mix(.0055,.0012,k/N) for k in range(N+1)], [88,117,55,255],cfg['tube_rings'])
        count=cfg['pairs']
        for j in range(count):
            u=.075 + .875*(j+.6)/count
            pinna_width_jitter=rng.uniform(.87,1.13)
            for side in (-1,1):
                stagger=.013/count*side + rng.uniform(-.003,.003)
                uu=max(.09,min(.975,u+stagger))
                plen=pinna_length(frond,uu,pinna_width_jitter*rng.uniform(.93,1.07))
                if plen<.012:continue
                green=color_leaf(rng,fi,j,uu,0.25)
                if cfg['micro']:
                    # Slim visible pinna midrib, growing from central rachis.
                    nodes=[pinna_pos(frond,uu,side,t,plen) for t in (0, .4, .75,1.)]
                    central=norm(frond.tangent(uu))
                    for k in range(len(nodes)-1):
                        p0=nodes[k];p1=nodes[k+1]
                        sidevec=norm(np.cross(central,p1-p0))*.0010*(1-k/len(nodes))
                        a=wood.vertex(p0-sidevec,[82,115,49,255]);b=wood.vertex(p0+sidevec,[101,135,61,255])
                        c=wood.vertex(p1+sidevec*.72,[99,133,61,255]);d=wood.vertex(p1-sidevec*.72,[80,110,46,255])
                        wood.quad(a,b,c,d)
                    # Twice-pinnate: each side branch has many paired small pinnules.
                    n=cfg['pinnules']
                    lateral=norm(np.cross(UP,frond.tangent(uu)))
                    branch_axis=norm(lateral*side*.92+frond.tangent(uu)*.25)
                    leafwing=norm(np.cross(UP,branch_axis))
                    for g in range(n):
                        tt=.10+(.79*g/max(1,n-1))
                        len_env=max(0,math.sin(math.pi*(tt**.79)))**.83
                        for sign in (-1,1):
                            t=max(.07, min(.96,tt+sign*.015))
                            origin=pinna_pos(frond,uu,side,t,plen)
                            len_p=plen*(.23*len_env)*rng.uniform(.86,1.09)
                            # little feathery forward-angled leaf blade
                            heading=norm(leafwing*sign*.85+branch_axis*.52)
                            heading += UP*rng.uniform(-.09,.08)
                            add_leaflet(foliage,origin,heading,branch_axis, len_p*.28,len_p,
                                        green,rng,detail=2)
                else:
                    # Contiguous feather-shaped pinna. Sawtooth lobes mimic distant leaflets.
                    tangent=frond.tangent(uu)
                    lateral=norm(np.cross(UP,tangent))
                    direction=norm(lateral*side*.93 + tangent*.27)
                    across=norm(np.cross(direction,UP))
                    segments=4 if level==1 else 2
                    # Single pinna silhouette with many serrations; 2-sided material.
                    row=[]
                    blade_w=plen*(.165 if level==1 else .185)
                    for k in range(segments+1):
                        t=k/segments
                        center=pinna_pos(frond,uu,side,t,plen)
                        env=max(0,math.sin(math.pi*t))**.73
                        serrated= .70 if k%3==0 and k>0 else 1.0
                        w=blade_w*env*serrated
                        c=[int(green[h]*(1-.11*t)) for h in range(3)]+[255]
                        L=foliage.vertex(center-across*w, [int(v*.85) for v in c[:3]]+[255]);
                        M=foliage.vertex(center+UP*.0022,c)
                        R=foliage.vertex(center+across*w,[int(v*.95) for v in c[:3]]+[255]);
                        row.append((L,M,R))
                    for k in range(segments):
                        a,b,c=row[k];d,e,f=row[k+1]
                        foliage.quad(a,d,e,b);foliage.quad(b,e,f,c)
    return wood.shape(),foliage.shape()


def pbr(name,double_sided=True):
    m=PBRMaterial(name=name,baseColorFactor=[255,255,255,255],metallicFactor=0.,roughnessFactor=.85,doubleSided=double_sided)
    return m


def export_glb(path,wood,foliage):
    scene=trimesh.Scene()
    for mesh,name in [(wood,'fern_rachis'),(foliage,'fern_fronds')]:
        if mesh is None:continue
        mesh.visual.material=pbr(name)
        scene.add_geometry(mesh,node_name=name,geom_name=name)
    data=trimesh.exchange.gltf.export_glb(scene,include_normals=True)
    path.write_bytes(data)
    return len(wood.faces if wood is not None else [])+len(foliage.faces if foliage is not None else [])


def render_meshes(meshes,path,width=900,height=900,azi=45,elev=20,background=(235,239,225,255),transparent=False):
    """Very small orthographic painter for package previews and far billboard sprites."""
    # Three-dimensional camera transform. Screen right points roughly +X for azimuth 0.
    az=math.radians(azi);el=math.radians(elev)
    look=np.array([math.sin(az)*math.cos(el), math.sin(el),math.cos(az)*math.cos(el)])
    right=norm(np.cross(UP,look));camera_up=norm(np.cross(look,right))
    verts=[];tris=[];cols=[]
    for mesh in meshes:
        if mesh is None:continue
        pts=np.asarray(mesh.vertices);faces=np.asarray(mesh.faces)
        rgba=np.asarray(mesh.visual.vertex_colors)
        # approximate light from upper-left / front direction
        flat=np.mean(rgba[faces][:,:,:3],axis=1)
        a=pts[faces[:,1]]-pts[faces[:,0]];b=pts[faces[:,2]]-pts[faces[:,0]]
        normal=np.cross(a,b);normal=normal/np.maximum(np.linalg.norm(normal,axis=1)[:,None],1e-6)
        sun=norm(np.array([.3,1.0,.7]))
        diffuse=.70 + .30*np.abs(normal@sun)
        col=np.clip(flat*diffuse[:,None],0,255).astype('uint8')
        verts.append(pts);tris.append(faces);cols.append(col)
    verts=np.concatenate(verts);tris=np.concatenate(tris);colors=np.concatenate(cols)
    if len(meshes)==2 and meshes[0] is not None:
        first=len(meshes[0].vertices);tris[len(meshes[0].faces):]+=first
    # center consistent across preview or sprite
    center=np.array([0,.49,0]);scale= (min(width,height)/2)*.82/1.07
    screen=np.stack([(verts-center)@right*scale+width/2, -(verts-center)@camera_up*scale+height/2],axis=-1)
    depth=(verts-center)@look
    z=np.mean(depth[tris],axis=1)
    order=np.argsort(z)
    im=Image.new('RGBA',(width,height),(0,0,0,0) if transparent else background)
    draw=ImageDraw.Draw(im,'RGBA')
    # render distant faces first, near faces last
    for faceid in order:
        p=screen[tris[faceid]]
        if np.max(p[:,0]) < -1 or np.min(p[:,0])>width+1 or np.max(p[:,1])<-1 or np.min(p[:,1])>height+1:continue
        points=[tuple(v) for v in p]
        r,g,b=colors[faceid]
        draw.polygon(points, fill=(int(r),int(g),int(b),255))
    im.save(path)
    return im


def normalize_impostor(raw_path,target_path):
    im=Image.open(raw_path).convert('RGBA')
    bbox=im.getbbox()
    if bbox is None:raise RuntimeError('Transparent impostor render')
    crop=im.crop(bbox)
    canvas=Image.new('RGBA',(1024,512),(0,0,0,0))
    # Preserve aspect ratio with a margin: actual geometry will occupy most of its quad.
    scale=min(940/crop.width,444/crop.height)
    new=(max(1,int(crop.width*scale)),max(1,int(crop.height*scale)))
    crop=crop.resize(new,Image.Resampling.LANCZOS)
    canvas.alpha_composite(crop,((1024-new[0])//2,(512-new[1])//2))
    canvas.save(target_path)


def create_showcase(meshes,path):
    from PIL import ImageFilter
    tmp=path.with_name('_render_transparent.png')
    render_meshes(meshes,tmp,width=1150,height=950,azi=35,elev=21,transparent=True)
    im=Image.open(tmp).convert('RGBA'); bbox=im.getbbox()
    crop=im.crop(bbox)
    W,H=1280,870
    arr=np.zeros((H,W,4),dtype=np.uint8)
    for y in range(H):
        t=y/max(1,H-1)
        arr[y,:,0]=int(20+27*t)
        arr[y,:,1]=int(38+32*t)
        arr[y,:,2]=int(31+24*t)
        arr[y,:,3]=255
    base=Image.fromarray(arr,'RGBA')
    # soft ground shadow, added only to the preview (not the game asset)
    sh=Image.new('RGBA',(W,H),(0,0,0,0));dr=ImageDraw.Draw(sh)
    dr.ellipse((225,697,1065,789),fill=(0,0,0,140))
    sh=sh.filter(ImageFilter.GaussianBlur(22));base=Image.alpha_composite(base,sh)
    scale=min(1060/crop.width,670/crop.height)
    new=(round(crop.width*scale),round(crop.height*scale))
    crop=crop.resize(new,Image.Resampling.LANCZOS)
    base.alpha_composite(crop,((W-new[0])//2,110+(660-new[1])//2))
    txt=ImageDraw.Draw(base)
    try:
        large=ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',25)
        small=ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',16)
    except OSError:large=ImageFont.load_default();small=large
    txt.text((50,45),'WOODLAND FERN',font=large,fill=(230,245,208))
    txt.text((52,85),'Procedural 3D vegetation  /  4 LODs  /  Bevy-ready GLB',font=small,fill=(163,197,162))
    txt.text((52,817),'1.7 m across   •   0.7 m high   •   33.6k / 5.5k / 1.3k / 4 tris',font=small,fill=(174,203,174))
    base.convert('RGB').save(path,quality=93)
    tmp.unlink(missing_ok=True)


def make_billboard(png_path,outpath):
    img=Image.open(png_path).convert('RGBA')
    # Height 1.3 m, width 2.14 m. Two planes perpendicular.
    # For better low-cost readability on both axes use two crossing cards, alpha mask.
    xw=.95;h=.88
    uv=np.array([[0,0],[1,0],[1,1],[0,1]],dtype=float)
    scene=trimesh.Scene()
    for i,angle in enumerate([0,math.pi/2]):
        d=np.array([math.cos(angle),0,math.sin(angle)])
        p=[(-d*xw)+UP*.0, d*xw+UP*.0,d*xw+UP*h,-d*xw+UP*h]
        mesh=trimesh.Trimesh(vertices=p,faces=[[0,1,2],[0,2,3]],process=False)
        # UV top/bottom in glTF; flip since PIL has top-left origin
        mesh.visual=trimesh.visual.TextureVisuals(uv=np.array([[0,1],[1,1],[1,0],[0,0]]),material=PBRMaterial(name='fern_impostor',baseColorTexture=img,metallicFactor=0.,roughnessFactor=.92,alphaMode='MASK',alphaCutoff=.45,doubleSided=True))
        scene.add_geometry(mesh,node_name='fern_billboard_'+str(i))
    outpath.write_bytes(trimesh.exchange.gltf.export_glb(scene,include_normals=True))
    return 4


def main():
    ap=argparse.ArgumentParser()
    ap.add_argument('--output',default='woodland_fern')
    ap.add_argument('--seed',type=int,default=SEED)
    args=ap.parse_args()
    out=Path(args.output).resolve();out.mkdir(parents=True,exist_ok=True)
    lods={}
    for i in range(3):
        wood,foliage=build(i,args.seed)
        glb=out/f'woodland_fern_lod{i}.glb'
        n=export_glb(glb,wood,foliage)
        lods[i]=(wood,foliage)
        print(f'LOD{i}: {n:,} triangles, {glb.stat().st_size/1024:.0f} KiB')
    # Main preview render from slightly elevated viewpoint.
    render_meshes(lods[0],out/'preview.png',width=1100,height=1100,azi=35,elev=23)
    # Impostor from straight-on view, transparent background.
    render_meshes(lods[0],out/'_raw_impostor.png',width=768,height=768,azi=0,elev=0,transparent=True)
    normalize_impostor(out/'_raw_impostor.png',out/'impostor.png')
    (out/'_raw_impostor.png').unlink(missing_ok=True)
    create_showcase(lods[0],out/'showcase.png')
    n=make_billboard(out/'impostor.png',out/'woodland_fern_lod3.glb')
    print(f'LOD3: {n} triangles, { (out/"woodland_fern_lod3.glb").stat().st_size/1024:.0f} KiB')
    print('Saved',out)

if __name__=='__main__':main()
