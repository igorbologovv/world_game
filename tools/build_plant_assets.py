#!/usr/bin/env python3
"""Build standalone low-poly plant GLBs using only the Python standard library."""

import argparse
import hashlib
import math
import random
from dataclasses import replace
from pathlib import Path

from plant_geometry import (
    MAX_TRIANGLES, ORIGIN, UP, Mesh, PathCurve, add, axis, cross, export_glb, frame, linear_rgb, scale, unit,
)
from plant_species import SPECIES


DESTINATION = Path(__file__).resolve().parents[1] / "assets/models/vegetation/procedural_plants"
CATALOG = {species.name: species for species in SPECIES}
PARTS = ("stem", "leaf", "branch", "flower")


def variation_seed(species, seed, variant):
    key = f"{species}/base" if variant == 0 else f"{species}/{seed}/{variant}"
    return int.from_bytes(hashlib.blake2b(key.encode(), digest_size=8).digest(), "little")


class Plant:
    """Attach reusable part recipes at positions along the selected growth habit."""

    def __init__(self, species, seed, variant):
        self.species = species
        self.random = random.Random(variation_seed(species.name, seed, variant))
        self.varied = variant != 0
        self.mesh = Mesh()
        self.flower_sites = []
        self.height = species.height * self.factor(0.76, 1.25)
        self.spread = species.branch.spread * self.height * self.factor(0.76, 1.30)
        self.rise = species.branch.rise * self.height * self.factor(0.83, 1.18)
        self.length = species.leaf.length * self.height * self.factor(0.76, 1.26)
        self.width = species.leaf.width * self.factor(0.75, 1.34)
        self.arch = species.leaf.arch * self.factor(0.75, 1.45)
        self.shape = self.pick(species.leaf.shapes)
        self.foliage = linear_rgb(self.pick(species.leaf.colors))
        self.stem_color = linear_rgb(species.stem.color)
        self.stem_radius = species.stem.radius * self.height * self.factor(0.75, 1.35)
        self.stem_curve = species.stem.curve * self.factor(0.60, 1.5)
        self.flower = species.flower

    def factor(self, low, high):
        return self.random.uniform(low, high) if self.varied else 1.0

    def pick(self, values):
        if self.varied:
            return self.random.choice(values)
        return values[0]

    def count(self, limits):
        return self.random.randint(*limits) if self.varied else sum(limits) // 2

    def directions(self, count):
        phase = self.random.random() * math.tau
        for i in range(count):
            yield axis(phase + math.tau * (i + self.random.uniform(-0.21, 0.21)) / count)

    def around(self, direction, count):
        side, normal = frame(unit(direction))
        for x, _, z in self.directions(count):
            yield add(scale(side, x), scale(normal, z))

    def branches(self):
        return self.directions(self.count(self.species.branch.count))

    def stalk(self, start, end, bend=ORIGIN, thickness=1.0, mesh=None):
        path = PathCurve(start, end, bend)
        radius = self.stem_radius * thickness
        rings = [(0, radius), (1, radius * 0.35)]
        if bend != ORIGIN:
            rings.insert(1, (0.48, radius * 0.72))
        destination = self.mesh if mesh is None else mesh
        destination.tube(path, rings, self.species.stem.sides, self.stem_color)
        return path

    def leaf(self, at, direction, size=1.0, arch=None, compact=False):
        length = self.length * size * self.random.uniform(0.86, 1.13)
        color = scale(self.foliage, self.random.uniform(0.82, 1.18))
        self.mesh.blade(at, direction, (length, length * self.width), self.shape,
                        self.arch if arch is None else arch, self.species.leaf.fold, color, compact=compact)

    def leaves_along(self, path, count, size=1.0, compact=False):
        for i, radial in enumerate(self.directions(count)):
            t = 0.26 + 0.65 * i / max(count - 1, 1)
            direction = add(radial, add(scale(UP, 0.22), scale(path.tangent(t), 0.24)))
            self.leaf(path.at(t), direction, size * (1.15 - t * 0.40), compact=compact)

    def bloom(self, mesh, at, heading, radius, petals, center_color, compact=False):
        radius *= self.random.uniform(0.87, 1.13)
        count = self.count(self.flower.parts)
        if compact:
            count = min(count, 4)
        form = self.flower.form

        if form in ("daisy", "cup", "bell"):
            mesh.bud(at, heading, radius * 0.26, radius * (0.29 if form == "daisy" else 0.17), center_color)
            petal_width = {"daisy": 0.20, "cup": 0.51, "bell": 0.47}[form]
            petal_lift = {"daisy": 0.08, "cup": 0.55, "bell": 1.40}[form]
            for radial in self.around(heading, count):
                offset = add(at, scale(radial, radius * 0.09))
                direction = add(radial, scale(heading, petal_lift))
                color = scale(petals, self.random.uniform(0.86, 1.11))
                mesh.blade(offset, direction, (radius, radius * petal_width), "petal", -0.32,
                           0.25, color, hint=heading)
            return

        if form in ("umbel", "panicle"):
            for i, radial in enumerate(self.around(heading, count)):
                rise = 0.48 if form == "umbel" else 0.40 + 0.22 * i
                point = add(at, scale(add(radial, scale(heading, rise)), radius))
                self.stalk(at, point, thickness=0.20, mesh=mesh)
                for floret in self.around(heading, 2 if compact else 3):
                    direction = add(floret, scale(heading, 0.72))
                    mesh.bud(point, direction, radius * 0.33, radius * 0.12, petals)
                if form == "umbel":
                    mesh.bud(point, heading, radius * 0.17, radius * 0.09, center_color)
            return

        end = add(at, scale(heading, radius * 2.1))
        spike = self.stalk(at, end, thickness=0.34, mesh=mesh)
        for i, radial in enumerate(self.around(heading, count)):
            t = (i + 0.35) / count
            size = radius * (0.78 - 0.43 * t)
            root = spike.at(t)
            mesh.bud(root, add(radial, scale(heading, 0.64)), size, size * 0.45, petals)
            if form == "plume":
                mesh.bud(root, add(scale(radial, -1), heading), size, size * 0.42, petals)

    def tuft(self):
        for radial in self.branches():
            base = scale(radial, self.spread * self.random.uniform(0.02, 0.19))
            splay = self.spread * self.random.uniform(0.43, 1.41)
            tilt = self.rise * self.random.uniform(0.43, 1.25)
            self.leaf(base, add(scale(radial, splay), scale(UP, tilt)), arch=self.arch * 3.6)
        for radial in self.directions(self.count(self.species.branch.leaves)):
            top = add(scale(radial, self.spread * 0.44), scale(UP, self.rise * self.random.uniform(0.67, 1.04)))
            if not self.flower:
                top = scale(top, 0.53)
            stalk = self.stalk(scale(radial, self.spread * 0.06), top,
                               scale(radial, -self.height * self.stem_curve * 0.19), thickness=0.62)
            self.leaf(stalk.at(0.32), add(radial, UP), 0.53)
            self.flower_sites.append((top, stalk.tangent(1), 0.68))

    def upright(self):
        leaf_count = self.count(self.species.branch.leaves)
        for i, radial in enumerate(self.branches()):
            extent = self.spread * (0.12 if i == 0 else self.random.uniform(0.42, 1.0))
            rise = self.rise * (1.05 if i == 0 else self.random.uniform(0.64, 0.92))
            end = add(scale(radial, extent), scale(UP, rise))
            stalk = self.stalk(ORIGIN, end, scale(radial, -self.height * self.stem_curve * 0.59))
            self.leaves_along(stalk, leaf_count)
            heading = add(scale(UP, -0.65), scale(radial, 0.78)) if self.flower and self.flower.form == "bell" else UP
            self.flower_sites.append((end, unit(heading), 1.0 if i == 0 else 0.78))

    def shrub(self):
        for radial in self.branches():
            length = self.random.uniform(0.66, 1.14)
            end = add(scale(radial, self.spread * length), scale(UP, self.rise * length))
            branch = self.stalk(ORIGIN, end, scale(UP, self.height * self.stem_curve * 0.60))
            count = self.count(self.species.branch.leaves)
            self.leaves_along(branch, count, 0.79, compact=True)
            side = cross(radial, UP)
            for n, sign in enumerate((-1, 1)):
                start = branch.at(0.43 + n * 0.23)
                fork = add(scale(radial, 0.18 * self.spread), scale(side, sign * 0.27 * self.spread))
                tip = add(start, add(fork, scale(UP, self.rise * self.random.uniform(0.22, 0.44))))
                twig = self.stalk(start, tip, thickness=0.44)
                self.leaves_along(twig, 2, 0.70, compact=True)
                self.leaf(tip, add(radial, UP), 0.67, compact=True)
            heading = scale(UP, -1) if self.flower and self.flower.form == "bell" else UP
            self.flower_sites.append((end, heading, 0.65))

    def creeper(self):
        for radial in self.branches():
            length = self.random.uniform(0.65, 1.21)
            start = scale(radial, self.height * 0.05)
            tip = add(scale(radial, self.spread * length), scale(UP, self.rise * length))
            branch = self.stalk(start, tip, scale(UP, self.height * self.stem_curve * 0.90), 0.78)
            side = cross(radial, UP)
            for i in range(self.count(self.species.branch.leaves)):
                direction = add(scale(side, (-1) ** i), scale(UP, self.random.uniform(0.18, 0.62)))
                at = branch.at(0.24 + 0.13 * i)
                joint = add(at, scale(direction, self.length * 0.37))
                self.stalk(at, joint, thickness=0.37)
                self.leaf(joint, add(direction, scale(radial, 0.23)), 0.84)
            if self.flower:
                start = branch.at(0.69)
                top = add(start, scale(UP, self.height * 0.52))
                self.stalk(start, top, thickness=0.46)
                self.flower_sites.append((top, radial if self.flower.form == "bell" else UP, 1.0))

    def rosette(self):
        for radial in self.branches():
            base = scale(radial, self.spread * 0.07)
            start = add(base, scale(UP, self.rise * 0.13))
            self.stalk(base, start, thickness=0.64)
            direction = add(scale(radial, self.spread), scale(UP, self.rise * self.random.uniform(0.50, 1.50)))
            self.leaf(start, direction, arch=self.arch * 2.7)
        if self.flower:
            for radial in self.directions(self.count(self.species.branch.leaves)):
                tip = add(scale(radial, self.spread * 0.48), scale(UP, self.rise * self.random.uniform(1.0, 1.6)))
                stalk = self.stalk(ORIGIN, tip, scale(radial, -self.height * self.stem_curve * 0.23))
                self.flower_sites.append((tip, stalk.tangent(1), 0.76))

    def fern(self, divided):
        for radial in self.branches():
            size = self.random.uniform(0.68, 1.17)
            tip = add(scale(radial, size * self.spread), scale(UP, size * self.rise))
            rachis = self.stalk(ORIGIN, tip, scale(UP, size * self.height * self.stem_curve * 0.80), 0.72)
            side = cross(radial, UP)
            pair_count = self.count(self.species.branch.leaves)
            for i in range(pair_count):
                along = i / pair_count
                t = 0.28 + 0.65 * along
                taper = size * (1.20 - along * 0.98)
                for sign in (-1, 1):
                    root = rachis.at(t + sign * 0.008)
                    direction = add(scale(side, sign), scale(radial, 0.36 + along * 0.75))
                    if divided and i < 2:
                        end = add(root, scale(unit(direction), self.spread * taper * 0.42))
                        pinna = self.stalk(root, end, thickness=0.24)
                        self.leaves_along(pinna, 3, taper * 0.8, compact=True)
                        self.leaf(end, direction, taper * 0.57, compact=True)
                    else:
                        self.leaf(root, direction, taper, compact=True)
            self.leaf(rachis.at(0.91), radial, size * 0.48)
            self.flower_sites.append((tip, UP, 0.53))

    def build(self):
        habit = self.species.habit
        if habit in ("pinnate", "divided"):
            self.fern(divided=habit == "divided")
        else:
            builders = {
                "tuft": self.tuft, "upright": self.upright, "shrub": self.shrub,
                "creeper": self.creeper, "rosette": self.rosette,
            }
            builders[habit]()
        if self.flower:
            petals = linear_rgb(self.pick(self.flower.colors))
            center_color = linear_rgb(self.flower.center)
            radius = self.flower.radius * self.height * self.factor(0.72, 1.28)
            flower_start = self.mesh.triangle_count
            for at, heading, size in self.flower_sites:
                bloom = Mesh()
                self.bloom(bloom, at, heading, radius * size, petals, center_color)
                if self.mesh.triangle_count == flower_start and flower_start + bloom.triangle_count > MAX_TRIANGLES:
                    # Keep one representative cluster when the foliage leaves little room.
                    bloom = Mesh()
                    self.bloom(bloom, at, heading, radius * size, petals, center_color, compact=True)
                if self.mesh.triangle_count + bloom.triangle_count <= MAX_TRIANGLES:
                    self.mesh.merge(bloom)
        return self.mesh


def nonnegative(text):
    value = int(text)
    if value < 0:
        raise argparse.ArgumentTypeError("expected a nonnegative integer")
    return value


def part_overrides(parser, swaps):
    overrides = {}
    for swap in swaps:
        kind, _, donor = swap.partition("=")
        if kind not in PARTS or donor not in CATALOG or kind in overrides:
            parser.error(f"invalid or repeated part swap {swap!r}; use PART=SPECIES with --list-species")
        overrides[kind] = CATALOG[donor]
    return overrides


def compose_species(parser, recipe, overrides):
    branch_donor = overrides.get("branch")
    if branch_donor and branch_donor.habit != recipe.habit:
        parser.error(f"branch donor {branch_donor.name} uses {branch_donor.habit}; {recipe.name} uses {recipe.habit}")
    return replace(recipe, **{kind: getattr(donor, kind) for kind, donor in overrides.items()})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=DESTINATION, help="destination directory")
    parser.add_argument("--species", nargs="+", choices=tuple(CATALOG), help="generate only the selected species")
    parser.add_argument("--list-species", action="store_true", help="list species names, categories and growth habits")
    parser.add_argument("--seed", type=nonnegative, default=0, help="seed for numbered variants")
    parser.add_argument("--variants", type=nonnegative, default=0, help="numbered variants per species, in addition to the base")
    parser.add_argument("--parts-from", action="append", default=[], metavar="PART=SPECIES",
                        help="replace a stem, leaf, branch or flower recipe with one from another species")
    args = parser.parse_args()

    if args.list_species:
        for recipe in SPECIES:
            print(f"{recipe.category:8} {recipe.name:23} {recipe.habit}")
        return

    overrides = part_overrides(parser, args.parts_from)
    names = dict.fromkeys(args.species) if args.species else CATALOG
    recipes = [compose_species(parser, CATALOG[name], overrides) for name in names]
    output = args.out
    if overrides:
        donors = "--".join(f"{kind}_{overrides[kind].name}" for kind in PARTS if kind in overrides)
        output = output / "hybrids" / donors

    triangle_counts = []
    total_size = 0
    for recipe in recipes:
        directory = output / recipe.category
        for variant in range(args.variants + 1):
            name = recipe.name
            path = directory / f"{name}.glb"
            if variant:
                name = f"{name}_{variant:04d}"
                path = directory / recipe.name / f"seed_{args.seed}" / f"variant_{variant:04d}.glb"
            try:
                triangles, size = export_glb(path, Plant(recipe, args.seed, variant).build(), name)
            except (ValueError, OSError) as error:
                parser.exit(1, f"{path}: {error}\n")
            triangle_counts.append(triangles)
            total_size += size
    print(f"Exported {len(triangle_counts):,} GLBs from {len(recipes)} species "
          f"({min(triangle_counts):,}–{max(triangle_counts):,} triangles, {total_size / 1024**2:.2f} MiB).")


if __name__ == "__main__":
    main()
