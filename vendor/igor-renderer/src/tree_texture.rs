use glam::{Vec2, Vec4};
use gltf::texture::WrappingMode;
use std::{
    collections::HashMap,
    error::Error,
    path::{Path, PathBuf},
};

const PAGE: u32 = 2064; // Four 512px or two 1024px textures, with borders.
pub(crate) type TextureRect = [u32; 4];

#[derive(Clone, Copy)]
enum Wrap {
    Repeat,
    Clamp,
    Mirror,
}

impl From<WrappingMode> for Wrap {
    fn from(mode: WrappingMode) -> Self {
        match mode {
            WrappingMode::Repeat => Self::Repeat,
            WrappingMode::ClampToEdge => Self::Clamp,
            WrappingMode::MirroredRepeat => Self::Mirror,
        }
    }
}

impl Wrap {
    fn texel(self, i: i32, size: u32) -> u32 {
        let size = size as i32;
        (match self {
            Self::Repeat => i.rem_euclid(size),
            Self::Clamp => i.clamp(0, size - 1),
            Self::Mirror => {
                let i = i.rem_euclid(size * 2);
                i.min(size * 2 - 1 - i)
            }
        }) as u32
    }
}

struct Texture {
    pixels: image::RgbaImage,
    origin: [u32; 2],
    layer: u32,
    wrap: [Wrap; 2],
}

impl Texture {
    fn texel(&self, x: i32, y: i32) -> [u8; 4] {
        let x = self.wrap[0].texel(x, self.pixels.width());
        let y = self.wrap[1].texel(y, self.pixels.height());
        self.pixels.get_pixel(x, y).0
    }
}

#[derive(Default)]
pub(crate) struct TextureAtlas {
    textures: Vec<Texture>,
    sources: HashMap<(PathBuf, Option<usize>, u32), TextureRect>,
    layer: u32,
    x: u32,
    y: u32,
    row_height: u32,
}

impl TextureAtlas {
    pub(crate) fn add(
        &mut self,
        model_path: &Path,
        blob: &[u8],
        texture: gltf::Texture<'_>,
    ) -> Result<TextureRect, Box<dyn Error>> {
        use gltf::image::Source;
        let wrap = [
            texture.sampler().wrap_s().into(),
            texture.sampler().wrap_t().into(),
        ];
        let flags = wrap[0] as u32 | (wrap[1] as u32) << 2;
        let source = texture.source();
        let key = match source.source() {
            Source::Uri { uri, .. } => {
                (model_path.with_file_name(uri).canonicalize()?, None, flags)
            }
            Source::View { .. } => (model_path.to_owned(), Some(source.index()), flags),
        };
        if let Some(&rect) = self.sources.get(&key) {
            return Ok(rect);
        }
        let pixels = match source.source() {
            Source::Uri { .. } => image::open(&key.0)?,
            Source::View { view, .. } => {
                image::load_from_memory(&blob[view.offset()..view.offset() + view.length()])?
            }
        }
        .into_rgba8();
        let rect=self.insert_pixels(pixels,wrap)?;
        self.sources.insert(key,rect);
        Ok(rect)
    }

    pub(crate) fn add_image(&mut self, input:crate::planetary::InputImage, repeat:bool)->Result<TextureRect,Box<dyn Error>> {
        let pixels=image::RgbaImage::from_raw(input.width,input.height,input.rgba).ok_or("invalid input texture")?;
        self.insert_pixels(pixels,[if repeat{Wrap::Repeat}else{Wrap::Clamp};2])
    }
    fn insert_pixels(&mut self,mut pixels:image::RgbaImage,wrap:[Wrap;2])->Result<TextureRect,Box<dyn Error>>{
        let flags=wrap[0] as u32|(wrap[1] as u32)<<2;
        // Explicit memory/quality policy; 2048 retains the original textures.
        let max_size = std::env::var("IGOR_TEXTURE_MAX").ok().and_then(|s| s.parse::<u32>().ok()).unwrap_or(512).clamp(64,2048);
        let (w0,h0)=pixels.dimensions();
        if w0.max(h0)>max_size {
            let scale=max_size as f32/w0.max(h0) as f32;
            pixels=image::imageops::resize(&pixels,(w0 as f32*scale).round().max(1.0) as u32,(h0 as f32*scale).round().max(1.0) as u32,image::imageops::FilterType::Triangle);
        }
        let (width, height) = pixels.dimensions();
        let (w, h) = (width + 2, height + 2);
        if w > PAGE || h > PAGE {
            return Err("tree texture exceeds atlas page size".into());
        }
        if self.x + w > PAGE {
            self.x = 0;
            self.y += self.row_height;
            self.row_height = 0;
        }
        if self.y + h > PAGE {
            self.x = 0;
            self.y = 0;
            self.row_height = 0;
            self.layer += 1;
        }
        let rect = [
            self.textures.len() as u32 + 1,
            (self.x + 1) | (self.y + 1) << 16,
            width | height << 16,
            self.layer | flags << 16,
        ];
        self.textures.push(Texture {
            pixels,
            origin: [self.x, self.y],
            layer: self.layer,
            wrap,
        });
        self.x += w;
        self.row_height = self.row_height.max(h);
        Ok(rect)
    }

    pub(crate) fn sample(&self, rect: TextureRect, uv: Vec2, srgb: bool) -> Vec4 {
        let texture = &self.textures[rect[0] as usize - 1];
        let size = Vec2::new(
            texture.pixels.width() as f32,
            texture.pixels.height() as f32,
        );
        let xy = uv * size - Vec2::splat(0.5);
        let lo = xy.floor();
        let f = xy - lo;
        let (x, y) = (lo.x as i32, lo.y as i32);
        let pixel = |dx, dy| {
            let [r, g, b, a] = texture.texel(x + dx, y + dy).map(|v| v as f32 / 255.0);
            let linear = |c: f32| {
                if !srgb {
                    c
                } else if c <= 0.04045 {
                    c / 12.92
                } else {
                    ((c + 0.055) / 1.055).powf(2.4)
                }
            };
            Vec4::new(linear(r), linear(g), linear(b), a)
        };
        pixel(0, 0)
            .lerp(pixel(1, 0), f.x)
            .lerp(pixel(0, 1).lerp(pixel(1, 1), f.x), f.y)
    }

    pub(crate) fn gpu_bytes(&self) -> u64 { u64::from(PAGE)*u64::from(PAGE)*4*u64::from(self.layer+1) }

    pub(crate) fn upload_data(&self)->crate::render_api::AtlasUpload{
        let mut tiles=Vec::with_capacity(self.textures.len());
        for texture in &self.textures{
            let (width,height)=texture.pixels.dimensions();
            let mut rgba=Vec::with_capacity(((width+2)*(height+2)*4) as usize);
            for y in -1..=height as i32{
                for x in -1..=width as i32{rgba.extend(texture.texel(x,y));}
            }
            tiles.push(crate::render_api::AtlasTile{origin:[texture.origin[0],texture.origin[1],texture.layer],size:[width+2,height+2],rgba});
        }
        crate::render_api::AtlasUpload{side:PAGE,layers:self.layer+1,tiles}
    }
    #[cfg(feature="gpu")]
    pub(crate) fn upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> [wgpu::TextureView; 2] {
        use wgpu::TextureFormat::{Rgba8Unorm, Rgba8UnormSrgb};
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tree color and normal atlas"),
            size: wgpu::Extent3d {
                width: PAGE,
                height: PAGE,
                depth_or_array_layers: self.layer + 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[Rgba8UnormSrgb],
        });
        for texture in &self.textures {
            let (width, height) = texture.pixels.dimensions();
            let mut pixels = Vec::with_capacity(((width + 2) * (height + 2)) as usize);
            // A one-texel gutter makes bilinear filtering work at wrap seams.
            for y in -1..=height as i32 {
                for x in -1..=width as i32 {
                    pixels.push(texture.texel(x, y));
                }
            }
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: texture.origin[0],
                        y: texture.origin[1],
                        z: texture.layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&pixels),
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some((width + 2) * 4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: width + 2,
                    height: height + 2,
                    depth_or_array_layers: 1,
                },
            );
        }
        [Rgba8UnormSrgb, Rgba8Unorm].map(|format| {
            atlas.create_view(&wgpu::TextureViewDescriptor {
                format: Some(format),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        })
    }
}
