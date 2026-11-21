"""Bake the supplied terrain and Hero Oak material packs into mipmapped KTX2.

Requires NumPy, ImageMagick 7, and zstd. See crates/world-terrain/README.md.
"""

import argparse
import struct
import subprocess
from pathlib import Path

import numpy as np


KTX2_MAGIC = b"\xabKTX 20\xbb\r\n\x1a\n"


def polyhaven_pack(root, name, resolution):
    textures = root / f"{name}_{resolution}.blend" / "textures"
    return (
        textures / f"{name}_diff_{resolution}.jpg",
        textures / f"{name}_nor_gl_{resolution}.exr",
        textures / f"{name}_rough_{resolution}.exr",
    )


def read_rgb(path, width, height, *, color=False):
    # Convert albedo to linear light for filtering. Normal and roughness samples
    # already represent linear values, including those stored in JPEG files.
    colorspace = ["-colorspace", "RGB"] if color else ["-set", "colorspace", "RGB"]
    pixels = subprocess.check_output([
        "magick", "-limit", "thread", "2", str(path), *colorspace,
        "-filter", "Box", "-resize", f"{width}x{height}!",
        "-depth", "16", "-endian", "LSB", "rgb:-",
    ])
    return np.frombuffer(pixels, dtype="<u2").reshape(height, width, 3) / np.float32(65535)


def mip_chain(pixels, *, srgb=False, normal=False):
    width = pixels.shape[1]
    height = pixels.shape[0]
    while True:
        encoded = pixels.copy()
        if srgb:
            rgb = pixels[:, :, :3]
            encoded[:, :, :3] = np.where(
                rgb <= 0.0031308, rgb * 12.92, 1.055 * rgb ** (1 / 2.4) - 0.055,
            )
        if normal:
            vectors = pixels[:, :, :3] * 2 - 1
            lengths = np.linalg.norm(vectors, axis=2, keepdims=True)
            encoded[:, :, :3] = 0.5 + 0.5 * vectors / np.maximum(lengths, 1e-6)
        yield np.rint(np.clip(encoded, 0, 1) * 255).astype(np.uint8).tobytes()

        if width == height == 1:
            break
        next_width, next_height = max(1, width // 2), max(1, height // 2)
        pixels = pixels.reshape(
            next_height, height // next_height, next_width, width // next_width, 4,
        ).mean(axis=(1, 3))
        width, height = next_width, next_height


def rgba(rgb, alpha):
    return np.concatenate((rgb, np.broadcast_to(alpha, (*rgb.shape[:2], 1))), axis=2)


def read_pack(paths, width, height):
    albedo, normal, roughness = paths
    return (
        rgba(read_rgb(albedo, width, height, color=True), np.float32(1)),
        read_rgb(normal, width, height),
        read_rgb(roughness, width, height)[:, :, :1],
    )


def write_ktx2(path, layers, template, *, srgb=False, normal=False):
    # The shipped arrays supply the descriptors for these same RGBA8 formats.
    header = template.read_bytes()
    vk_format, type_size = struct.unpack_from("<2I", header, 12)
    dfd_offset, dfd_size = struct.unpack_from("<2I", header, 48)
    dfd = header[dfd_offset:dfd_offset + dfd_size]
    levels = [
        b"".join(level)
        for level in zip(*(mip_chain(layer, srgb=srgb, normal=normal) for layer in layers))
    ]
    height, width = layers[0].shape[:2]
    layer_count = len(layers) if len(layers) > 1 else 0
    dfd_offset = 80 + len(levels) * 24
    container = bytearray(KTX2_MAGIC + struct.pack(
        "<13I2Q", vk_format, type_size, width, height, 0, layer_count, 1,
        len(levels), 2,  # Zstandard supercompression
        dfd_offset, dfd_size, 0, 0, 0, 0,
    ))
    container.extend(bytes(len(levels) * 24))
    container.extend(dfd)
    # The level index starts at full resolution. Payloads start at the smallest mip.
    for mip in reversed(range(len(levels))):
        compressed = subprocess.check_output(["zstd", "-q", "-c"], input=levels[mip])
        struct.pack_into("<3Q", container, 80 + mip * 24,
                         len(container), len(compressed), len(levels[mip]))
        container.extend(compressed)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(container)


def build(source, assets):
    terrain = assets / "textures/terrain"
    color_template = terrain / "albedo.ktx2"
    data_template = terrain / "normal-roughness.ktx2"
    ice = source / "Ice003_1K-JPG"
    terrain_packs = (
        polyhaven_pack(source, "gravelly_sand", "2k"),
        polyhaven_pack(source, "rocky_terrain_02", "4k"),
        polyhaven_pack(source, "lichen_rock", "4k"),
        tuple(ice / f"Ice003_1K-JPG_{channel}.jpg"
              for channel in ("Color", "NormalGL", "Roughness")),
    )
    colors, finishes = [], []
    for pack in terrain_packs:
        color, normal, roughness = read_pack(pack, 512, 512)
        colors.append(color)
        finishes.append(rgba(normal, roughness))
    write_ktx2(terrain / "detail-albedo.ktx2", colors, color_template, srgb=True)
    write_ktx2(terrain / "detail-normal-roughness.ktx2", finishes, data_template, normal=True)

    bark = assets / "models/hero_oak/bark"
    color, normal, roughness = read_pack(
        polyhaven_pack(source, "jolcham_oak_bark_01", "4k"), 256, 512,
    )
    metallic_roughness = np.zeros_like(color)
    metallic_roughness[:, :, 1:2] = roughness
    metallic_roughness[:, :, 3] = 1
    write_ktx2(bark / "albedo.ktx2", [color], color_template, srgb=True)
    write_ktx2(bark / "normal.ktx2", [rgba(normal, np.float32(1))], data_template, normal=True)
    write_ktx2(bark / "metallic-roughness.ktx2", [metallic_roughness], data_template)


if __name__ == "__main__":
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, default=root)
    parser.add_argument("--asset-root", type=Path, default=root / "assets")
    args = parser.parse_args()
    build(args.source_root, args.asset_root)
