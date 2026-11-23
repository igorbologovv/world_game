"""Species recipes for the offline vegetation asset generator."""

from dataclasses import dataclass


@dataclass(frozen=True)
class Stem:
    color: str
    radius: float  # Relative to plant height.
    curve: float
    sides: int = 5


@dataclass(frozen=True)
class Leaf:
    shapes: tuple[str, ...]
    colors: tuple[str, ...]
    length: float  # Relative to plant height.
    width: float  # Half-width relative to leaf length.
    arch: float
    fold: float = 0.7


@dataclass(frozen=True)
class Branch:
    count: tuple[int, int]
    spread: float  # Relative to plant height.
    rise: float
    leaves: tuple[int, int]


@dataclass(frozen=True)
class Flower:
    form: str
    colors: tuple[str, ...]
    center: str
    radius: float  # Relative to plant height.
    parts: tuple[int, int]


@dataclass(frozen=True)
class Species:
    name: str
    category: str
    habit: str
    height: float  # Meters at the reference scale.
    stem: Stem
    leaf: Leaf
    branch: Branch
    flower: Flower | None = None


def leaf(shapes, colors, length, width, arch, fold=0.7):
    return Leaf(tuple(shapes.split()), tuple(colors.split()), length, width, arch, fold)


def flower(form, colors, center, radius, low, high):
    return Flower(form, tuple(colors.split()), center, radius, (low, high))


GREEN = Stem("5c713d", 0.009, 0.12)
RUSH = Stem("777642", 0.007, 0.06)
RED = Stem("845045", 0.011, 0.23)
WOODY = Stem("705642", 0.024, 0.22, 6)
SILVER = Stem("7f8269", 0.016, 0.17)
FROND = Stem("8a844a", 0.010, 0.25)
DARK_FROND = Stem("4d453d", 0.008, 0.22)
FLESHY = Stem("55764b", 0.036, 0.15, 6)


SPECIES = (
    Species("blue_fescue", "grasses", "tuft", 0.26, RUSH,
            leaf("needle ribbon", "648784 789b98 527677", 0.99, 0.043, 0.27),
            Branch((16, 22), 0.64, 0.82, (2, 3))),
    Species("red_bentgrass", "grasses", "tuft", 0.52, RED,
            leaf("ribbon needle", "607a43 78864c 6e7145", 0.66, 0.061, 0.40),
            Branch((10, 15), 0.54, 0.83, (3, 5)),
            flower("panicle", "99556c b37488 976970", "876253", 0.14, 4, 6)),
    Species("silver_oat_grass", "grasses", "tuft", 0.70, RUSH,
            leaf("strap ribbon", "698570 829989 577361", 0.72, 0.075, 0.52),
            Branch((10, 14), 0.68, 0.83, (2, 4)),
            flower("panicle", "c1b68a ada77e cdc79d", "9f946f", 0.13, 4, 6)),
    Species("purple_moor_grass", "grasses", "tuft", 0.91, RED,
            leaf("ribbon strap", "677947 878850 687f4c", 0.58, 0.070, 0.55),
            Branch((12, 17), 0.47, 0.97, (4, 6)),
            flower("spike", "735168 91647c a77d88", "755056", 0.11, 5, 7)),
    Species("golden_foxtail", "grasses", "tuft", 0.57, GREEN,
            leaf("strap lance", "789247 667e39 8f9a53", 0.76, 0.092, 0.46),
            Branch((10, 15), 0.53, 0.84, (3, 5)),
            flower("plume", "c5a558 d7bd7d ae8e4b", "8b7041", 0.11, 5, 7)),
    Species("prairie_dropseed", "grasses", "tuft", 0.51, RUSH,
            leaf("ribbon needle", "8e8f50 769052 aa9d65", 0.94, 0.047, 0.62),
            Branch((18, 23), 0.83, 0.72, (2, 4)),
            flower("panicle", "b4a27d ba956a c4af8b", "887a53", 0.12, 5, 7)),
    Species("ribbon_grass", "grasses", "tuft", 0.48, GREEN,
            leaf("strap", "769352 98a86e 63864e", 0.94, 0.135, 0.37, 1.2),
            Branch((10, 14), 0.67, 0.62, (2, 3))),
    Species("spear_grass", "grasses", "tuft", 0.79, RUSH,
            leaf("needle ribbon", "8a8d51 9b975d 747e49", 0.60, 0.046, 0.28),
            Branch((11, 17), 0.49, 1.03, (3, 5)),
            flower("spike", "d4bd84 b69b5e d1c396", "9a844d", 0.13, 5, 7)),

    Species("oxeye_daisy", "flowers", "upright", 0.55, GREEN,
            leaf("spoon lobed", "50783e 6f8946 628347", 0.24, 0.24, 0.32),
            Branch((3, 5), 0.42, 0.98, (2, 4)),
            flower("daisy", "f7edde e5e7d8 f7f3e6", "dba52b", 0.17, 8, 12)),
    Species("scarlet_poppy", "flowers", "upright", 0.53, GREEN,
            leaf("lobed lance", "638254 799364 546f45", 0.30, 0.27, 0.38),
            Branch((2, 4), 0.39, 0.93, (2, 3)),
            flower("cup", "db4738 e8593e c93845", "442d38", 0.24, 4, 6)),
    Species("harebell", "flowers", "upright", 0.38, GREEN,
            leaf("lance heart", "618052 718d58 527349", 0.24, 0.20, 0.21),
            Branch((3, 5), 0.49, 0.96, (2, 4)),
            flower("bell", "776bb6 6c7fc8 8c79c2", "d7cd9f", 0.14, 4, 6)),
    Species("pink_yarrow", "flowers", "upright", 0.62, SILVER,
            leaf("lobed needle", "6c8c63 759366 5d8060", 0.24, 0.24, 0.25),
            Branch((2, 4), 0.44, 0.98, (3, 4)),
            flower("umbel", "dc8ba9 f0abbd eaa2b3", "e1b66b", 0.16, 5, 7)),
    Species("blue_lupine", "flowers", "upright", 0.78, GREEN,
            leaf("fan", "637d55 738f66 55784e", 0.23, 0.30, 0.20),
            Branch((2, 4), 0.37, 1.02, (2, 4)),
            flower("spike", "635db0 7573c2 9385c9", "b3a3c0", 0.17, 6, 8)),
    Species("yellow_buttercup", "flowers", "upright", 0.29, GREEN,
            leaf("lobed heart", "568045 6c914d 6d8a47", 0.31, 0.33, 0.29),
            Branch((3, 5), 0.62, 0.85, (2, 3)),
            flower("cup", "f5c538 e7ad2a f5d35c", "c18f25", 0.16, 5, 6)),
    Species("wild_carrot", "flowers", "upright", 0.76, GREEN,
            leaf("lobed", "617d40 72934a 547b4c", 0.28, 0.25, 0.27),
            Branch((2, 4), 0.50, 0.94, (3, 4)),
            flower("umbel", "e8e6d2 f3ecdf d2debd", "a78e55", 0.21, 6, 9)),
    Species("meadow_cranesbill", "flowers", "upright", 0.45, RED,
            leaf("lobed fan", "5f864e 6f945d 518653", 0.32, 0.31, 0.24),
            Branch((3, 6), 0.64, 0.78, (2, 4)),
            flower("cup", "8d83c5 a39ad0 b99ddb", "dac2bb", 0.16, 5, 6)),

    Species("silver_sagebrush", "shrubs", "shrub", 0.85, SILVER,
            leaf("lobed spoon", "8d9e86 a4af97 7d9688", 0.20, 0.23, 0.23),
            Branch((6, 9), 0.77, 0.70, (4, 6))),
    Species("flowering_heather", "shrubs", "shrub", 0.54, WOODY,
            leaf("needle lance", "567649 657f46 766846", 0.115, 0.14, 0.16),
            Branch((6, 9), 0.72, 0.67, (4, 6)),
            flower("spike", "b674a0 a35d90 d195b2", "c9a6a5", 0.09, 4, 6)),
    Species("juniper_scrub", "shrubs", "shrub", 1.15, WOODY,
            leaf("needle", "3f7061 557b66 4b6851", 0.22, 0.105, 0.19, 0.9),
            Branch((7, 10), 0.82, 0.51, (5, 7))),
    Species("red_dogwood", "shrubs", "shrub", 1.48, RED,
            leaf("oval lance", "ac6345 8b7841 bd754b", 0.22, 0.28, 0.27),
            Branch((5, 8), 0.72, 0.92, (3, 5)),
            flower("umbel", "e8dfc7 ddd2b6 f1e4cb", "aa925f", 0.10, 4, 5)),
    Species("bog_rosemary", "shrubs", "shrub", 0.43, SILVER,
            leaf("lance strap", "5e806e 76948b 698578", 0.22, 0.15, 0.24),
            Branch((5, 8), 0.71, 0.65, (4, 6)),
            flower("bell", "e49caa e9b6bd d98298", "d7ad88", 0.11, 4, 6)),
    Species("wild_boxwood", "shrubs", "shrub", 0.89, WOODY,
            leaf("oval spoon", "497141 63854b 567c46", 0.24, 0.34, 0.20),
            Branch((7, 10), 0.68, 0.70, (4, 6))),
    Species("sweet_myricaria", "shrubs", "shrub", 0.96, SILVER,
            leaf("needle lance", "7f9786 6e897d 9ca58b", 0.15, 0.14, 0.22),
            Branch((6, 8), 0.67, 0.92, (5, 7)),
            flower("plume", "edb5b9 d794a5 d9a0b2", "b9848d", 0.10, 4, 6)),
    Species("scrub_cinquefoil", "shrubs", "shrub", 0.72, WOODY,
            leaf("fan spoon", "64844e 789258 688052", 0.19, 0.27, 0.21),
            Branch((5, 8), 0.82, 0.57, (3, 5)),
            flower("cup", "efbb3d f4cb52 dca63e", "bd8d35", 0.10, 5, 6)),

    Species("sword_fern", "ferns", "pinnate", 0.67, FROND,
            leaf("lance ribbon", "4e864b 5a9054 6c9e60", 0.29, 0.17, 0.23),
            Branch((5, 8), 0.79, 0.75, (7, 10))),
    Species("lady_fern", "ferns", "pinnate", 0.61, FROND,
            leaf("lobed lance", "78a252 6e964b 89a25a", 0.31, 0.23, 0.24),
            Branch((5, 7), 0.78, 0.59, (7, 9))),
    Species("maidenhair_fern", "ferns", "pinnate", 0.37, DARK_FROND,
            leaf("fan heart", "649a56 77a65f 589453", 0.27, 0.40, 0.13),
            Branch((5, 8), 0.92, 0.62, (5, 7))),
    Species("bracken_fern", "ferns", "divided", 0.87, FROND,
            leaf("lance lobed", "568049 6c8f4c 779446", 0.13, 0.24, 0.22),
            Branch((4, 6), 0.89, 0.77, (4, 6))),
    Species("painted_fern", "ferns", "pinnate", 0.49, RED,
            leaf("lance lobed", "82998c a5afa0 718e84", 0.30, 0.26, 0.18, 1.05),
            Branch((5, 7), 0.93, 0.48, (6, 8))),
    Species("hart_tongue_fern", "ferns", "rosette", 0.46, FROND,
            leaf("strap lobed", "588842 6e9c46 4c8039", 1.04, 0.16, 0.42),
            Branch((7, 11), 0.68, 0.64, (1, 2))),
    Species("oak_fern", "ferns", "divided", 0.29, DARK_FROND,
            leaf("oval fan", "679954 72a95a 588f50", 0.17, 0.29, 0.16),
            Branch((4, 6), 1.04, 0.68, (3, 5))),
    Species("bird_nest_fern", "ferns", "rosette", 0.56, FROND,
            leaf("strap spoon", "679748 76a848 8ea853", 1.06, 0.19, 0.40, 1.0),
            Branch((8, 12), 0.59, 0.76, (1, 2))),

    Species("dune_marram", "coastal", "tuft", 0.69, RUSH,
            leaf("ribbon needle", "86966b acad7c 718b66", 0.84, 0.064, 0.53),
            Branch((13, 19), 0.75, 0.83, (3, 5)),
            flower("plume", "c6b482 e0cd9e b0a06f", "998957", 0.12, 5, 7)),
    Species("sand_sedge", "coastal", "tuft", 0.35, RUSH,
            leaf("strap ribbon", "787f41 928447 68834c", 0.83, 0.098, 0.31),
            Branch((9, 14), 0.76, 0.71, (3, 5)),
            flower("spike", "a48257 b49a69 987749", "735b3e", 0.13, 4, 6)),
    Species("sea_lavender", "coastal", "upright", 0.47, SILVER,
            leaf("spoon lance", "789272 668665 849b7c", 0.33, 0.25, 0.23),
            Branch((3, 5), 0.53, 0.94, (2, 3)),
            flower("panicle", "a080bd b79bcf 8d72ad", "dac7b7", 0.20, 5, 7)),
    Species("beach_morning_glory", "coastal", "creeper", 0.22, RED,
            leaf("heart", "5f9468 71a073 4b865d", 0.59, 0.43, 0.18),
            Branch((3, 5), 2.9, 0.22, (3, 5)),
            flower("cup", "d37aa9 ad629c e898b8", "f2dbd4", 0.29, 5, 6)),
    Species("sea_rocket", "coastal", "upright", 0.34, FLESHY,
            leaf("lobed succulent", "729b73 609481 7a9a72", 0.37, 0.27, 0.26),
            Branch((3, 5), 0.77, 0.78, (3, 5)),
            flower("cup", "dfb0cc ebc2d6 d0a2bd", "e7d38f", 0.12, 4, 4)),
    Species("red_glasswort", "coastal", "shrub", 0.33, FLESHY,
            leaf("succulent needle", "a65544 b4684a 7d8048", 0.18, 0.21, 0.16, 1.2),
            Branch((6, 9), 0.70, 0.82, (4, 6))),
    Species("beach_pea", "coastal", "creeper", 0.25, GREEN,
            leaf("oval heart", "729474 63896a 7c9e74", 0.55, 0.31, 0.22),
            Branch((4, 6), 2.25, 0.28, (4, 5)),
            flower("bell", "a46ca6 b481b3 8e5e99", "dac0cd", 0.19, 4, 5)),
    Species("sea_thrift", "coastal", "rosette", 0.25, RUSH,
            leaf("needle ribbon", "608349 758d4d 568047", 0.73, 0.063, 0.27),
            Branch((12, 18), 0.76, 0.53, (2, 4)),
            flower("umbel", "da84af d995b6 ebacc7", "dfb6b1", 0.21, 5, 7)),
)
