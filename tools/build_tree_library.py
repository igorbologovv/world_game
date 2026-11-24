"""Build original tree geometry, foliage PBR maps and forest-compatible GLBs."""

import argparse
from dataclasses import dataclass
import json
import math
from pathlib import Path
import random
import struct
import zlib

import numpy as np
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "assets/models/vegetation/library"
RUST = ROOT / "crates/world-vegetation/src/forest/tree_catalog.rs"
TAU = math.tau
UP = np.array((0., 1., 0.))


@dataclass(frozen=True)
class Tree:
    slug: str
    name: str
    biome: str
    form: str
    height: float
    width: float  # crown radius divided by height
    stem: float  # ratio of trunk radius to height
    start: float
    spread: float  # branch rise as a fraction of its length
    levels: int
    color: tuple
    bark: str
    snow: float = 0.0


TREES = (
    Tree("norway-spruce", "Norway spruce", "Pine", "spire", 20, .242, .014, .11, -.32, 11, (54, 91, 57), "pine_bark"),
    Tree("european-beech", "European beech", "Oak", "oval", 16, .395, .029, .25, .56, 8, (83, 122, 47), "japanese_sycamore"),
    Tree("umbrella-thorn", "Umbrella thorn", "Acacia", "umbrella", 10.8, .56, .034, .40, .76, 6, (107, 126, 57), "bark_brown_02"),
    Tree("english-oak", "English oak", "Oak", "widespread", 17, .59, .042, .26, .54, 7, (80, 111, 46), "bark_brown_02"),
    Tree("silver-birch", "Silver birch", "Oak", "vase", 17, .29, .018, .31, .84, 7, (105, 143, 65), "japanese_sycamore"),
    Tree("sugar-maple", "Sugar maple", "Oak", "oval", 19, .45, .032, .28, .32, 8, (91, 139, 48), "bark_brown_02"),
    Tree("weeping-willow", "Weeping willow", "Oak", "weeping", 14, .51, .043, .33, .45, 7, (104, 130, 69), "bark_brown_02"),
    Tree("quaking-aspen", "Quaking aspen", "Oak", "vase", 18, .25, .015, .39, .98, 7, (103, 143, 64), "japanese_sycamore"),
    Tree("american-sycamore", "American sycamore", "Oak", "widespread", 21, .48, .042, .33, .66, 7, (83, 126, 50), "japanese_sycamore"),
    Tree("scots-pine", "Scots pine", "Pine", "cedar", 18, .33, .022, .50, .25, 9, (66, 98, 57), "pine_bark"),
    Tree("douglas-fir", "Douglas fir", "Pine", "spire", 25, .22, .020, .26, -.13, 10, (53, 83, 52), "pine_bark"),
    Tree("western-red-cedar", "Western red cedar", "Pine", "droop", 23, .30, .038, .12, -.24, 10, (64, 100, 49), "bark_brown_02"),
    Tree("colorado-blue-spruce", "Colorado blue spruce", "Pine", "column", 17, .23, .018, .14, -.26, 11, (70, 112, 103), "pine_bark"),
    Tree("winter-white-spruce", "Snow-covered white spruce", "Pine", "spire", 17, .255, .016, .11, -.26, 11, (60, 90, 62), "pine_bark", .88),
    Tree("winter-stone-pine", "Snow-covered stone pine", "Pine", "tiered", 13, .41, .028, .29, .26, 8, (67, 99, 59), "pine_bark", .70),
    Tree("winter-subalpine-fir", "Snow-covered subalpine fir", "Pine", "column", 20, .145, .016, .15, .17, 11, (60, 88, 69), "pine_bark", .94),
    Tree("winter-siberian-larch", "Snow-covered Siberian larch", "Pine", "droop", 18, .29, .019, .25, -.34, 9, (82, 102, 66), "pine_bark", .57),
    Tree("fever-tree", "Fever tree", "Acacia", "vase", 15, .46, .029, .37, .79, 7, (110, 143, 66), "japanese_sycamore"),
    Tree("desert-ironwood", "Desert ironwood", "Acacia", "oval", 8, .57, .044, .20, .42, 6, (92, 112, 76), "bark_brown_02"),
    Tree("joshua-tree", "Joshua tree", "Acacia", "yucca", 6.8, .43, .048, .24, .9, 6, (116, 134, 67), "palm_tree_bark"),
    Tree("ghaf", "Ghaf", "Acacia", "weeping", 12, .50, .039, .25, .47, 7, (111, 124, 72), "bark_brown_02"),
    Tree("kapok", "Kapok", "Jungle", "kapok", 27, .37, .038, .53, .54, 8, (75, 116, 46), "bark_brown_02"),
    Tree("bigleaf-mahogany", "Bigleaf mahogany", "Jungle", "oval", 24, .43, .028, .33, .51, 8, (65, 100, 44), "bark_brown_02"),
    Tree("rubber-fig", "Rubber fig", "Jungle", "fig", 19, .58, .058, .24, .52, 7, (58, 115, 57), "bark_brown_02"),
    Tree("red-mangrove", "Red mangrove", "Jungle", "mangrove", 11, .44, .033, .34, .47, 7, (74, 121, 62), "bark_brown_02"),
    Tree("breadfruit", "Breadfruit", "Jungle", "oval", 15, .50, .036, .23, .29, 8, (68, 124, 42), "japanese_sycamore"),
    Tree("silver-tree-fern", "Silver tree fern", "Jungle", "pinnate", 9, .39, .035, .75, -.25, 12, (88, 131, 63), "palm_tree_bark"),
    Tree("date-palm", "Date palm", "Palm", "pinnate", 15, .295, .029, .65, .35, 23, (80, 118, 71), "palm_tree_bark"),
    Tree("mexican-fan-palm", "Mexican fan palm", "Palm", "fan", 20, .22, .018, .75, -.3, 19, (98, 130, 56), "palm_tree_bark"),
    Tree("canary-island-palm", "Canary Island date palm", "Palm", "pinnate", 12, .51, .065, .70, .24, 30, (74, 113, 42), "palm_tree_bark"),
)


def unit(value):
    return value / max(np.linalg.norm(value), 1e-9)


def horizontal(angle):
    return np.array((math.cos(angle), 0., math.sin(angle)))


def perpendicular(axis):
    return unit(np.cross(axis, UP if abs(axis[1]) < .9 else np.array((1., 0., 0.))))


class Mesh:
    def __init__(self):
        self.positions, self.normals, self.uv, self.indices = [], [], [], []

    def vertex(self, p, n, uv):
        self.positions.append(p)
        self.normals.append(n)
        self.uv.append(uv)
        return len(self.positions) - 1

    def tube(self, points, radii, sides, rough):
        base = len(self.positions)
        length = 0.0
        right = None
        for row, (center, radius) in enumerate(zip(points, radii)):
            axis = unit(points[min(row + 1, len(points) - 1)] - points[max(row - 1, 0)])
            right = perpendicular(axis) if right is None else unit(right - axis * np.dot(right, axis))
            across = np.cross(axis, right)
            if row:
                length += np.linalg.norm(center - points[row - 1])
            for side in range(sides + 1):
                theta = TAU * (side % sides) / sides
                normal = math.cos(theta) * right + math.sin(theta) * across
                rib = 1 + rough * (math.sin(theta * 3 + row * .7) + .45 * math.cos(theta * 5))
                self.vertex(center + normal * max(.002, radius * rib), normal, (side / sides, length * .67))
        for row in range(len(points) - 1):
            for side in range(sides):
                a = base + row * (sides + 1) + side
                b = a + sides + 1
                self.indices.extend((a, a + 1, b, a + 1, b + 1, b))

    def spray(self, center, direction, length, width, tile, tilt, lod):
        axis = unit(direction)
        right = perpendicular(axis)
        normal = np.cross(right, axis)
        right = unit(right * math.cos(tilt) + normal * math.sin(tilt))
        normal = np.cross(right, axis)
        front, back = center + axis * length * .5, center - axis * length * .5
        u, v = (tile % 2) * .5, (tile // 2) * .5
        points = (back - right * width, back + right * width, front + right * width, front - right * width)
        coords = [(u+.008, v+.492), (u+.492, v+.492), (u+.492, v+.008), (u+.008, v+.008)]
        start = len(self.positions)
        for p, tex in zip(points, coords):
            n = unit(normal * .85 + unit(p - center) * .33)
            self.vertex(p, n, tex)
        if lod == 0:
            self.vertex(center + normal * (width * .24), normal, (u+.25, v+.25))
            self.indices.extend(start + j for j in (0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4))
        else:
            self.indices.extend(start + j for j in (0, 1, 2, 0, 2, 3))


def skeleton(tree, variant):
    pilots = {"norway-spruce", "european-beech", "umbrella-thorn"}
    seed = 0 if tree.slug in pilots else zlib.crc32(tree.slug.encode())
    r = random.Random(12351 + variant * 8839 + seed)
    uniform = r.uniform
    h = tree.height * (1 + .047 * variant)
    width = tree.width * h
    trunk_radius = tree.stem * h
    conifer = tree.form in {"spire", "column", "tiered", "droop", "cedar"}
    palm = tree.form in {"pinnate", "fan"}
    flat = tree.form in {"umbrella", "widespread", "kapok", "fig"}
    bend = horizontal(uniform(0, TAU)) * h * uniform(.017, .065 if conifer else .13)
    wave = horizontal(uniform(0, TAU)) * h * uniform(.015, .048)

    def trunk(t):
        return UP * (h * (t if not flat else t*.82) - .28) + bend * t**1.6 + wave * math.sin(t * 4.8) * max(t-.09,0)

    branches = [(0, [trunk(i / 14) for i in range(15)],
                 [trunk_radius * (1 - i / 14)**.82 * (1 + .46 * (1 - i / 14)**20) + .004 for i in range(15)])]
    if flat:
        order, points, radii = branches[0]
        # The divided scaffold takes over above the fork.
        branches[0] = (order, points[:10], radii[:9] + [radii[9] * .28])
    sprays = []

    def branch(start, outward, reach, rise, radius, tier, sideways):
        side = unit(np.cross(UP, outward)) * sideways
        path = [start + outward * (reach * f) + UP * (reach * rise * (f**1.5 - .17 * math.sin(math.pi*f)))
                + side * (reach * math.sin(f * 2.4) * f) for f in (0, .20, .45, .72, 1.)]
        branches.append((tier, path, [radius * value for value in (1, .83, .61, .34, .045)]))
        return path

    roots = 9 if tree.form in {"kapok", "fig", "mangrove"} else (4 if palm else 7)
    for i in range(roots):
        axis = horizontal(i * TAU / roots + uniform(-.24, .24))
        reach = trunk_radius * uniform(2.1, 4.2)
        origin = trunk(0) + UP * trunk_radius * uniform(.5, 1.3)
        if tree.form in {"kapok", "fig", "mangrove"}:
            origin = trunk(uniform(.06, .16))
            reach *= 1.5 if tree.form == "mangrove" else 1.2
        branch(origin, axis, reach, -.36 if tree.form == "mangrove" else -.25,
               trunk_radius * uniform(.21, .44), 1, uniform(-.12, .12))

    if tree.form == "yucca":
        points, widths = branches[0][1:]
        branches[0] = (0, points[:10], widths[:10])
        for i in range(tree.levels):
            outward = horizontal(i * 2.399963 + uniform(-.3, .3))
            start = trunk(uniform(.29, .55))
            main = branch(start, outward, width * uniform(.46, .82), 1.15,
                          trunk_radius * uniform(.32, .54), 1, uniform(-.14, .14))
            for side in (-1, 1):
                axis = horizontal(i * 2.399963 + side * uniform(.38, .94))
                tip = branch(main[3], axis, width * uniform(.14, .28), 1.6, trunk_radius*.22,
                             2, side*.06)[-1]
                for j in range(6):
                    direction = horizontal(j*TAU/6) + UP*uniform(.12, .93)
                    sprays.append((tip, direction, h*.21, h*.069, j%4, uniform(-.9,.9), 0))
        return branches, sprays

    if palm:
        branches[0] = (
            0,
            [trunk(i / 14) for i in range(15)],
            [trunk_radius * (.82 + .24 * (1 - i / 14)**12) for i in range(14)] + [.005],
        )
        start = trunk(.965)
        for i in range(tree.levels):
            azimuth = i * 2.399963 + uniform(-.18, .18)
            direction = horizontal(azimuth)
            age = i / max(1, tree.levels - 1)
            length = width * (.69 + .37 * math.sin(math.pi * age)) * uniform(.83, 1.13)
            if tree.slug == "silver-tree-fern":
                rise, arch = uniform(-.58, -.20), .14
            else:
                rise = tree.spread + .70 - 1.93 * age + uniform(-.16, .16)
                arch = .19 + .11 * age
            origin = start - UP * (trunk_radius * age * .9)

            def rachis(t):
                return origin + length * (direction*t + UP*(rise*t*t + arch*math.sin(math.pi*t)))

            path = [rachis(t) for t in (0, .22, .46, .72, .89, 1.)]
            stem = min(trunk_radius * .20, length * .036)
            branches.append((1, path, [stem * taper for taper in (1, .84, .61, .36, .15, .04)]))
            if tree.form == "fan":
                for j in range(3):
                    ray = horizontal(azimuth + (j-1)*.50) + UP * (rise - .28)
                    center = path[-1] + unit(ray) * length * .16
                    sprays.append((center, ray, length*.55, length*.26, j, (j-1)*.22, 0))
                continue
            for j in range(8):
                t = (j + 1.1) / 9.3
                blade = length * (.27 + .18*math.sin(math.pi*t)) * (1 - .54*t)
                for sign in (-1, 1):
                    side = horizontal(azimuth + sign * (1.18 - .65*t)) - UP * (.28 + .45*t)
                    center = rachis(t) + unit(side) * blade * .29
                    sprays.append((center, side, blade, length*.13, (j+i)%4, sign*.20, 0))
            sprays.append((path[-1], direction-UP*.4, length*.19, length*.06, i%4, .1, 0))
        return branches, sprays

    for level in range(tree.levels):
        n = 3 if conifer or flat else (3 if level < 4 else 4)
        t = tree.start + (.952 - tree.start) * level / max(1, tree.levels - 1)
        taper = (t - tree.start) / (.967 - tree.start)
        if tree.form == "spire":
            envelope = (1 - taper)**.83 * .93 + .08
        elif tree.form == "column":
            envelope = (.60 + .4 * math.sin(math.pi*taper)) * (1 - taper*.64)
        elif tree.form == "tiered" or tree.form == "cedar":
            envelope = (1 - taper)**.56 * (.79 if level % 3 == 1 else 1.05) + .05
        elif tree.form == "droop":
            envelope = math.sqrt(1 - taper) * (.71 + .28*math.sin(math.pi*taper)) + .07
        elif flat:
            envelope = .31 + .69 * math.sin(taper * math.pi * .67)**.72
        elif tree.form == "weeping":
            envelope = .46 + .54*math.sin(math.pi*taper)**.7
        elif tree.form == "vase":
            envelope = .33 + .67*math.sin(math.pi*taper*.84)**.9
        else:
            envelope = .26 + .77*math.sin(math.pi*taper)**.82
        for j in range(n):
            a = level * 2.399963 + j * TAU / n + uniform(-.25, .25)
            axis = horizontal(a)
            local_t = min(.96, t + uniform(-.024, .024))
            start = trunk(local_t if not flat else min(local_t, .40 + .18 * taper))
            reach = width * envelope * uniform(.66, 1.14)
            if tree.form == "umbrella":
                target_height = h * (.78 + .083 * taper + uniform(-.040, .043))
                rise = (target_height - start[1]) / (reach * .84)
            else:
                rise = tree.spread + uniform(-.18, .25)
            primary_radius = trunk_radius * (1 - local_t)**.71 * uniform(.29, .59)
            path = branch(start, axis, reach * .84, rise, primary_radius, 1 if (j + level) % 3 != 0 else 2, uniform(-.25, .25))
            for k in range(4 if conifer else 3):
                f = (k + 1.1) * (.17 if conifer else .25)
                junction = path[min(4, k + (2 if flat else 1))]
                side = horizontal(a + (1 if k % 2 else -1) * uniform(.55, 1.35))
                extension = reach * uniform(.29, .47) * (1 - f * .35)
                twig_rise = (-.02 if conifer else rise * .30) + uniform(-.29, .24)
                if flat:
                    twig_rise *= .21
                if tree.form in {"droop", "weeping"}:
                    twig_rise -= uniform(.36, .75)
                twig = branch(junction, side, extension, twig_rise, primary_radius * uniform(.24, .43), 2 if k % 2 == 0 else 3, uniform(-.18, .18))
                for s in range(3 if conifer else 4):
                    card_axis = unit(side + horizontal(a + uniform(-2, 2)) * .52 + UP * (twig_rise + uniform(-.58, .56)))
                    center = twig[min(4, 1 + s)] + card_axis * extension * uniform(-.17, .36)
                    if conifer:
                        center -= UP * reach * uniform(.06,.25)
                    length = extension * uniform(.65, 1.25) + h * (.064 if conifer else .048)
                    if tree.form == "weeping":
                        card_axis = unit(card_axis * .43 - UP)
                        length *= 1.55
                        center -= UP * length*.32
                    half_width = length * uniform(.32, .55 if conifer else .52)
                    sprays.append((center, card_axis, length, half_width, r.randrange(4), uniform(-.8, .8), int(s == 0 and k % 2 == 0)))
            for s in range(3):
                axis_tip = unit(axis + UP * (rise + uniform(-.6, .45)) + horizontal(a + uniform(-1.5, 1.5))*.65)
                length = reach * uniform(.23,.40) + h*.028
                sprays.append((path[-1]-axis*length*.18, axis_tip, length, length*(.28 if conifer else .43), r.randrange(4), uniform(-.9,.9), 0))
    for i in range(0 if flat else 7):
        t = .90 + .013 * i
        axis = horizontal(i * 2.399963)
        size = h * (.070 if conifer else .115) * (1 - .064 * i)
        sprays.append((trunk(t), unit(axis+UP*.34), size, size*.45, i%4, .6, 0))
    return branches, sprays


def growth_texture(tree, directory):
    size = 256
    atlas = Image.new("RGBA", (size*2, size*2), (*tree.color, 0))
    relief = Image.new("L", atlas.size, 93)
    conifer = tree.form in {"spire", "column", "tiered", "droop", "cedar"}
    palm = tree.form in {"pinnate", "fan", "yucca"}
    leaflets = tree.form in {"umbrella", "widespread"}
    brush, height = ImageDraw.Draw(atlas), ImageDraw.Draw(relief)
    rng = random.Random(12777)
    for tile in range(4):
        x0, y0 = tile%2*size, tile//2*size
        if tree.form == "fan":
            center = (.50,.14)
            for leaflet in range(25):
                a = (leaflet-12)*.063
                reach = .73 + .09*math.cos(a*2.8) + rng.uniform(-.045,.05)
                dx,dy=math.sin(a),math.cos(a)
                end=(.5+dx*reach,.14+dy*reach)
                pts=[center, (.50+dx*reach*.71-dy*.031,.14+dy*reach*.71+dx*.031),end,
                     (.50+dx*reach*.71+dy*.026,.14+dy*reach*.71-dx*.026)]
                pixels=[(int(x0+x*size),int(y0+y*size)) for x,y in pts]
                color=tuple(min(255,int(c*rng.uniform(.76,1.23))) for c in tree.color)
                brush.polygon(pixels,fill=(*color,255))
                height.polygon(pixels,fill=142+rng.randrange(22))
            continue
        for branch in range(10 if conifer else 8):
            y = .12 + .075 * branch
            for side in (-1,1):
                start = np.array((.5 + rng.uniform(-.045,.045), y))
                end = np.array((.5 + side * (.23 + .17*math.sin(math.pi*y))*rng.uniform(.78,1.06),
                                y + rng.uniform(.11,.23)))
                pixel = lambda p: tuple((p*size + (x0,y0)).astype(int))
                brush.line((pixel(start),pixel(end)),fill=(88,76,45,255),width=2)
                axis = end-start
                normal = unit(np.array((-axis[1],axis[0])))
                leaves = 16 if conifer or palm else 11
                for leaf in range(leaves):
                    f = (leaf+.3) / leaves
                    root = start + axis * f + normal * rng.uniform(-.017,.017)
                    if conifer:
                        length = rng.uniform(.035,.083) * (1-f*.44)
                        half = length * rng.uniform(.086,.154)
                    elif palm:
                        length, half = rng.uniform(.14,.20), rng.uniform(.009,.016)
                    elif leaflets:
                        length, half = rng.uniform(.035,.061), rng.uniform(.007,.015)
                    else:
                        length = rng.uniform(.042,.102)
                        half = length * rng.uniform(.20,.40)
                    d = unit(normal * (-1 if leaf%2 else 1) + axis * rng.uniform(.14,1.45))
                    wing = np.array((-d[1],d[0]))
                    tip = root + length*d
                    pts=[root]+[root+d*length*f+wing*half*math.sin(math.pi*f) for f in (.20,.40,.60,.80)]
                    pts += [tip]+[root+d*length*f-wing*half*.87*math.sin(math.pi*f) for f in (.82,.62,.42,.22)]
                    points=list(map(pixel,pts))
                    var = rng.uniform(.69,1.35)
                    snow_accumulation = (.42 + .5 * math.sin(math.pi*y)**2) * (.97 if side == 1 else .66)
                    snowy = rng.random() < min(.98, tree.snow * snow_accumulation * 1.31)
                    color = tuple(int(np.clip(c*var, 0,255)) for c in ((211,220,226) if snowy else tree.color))
                    brush.polygon(points, fill=(*color,255))
                    height.polygon(points,fill=128 + rng.randrange(10,39))
                    vein = tuple(min(255,int(c*1.15)) for c in color)
                    brush.line((pixel(root),pixel(root+d*length*.85)),fill=(*vein,255),width=1)
                    height.line((pixel(root),pixel(root+d*length*.86)),fill=176,width=1)
    name = tree.slug
    atlas.transpose(Image.Transpose.FLIP_TOP_BOTTOM).save(directory/f"{name}_leaves.png",optimize=True)
    dy, dx = np.gradient(np.asarray(relief,dtype=float) / 255.)
    normal = np.dstack((-dx*1.28, dy*1.28, np.ones_like(dx)))
    normal /= np.linalg.norm(normal,axis=2)[...,None]
    Image.fromarray(np.uint8(127.5*(normal+1))).transpose(Image.Transpose.FLIP_TOP_BOTTOM).save(directory/f"{name}_normal.png",optimize=True)
    rgba = np.asarray(atlas)
    rgb = rgba[rgba[...,3] >= 100,:3].astype(float) / 255.
    return np.where(rgb <= .04045, rgb/12.92, ((rgb+.055)/1.055)**2.4).mean(axis=0)


def export(path, tree, branches, sprays, lod):
    wood, foliage = Mesh(), Mesh()
    for order, points, widths in branches:
        if (lod == 1 and order == 3) or (lod == 2 and order >= 2):
            continue
        if order >= 2 or lod == 2:
            keep = [0, *range(2, len(points)-1, 2), len(points)-1]
            points, widths = [points[i] for i in keep], [widths[i] for i in keep]
        sides = ((9,7,5), (6,5,4), (4,3,3), (3,3,3))[order][lod]
        wood.tube(points,widths,sides,.115 if order==0 else .046)
    for center,axis,length,width,tile,tilt,interior in sprays:
        if lod==2 and interior:
            continue
        foliage.spray(center,axis,length,width,tile,tilt,lod)
    binary = bytearray()
    views, accessors = [], []

    def accessor(values, dtype, kind, target):
        array = np.asarray(values,dtype=dtype)
        raw = array.tobytes()
        view = len(views)
        views.append(dict(buffer=0,byteOffset=len(binary),byteLength=len(raw),target=target))
        binary.extend(raw)
        binary.extend(bytes(-len(binary)%4))
        index = len(accessors)
        value=dict(bufferView=view, componentType=5126 if dtype=="<f4" else 5123, count=len(array), type=kind)
        if kind == "VEC3":
            value.update(min=array.min(axis=0).tolist(), max=array.max(axis=0).tolist())
        accessors.append(value)
        return index

    primitives=[]
    for i,mesh in enumerate((wood,foliage)):
        primitives.append(dict(attributes={
            "POSITION":accessor(mesh.positions,"<f4","VEC3",34962),
            "NORMAL":accessor(mesh.normals,"<f4","VEC3",34962),
            "TEXCOORD_0":accessor(mesh.uv,"<f4","VEC2",34962),
        },indices=accessor(mesh.indices,"<u2","SCALAR",34963),material=i))
    bark = f"../textures/{tree.bark}"
    doc={
      "asset":{"version":"2.0","generator":"world-client procedural tree library"},
      "scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"name":tree.name}],"meshes":[{"primitives":primitives}],
      "materials":[
        {"name":"Bark","pbrMetallicRoughness":{"baseColorTexture":{"index":0},"metallicRoughnessTexture":{"index":2},"metallicFactor":0},"normalTexture":{"index":1}},
        {"name":"Leaves","alphaMode":"MASK","alphaCutoff":.39,"doubleSided":True,
         "pbrMetallicRoughness":{"baseColorTexture":{"index":3},"roughnessFactor":.82,"metallicFactor":0},
         "normalTexture":{"index":4,"scale":.47}},
      ],
      "images":[{"uri":uri} for uri in [f"{bark}/diff.jpg",f"{bark}/nor_gl.jpg",f"{bark}/arm.jpg",
                 f"../textures/{tree.slug}_leaves.png",f"../textures/{tree.slug}_normal.png"]],
      "samplers":[{"magFilter":9729,"minFilter":9987},{"magFilter":9729,"minFilter":9987,"wrapS":33071,"wrapT":33071}],
      "textures":[{"source":i,"sampler":int(i>2)} for i in range(5)],
      "buffers":[{"byteLength":len(binary)}],"bufferViews":views,"accessors":accessors,
    }
    payload=json.dumps(doc,separators=(",",":")).encode()
    payload+=b" "*(-len(payload)%4)
    path.write_bytes(struct.pack("<5I",0x46546C67,2,28+len(payload)+len(binary),len(payload),0x4E4F534A)+payload
                     +struct.pack("<2I",len(binary),0x004E4942)+binary)
    highest = max(max(p[1] for p in wood.positions), max(p[1] for p in foliage.positions))
    width = max(math.hypot(p[0], p[2]) for p in foliage.positions)
    return (len(wood.indices)+len(foliage.indices))//3, highest, width/highest


def main():
    args=argparse.ArgumentParser(description=__doc__)
    args.add_argument("--output",type=Path,default=DEST)
    args.add_argument("--catalog",type=Path,default=RUST)
    options=args.parse_args()
    root=options.output
    textures=root/"textures"
    textures.mkdir(parents=True,exist_ok=True)
    records=[]
    for tree in sorted(TREES, key=lambda entry: entry.biome):
        folder=root/tree.slug
        folder.mkdir(exist_ok=True)
        color = growth_texture(tree,textures)
        leaf = "["+", ".join(f"{channel:.6f}" for channel in color)+"]"
        for variant in range(2):
            branches,sprays=skeleton(tree,variant)
            records_by_lod=[export(folder/f"tree_{variant}_lod{lod}.glb", tree, branches, sprays, lod) for lod in range(3)]
            counts, heights, widths = zip(*records_by_lod)
            if not 6000 >= counts[0] > counts[1] > counts[2]:
                raise ValueError((tree.slug,variant,counts))
            print(tree.slug,variant,counts)
            path=f"library/{tree.slug}/tree_{variant}"
            records.append(f'    TreeAsset {{ path: "{path}", species: Species::{tree.biome}, height: {heights[0]:.4f}, width: {widths[0]:.4f}, color: {leaf}, snow: {str(bool(tree.snow)).lower()} }},')
    options.catalog.write_text("    // Generated by tools/build_tree_library.py.\n&[\n"+"\n".join(records)+"\n]\n")


if __name__=="__main__":
    main()
