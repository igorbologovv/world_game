"""Bake tileable freshwater tangent slopes and surface breakup.

Requires NumPy and zstd. Run from the workspace root.
"""

from pathlib import Path

import numpy as np

from build_surface_textures import write_ktx2


def filtered_noise(random, size, frequency, anisotropy=1.0):
    frequency_x = np.fft.rfftfreq(size) * size
    frequency_y = np.fft.fftfreq(size)[:, None] * size
    radius = np.hypot(frequency_x / anisotropy, frequency_y)
    band = np.exp(-0.5 * ((radius - frequency) / (0.47 * frequency)) ** 2)
    band[0, 0] = 0
    noise = np.fft.irfft2(np.fft.rfft2(random.standard_normal((size, size))) * band, s=(size, size), axes=(0, 1))
    return noise / noise.std()


def build():
    size = 256
    random = np.random.default_rng(7214)
    ripple = filtered_noise(random, size, 7, 0.58) + 0.36 * filtered_noise(random, size, 19)
    slope = [
        (np.roll(ripple, -1, axis) - np.roll(ripple, 1, axis)) * 0.5
        for axis in (1, 0)
    ]
    foam = filtered_noise(random, size, 10) + 0.25 * filtered_noise(random, size, 27)
    foam = np.exp(-np.abs(foam) * 1.7)
    sediment = filtered_noise(random, size, 4) + 0.25 * filtered_noise(random, size, 13)
    pixels = np.stack([
        0.5 + 0.16 * slope[0] / slope[0].std(),
        0.5 + 0.16 * slope[1] / slope[1].std(),
        foam,
        0.5 + 0.17 * sediment,
    ], axis=-1).clip(0, 1)
    assets = Path("assets/textures")
    write_ktx2(
        assets / "water/freshwater.ktx2", [pixels],
        assets / "terrain/normal-roughness.ktx2",
    )


if __name__ == "__main__":
    build()
