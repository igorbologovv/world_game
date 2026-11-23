//! A stable cubemap with render-world uploads spread across frames.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread,
};

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageSampler, ImageSamplerDescriptor},
    prelude::*,
    render::{
        extract_resource::{ExtractResource, ExtractResourcePlugin},
        render_asset::RenderAssets,
        render_resource::{
            Extent3d, Origin3d, TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect,
            TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
        },
        renderer::RenderQueue,
        texture::GpuImage,
        Render, RenderApp, RenderSystems,
    },
};
use world_core::{canopy::ForestCanopy, runtime::WorldAssets, SurfaceHeights};

use super::bake::{self, Mip, FACES, MIPS, PREVIEW_MIP, SIDE};

const BYTES_PER_FRAME: u32 = 1024 * 1024;

#[derive(Clone, Resource, ExtractResource)]
#[extract_app(RenderApp)]
pub(super) struct Globe {
    pub image: Handle<Image>,
    transfer: Arc<Transfer>,
}

struct Transfer {
    pending: Mutex<VecDeque<Mip>>,
    resident: AtomicU32,
}

#[derive(Default)]
struct Cursor {
    mip: Option<Mip>,
    face: u32,
    row: u32,
}

pub(super) fn install(app: &mut App) {
    if app.get_sub_app(RenderApp).is_some() {
        app.add_plugins(ExtractResourcePlugin::<Globe>::default());
        app.sub_app_mut(RenderApp)
            .add_systems(Render, upload.in_set(RenderSystems::PrepareViews));
        app.add_systems(Update, animate);
    }
}

impl Globe {
    pub(super) fn new(images: &mut Assets<Image>) -> Self {
        let mut image = Image::new_uninit(
            Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: FACES,
            },
            TextureDimension::D2,
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::default(),
        );
        image.texture_descriptor.label = Some("orbital forest canopy");
        image.texture_descriptor.mip_level_count = MIPS;
        image.texture_view_descriptor = Some(TextureViewDescriptor {
            dimension: Some(TextureViewDimension::Cube),
            ..default()
        });
        image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
        Self {
            image: images.add(image),
            transfer: Arc::new(Transfer {
                pending: Mutex::new(VecDeque::new()),
                resident: AtomicU32::new(MIPS),
            }),
        }
    }

    pub(super) fn start(&self, assets: &WorldAssets, heights: &SurfaceHeights) {
        let path = assets.directory().to_owned();
        let heights = heights.clone();
        let transfer = self.transfer.clone();
        let worker = thread::Builder::new()
            .name("planet-canopy".into())
            .spawn(move || {
                if let Err(error) = bake::load_or_bake(&path, &heights, |mips| {
                    transfer.pending.lock().unwrap().extend(mips);
                }) {
                    warn!("Orbital forest canopy: {error}");
                }
            });
        if let Err(error) = worker {
            warn!("Could not start orbital canopy worker: {error}");
        }
    }
}

fn animate(time: Res<Time>, globe: Res<Globe>, mut canopy: ResMut<ForestCanopy>) {
    let resident = globe.transfer.resident.load(Ordering::Acquire);
    if resident > PREVIEW_MIP {
        return;
    }
    let fade = canopy.layout.fade;
    // Crossfade adjacent mips down to the finest fully uploaded level.
    let step = time.delta_secs().min(0.05);
    let mip = if fade.w == 0.0 {
        resident as f32
    } else {
        (fade.z - 8.0 * step).max(resident as f32)
    };
    let opacity = (fade.w + 3.0 * step).min(1.0);
    if fade.z != mip || fade.w != opacity {
        canopy.layout.fade.z = mip;
        canopy.layout.fade.w = opacity;
    }
}

fn upload(
    globe: Option<Res<Globe>>,
    images: Res<RenderAssets<GpuImage>>,
    queue: Res<RenderQueue>,
    mut cursor: Local<Cursor>,
) {
    let Some(globe) = globe else { return };
    let Some(image) = images.get(&globe.image) else {
        return;
    };
    let mut remaining = BYTES_PER_FRAME;
    let cursor = &mut *cursor;
    loop {
        if cursor.mip.is_none() {
            cursor.mip = globe.transfer.pending.lock().unwrap().pop_front();
        }
        let Some(mip) = &cursor.mip else { break };
        let side = mip.side();
        let row_bytes = side * 4;
        let rows = (remaining / row_bytes).min(side - cursor.row);
        if rows == 0 {
            break;
        }
        let offset = ((cursor.face * side + cursor.row) * row_bytes) as usize;
        let size = rows * row_bytes;
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &image.texture,
                mip_level: mip.level,
                origin: Origin3d {
                    x: 0,
                    y: cursor.row,
                    z: cursor.face,
                },
                aspect: TextureAspect::All,
            },
            &mip.bytes[offset..offset + size as usize],
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(rows),
            },
            Extent3d {
                width: side,
                height: rows,
                depth_or_array_layers: 1,
            },
        );
        remaining -= size;
        cursor.row += rows;
        if cursor.row == side {
            cursor.face += 1;
            cursor.row = 0;
        }
        if cursor.face == FACES {
            globe.transfer.resident.store(mip.level, Ordering::Release);
            *cursor = Cursor::default();
        }
    }
}
