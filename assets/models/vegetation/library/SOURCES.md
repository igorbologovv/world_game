# Tree library asset sources

The tree meshes and foliage color and normal maps are original procedural outputs
of [the library builder](../../../../tools/build_tree_library.py). The builder
records each tree name and its growth parameters. The meshes and foliage maps
contain no third-party tree geometry or foliage photography. They are supplied
as part of this repository.

The bark maps are the 1K JPEG downloads from Poly Haven. The diffuse, OpenGL
normal and packed ambient-occlusion, roughness and metalness maps are stored
under `textures/<asset-id>/`. The GLBs reference these shared files and use
the green channel of the packed map for roughness.

| Directory | Source | Creator | License |
| --- | --- | --- | --- |
| `pine_bark` | [Pine Bark](https://polyhaven.com/a/pine_bark) | Dimitrios Savva | CC0 1.0 |
| `bark_brown_02` | [Bark Brown 02](https://polyhaven.com/a/bark_brown_02) | Rob Tuytel | CC0 1.0 |
| `japanese_sycamore` | [Japanese Sycamore](https://polyhaven.com/a/japanese_sycamore) | Charlotte Baglioni | CC0 1.0 |
| `palm_tree_bark` | [Palm Tree Bark](https://polyhaven.com/a/palm_tree_bark) | Dimitrios Savva and Rico Cilliers | CC0 1.0 |

[Poly Haven license policy](https://polyhaven.com/license) · [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/)
