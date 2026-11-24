# Ocean normal layers

`wave-normals.ktx2` is a 512×512 linear RGBA8 texture with averaged slope mipmaps. The ocean samples it in three warped projections for ripples and foam breakup. Animated vertex displacement and breaking surf come from the procedural wave field evaluated against the existing ocean surface and bathymetry.

- **RG** stores the broad wave slopes from frame `000.png` of `waves5.zip`. The heightmap was filtered with a wrapping 3-tap kernel before taking its centered differences. Source and CC0 license by [zookeeper on OpenGameArt](https://opengameart.org/content/seamless-looping-waves-heightmaps).
- **BA** stores the fine wave slopes from `water-ocean-normal-map-1k/normal.png`, box-filtered to 512×512 and converted to positive UV-axis slopes. Source and CC0 license by [ProcTexture](https://proctexture.com/textures/water/normal-maps/ocean-normal-map).
