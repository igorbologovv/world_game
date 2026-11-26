use crate::frame_counter::FrameCounter;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use igor_renderer::Scene;
use std::{collections::HashSet, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Instant, Duration}};
use wgpu::util::DeviceExt;
use winit::{
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::WindowBuilder,
};

#[derive(Clone)]
pub struct Camera {
    pub position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}
impl Camera {
    pub fn new(scene: &Scene, aerial: bool) -> Self {
        let e = scene.extent();
        Self {
            position: Vec3::new(e.x * 0.25, if aerial { 110.0 } else { 2.7 }, e.z * 0.25),
            yaw: 0.68,
            pitch: if aerial { -0.55 } else { 0.02 },
        }
    }
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let f = Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        );
        let r = Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin());
        let u = f.cross(r);
        (f, r, u)
    }
}
#[derive(Clone, Copy)]
struct ViewHistory { position:Vec3, forward:Vec3, right:Vec3, up:Vec3, moving:bool }
impl ViewHistory {
    fn capture(c:&Camera,moving:bool)->Self {let (forward,right,up)=c.basis();Self{position:c.position,forward,right,up,moving}}
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    origin: [f32; 4],
    forward: [f32; 4],
    right: [f32; 4],
    up: [f32; 4],
    screen: [f32; 4],
    levels: [[u32; 4]; 3],
    flags: [u32; 4],
    hud: [[u32; 4]; 8],
    prev_origin:[f32;4], prev_forward:[f32;4], prev_right:[f32;4], prev_up:[f32;4], history:[u32;4],
}
impl Uniform {
    fn new(c: &Camera, w: u32, h: u32, scene: &Scene, skip: bool, proxy: bool, counter: &FrameCounter) -> Self {
        let (f, r, u) = c.basis();
        Self {
            origin: c.position.extend(0.0).to_array(),
            forward: f.extend(0.0).to_array(),
            right: r.extend(0.0).to_array(),
            up: u.extend(0.0).to_array(),
            screen: [
                w as f32,
                h as f32,
                2.0 * (60f32.to_radians() / 2.0).tan() / h.max(1) as f32,
                0.0,
            ],
            levels: scene.levels.map(|l| l.uniform()),
            flags: [skip as u32, proxy as u32, 0, 0],
            hud: counter.glyphs(),
            prev_origin:[0.0;4],prev_forward:[0.0;4],prev_right:[0.0;4],prev_up:[0.0;4],history:[0;4],
        }
    }
}

struct FrameTargets { textures:[wgpu::Texture;2], views:[wgpu::TextureView;2] }
impl FrameTargets {
    fn new(device:&wgpu::Device,w:u32,h:u32)->Self {
        let textures=std::array::from_fn(|_|device.create_texture(&wgpu::TextureDescriptor{
            label:Some("ping-pong linear color and depth"),size:wgpu::Extent3d{width:w,height:h,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba16Float,usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::STORAGE_BINDING,view_formats:&[],
        }));
        let views=std::array::from_fn(|i|textures[i].create_view(&wgpu::TextureViewDescriptor::default()));
        Self{textures,views}
    }
}

fn frame_binds(device:&wgpu::Device,compute:&wgpu::ComputePipeline,present:&wgpu::RenderPipeline,uniform:&wgpu::Buffer,buffers:&[wgpu::Buffer;7],acceleration:&wgpu::Buffer,atlases:&[wgpu::TextureView;2],sampler:&wgpu::Sampler,targets:&FrameTargets)->([wgpu::BindGroup;2],[wgpu::BindGroup;2]) {
    let compute_binds=std::array::from_fn(|i|{
        let mut e=vec![wgpu::BindGroupEntry{binding:0,resource:uniform.as_entire_binding()}];
        e.extend(buffers.iter().enumerate().map(|(j,b)|wgpu::BindGroupEntry{binding:j as u32+1,resource:b.as_entire_binding()}));
        e.extend([
            wgpu::BindGroupEntry{binding:8,resource:wgpu::BindingResource::TextureView(&atlases[0])},
            wgpu::BindGroupEntry{binding:9,resource:wgpu::BindingResource::TextureView(&atlases[1])},
            wgpu::BindGroupEntry{binding:10,resource:wgpu::BindingResource::Sampler(sampler)},
            wgpu::BindGroupEntry{binding:11,resource:acceleration.as_entire_binding()},
            wgpu::BindGroupEntry{binding:12,resource:wgpu::BindingResource::TextureView(&targets.views[1-i])},
            wgpu::BindGroupEntry{binding:13,resource:wgpu::BindingResource::TextureView(&targets.views[i])},
        ]);
        device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("packet ping-pong"),layout:&compute.get_bind_group_layout(0),entries:&e})
    });
    let present_binds=std::array::from_fn(|i|device.create_bind_group(&wgpu::BindGroupDescriptor{
        label:Some("display ping-pong"),layout:&present.get_bind_group_layout(0),entries:&[
            wgpu::BindGroupEntry{binding:0,resource:uniform.as_entire_binding()},
            wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&targets.views[i])},
        ]
    }));
    (compute_binds,present_binds)
}

pub fn run(
    scene: Scene,
    aerial: bool,
    width: u32,
    height: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let event_loop = EventLoop::new()?;
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Igor Renderer")
            .with_inner_size(winit::dpi::PhysicalSize::new(width, height))
            .with_min_inner_size(winit::dpi::PhysicalSize::new(96, 32))
            .build(&event_loop)?,
    );
    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(window.clone())?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }))
    .ok_or("no compatible GPU")?;
    println!("GPU: {:?}", adapter.get_info());
    let geometry = &scene.geometry;
    let buffers: [(&str, &[u8]); 7] = [
        ("flat block grids", bytemuck::cast_slice(&scene.blocks)),
        (
            "tree batch directory",
            bytemuck::cast_slice(&geometry.batches),
        ),
        ("surfel templates", bytemuck::cast_slice(&geometry.surfels)),
        (
            "surfel material coordinates",
            bytemuck::cast_slice(&geometry.patches),
        ),
        ("tree materials", bytemuck::cast_slice(&geometry.materials)),
        (
            "tree bin offsets",
            bytemuck::cast_slice(&geometry.bin_offsets),
        ),
        (
            "tree bin indices",
            bytemuck::cast_slice(&geometry.bin_indices),
        ),
    ];
    let accel_bytes: &[u8] = bytemuck::cast_slice(&geometry.acceleration);
    let total = scene.bytes() as u64 + geometry.texture_bytes();
    let budget = std::env::var("IGOR_GPU_BUDGET_MIB").ok().and_then(|s|s.parse::<u64>().ok()).unwrap_or(1536);
    println!("GPU scene estimate: {:.1} MiB buffers + {:.1} MiB atlas = {:.1} MiB; budget {} MiB",
        scene.bytes() as f64/1048576.0, geometry.texture_bytes() as f64/1048576.0, total as f64/1048576.0,budget);
    if total > budget.saturating_mul(1048576) {
        return Err("scene exceeds configured GPU budget; lower IGOR_TEXTURE_MAX or use a higher explicit IGOR_GPU_BUDGET_MIB if VRAM permits".into());
    }
    let limits = adapter.limits();
    if limits.max_storage_buffers_per_shader_stage < 8 {return Err("GPU needs eight storage buffers per shader stage".into());}
    let largest = u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
    for (label, bytes) in buffers.into_iter().chain(std::iter::once(("local acceleration and LOD",accel_bytes))) {
        if bytes.len() as u64 > largest {
            return Err(format!(
                "{label} requires {} MiB, GPU buffer limit is {} MiB",
                bytes.len() as u64 / (1 << 20),
                largest / (1 << 20)
            )
            .into());
        }
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("Igor Renderer"),
            required_features: wgpu::Features::empty(),
            required_limits: limits,
        },
        None,
    ))?;
    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let size = window.inner_size();
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 1,
    };
    surface.configure(&device, &config);
    let mut camera = Camera::new(&scene, aerial);
    let mut skip = true;
    let mut proxy = std::env::var("IGOR_FULL_DETAIL").as_deref()!=Ok("1");
    println!("L: toggle approximate distant LOD; F: toggle exact empty-space skips; proxy={proxy}");
    let mut counter = FrameCounter::default();
    let mut packet=std::env::var("IGOR_PACKET").as_deref()!=Ok("0");
    let mut temporal=std::env::var("IGOR_TEMPORAL").as_deref()!=Ok("0");
    let mut legacy=std::env::var("IGOR_LEGACY").as_deref()==Ok("1");
    let mut previous:Option<ViewHistory>=None;
    let mut phase=0u32;
    let mut target_index=0usize;
    println!("P: packet traversal; T: temporal reuse; B: old fragment path; packet={packet}, temporal={temporal}, legacy={legacy}");

    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("camera"),
        contents: bytemuck::bytes_of(&Uniform::new(
            &camera,
            config.width,
            config.height,
            &scene,
            skip,
            proxy,
            &counter,
        )),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let upload = |label, data: &[u8]| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: data,
            usage: wgpu::BufferUsages::STORAGE,
        })
    };
    let gpu_buffers = buffers.map(|(label, data)| upload(label, data));
    let acceleration = upload("local acceleration and LOD", accel_bytes);
    let atlases = geometry.upload_textures(&device, &queue);
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("bilinear tree materials"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("point ray casting"),
        source: wgpu::ShaderSource::Wgsl(include_str!("trace.wgsl").into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("full screen ray pass"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
    });
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: uniform.as_entire_binding(),
    }];
    entries.extend(
        gpu_buffers
            .iter()
            .enumerate()
            .map(|(i, buffer)| wgpu::BindGroupEntry {
                binding: i as u32 + 1,
                resource: buffer.as_entire_binding(),
            }),
    );
    entries.extend([
        wgpu::BindGroupEntry { binding: 11, resource: acceleration.as_entire_binding() },
        wgpu::BindGroupEntry {
            binding: 8,
            resource: wgpu::BindingResource::TextureView(&atlases[0]),
        },
        wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::TextureView(&atlases[1]),
        },
        wgpu::BindGroupEntry {
            binding: 10,
            resource: wgpu::BindingResource::Sampler(&sampler),
        },
    ]);
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("scene"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let packet_source=concat!(include_str!("trace.wgsl"),"\n",include_str!("packet.wgsl"));
    let compute_shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("packet ray casting"),source:wgpu::ShaderSource::Wgsl(packet_source.into())});
    let compute=device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some("8x8 packet ray casting"),layout:None,module:&compute_shader,entry_point:"packet_cs"});
    let present_shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("display cached frame"),source:wgpu::ShaderSource::Wgsl(include_str!("present.wgsl").into())});
    let present=device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{
        label:Some("display packet result"),layout:None,vertex:wgpu::VertexState{module:&present_shader,entry_point:"vs",buffers:&[]},
        fragment:Some(wgpu::FragmentState{module:&present_shader,entry_point:"fs",targets:&[Some(wgpu::ColorTargetState{format,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
        primitive:wgpu::PrimitiveState::default(),depth_stencil:None,multisample:wgpu::MultisampleState::default(),multiview:None,
    });
    let mut targets=FrameTargets::new(&device,config.width,config.height);
    let (mut compute_binds,mut present_binds)=frame_binds(&device,&compute,&present,&uniform,&gpu_buffers,&acceleration,&atlases,&sampler,&targets);
    let mut keys = HashSet::new();
    let mut last = Instant::now();
    let mut drawable = true;
    let mut title_time=Instant::now();
    let mut last_mode="initial";
    let mut gpu_start=Instant::now();
    let mut gpu_elapsed=0.0f64;
    let complete = Arc::new(AtomicBool::new(true));
    let mut pending = false;
    let mut next_frame = Instant::now();
    let cooldown = Duration::from_millis(std::env::var("IGOR_GPU_COOLDOWN_MS").ok().and_then(|s|s.parse().ok()).unwrap_or(16u64));
    event_loop.run(move |event, elwt| {
        elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now()+Duration::from_millis(8)));
        match event {
            Event::WindowEvent { event, window_id } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Focused(false) => keys.clear(),
                WindowEvent::KeyboardInput { event, .. } => {
                    if let PhysicalKey::Code(k) = event.physical_key {
                        if event.state == ElementState::Pressed {
                            keys.insert(k);
                            if k == KeyCode::Escape {
                                elwt.exit();
                            }
                            if !event.repeat && k == KeyCode::KeyF {skip=!skip;previous=None;}
                            if !event.repeat && k == KeyCode::KeyP {packet=!packet;previous=None;counter=FrameCounter::default();println!("packet={packet}");}
                            if !event.repeat && k == KeyCode::KeyT {temporal=!temporal;previous=None;counter=FrameCounter::default();println!("temporal={temporal}");}
                            if !event.repeat && k == KeyCode::KeyB {legacy=!legacy;previous=None;counter=FrameCounter::default();println!("legacy={legacy}");}
                            if !event.repeat && k == KeyCode::KeyL { proxy = !proxy; previous=None; println!("approximate far LOD={proxy}"); }
                            if !event.repeat && k == KeyCode::KeyR {
                                camera = Camera::new(&scene, camera.position.y < 30.0);previous=None;
                            }
                        } else {
                            keys.remove(&k);
                        }
                    }
                }
                WindowEvent::Resized(size) => {
                    drawable = size.width > 0 && size.height > 0;
                    if drawable {
                        config.width = size.width;
                        config.height = size.height;
                        surface.configure(&device, &config);
                        targets=FrameTargets::new(&device,config.width,config.height);
                        (compute_binds,present_binds)=frame_binds(&device,&compute,&present,&uniform,&gpu_buffers,&acceleration,&atlases,&sampler,&targets);
                        previous=None;target_index=0;
                    } else {
                        counter = FrameCounter::default();previous=None;
                    }
                }
                WindowEvent::RedrawRequested if drawable && !pending && Instant::now()>=next_frame => {
                    let now = Instant::now();
                    let dt = (now - last).as_secs_f32().min(0.1);
                    last = now;
                    let held = |k| keys.contains(&k);
                    let speed = if held(KeyCode::ControlLeft) {
                        120.0
                    } else {
                        16.0
                    };
                    if held(KeyCode::ArrowLeft) {
                        camera.yaw -= dt * 1.5;
                    }
                    if held(KeyCode::ArrowRight) {
                        camera.yaw += dt * 1.5;
                    }
                    if held(KeyCode::ArrowUp) {
                        camera.pitch += dt * 1.5;
                    }
                    if held(KeyCode::ArrowDown) {
                        camera.pitch -= dt * 1.5;
                    }
                    camera.pitch = camera.pitch.clamp(-1.5, 1.5);
                    let (f, r, _) = camera.basis();
                    for (k, v) in [
                        (KeyCode::KeyW, f),
                        (KeyCode::KeyS, -f),
                        (KeyCode::KeyD, r),
                        (KeyCode::KeyA, -r),
                        (KeyCode::Space, Vec3::Y),
                        (KeyCode::ShiftLeft, Vec3::NEG_Y),
                    ] {
                        if held(k) {
                            camera.position += v * speed * dt;
                        }
                    }
                    let mut u=Uniform::new(&camera,config.width,config.height,&scene,skip,proxy,&counter);
                    u.flags[2]=packet as u32;
                    let mut current=ViewHistory::capture(&camera,false);
                    if let Some(old)=previous {
                        let unchanged=current.position==old.position && current.forward==old.forward;
                        current.moving=!unchanged;
                        if temporal && !legacy {
                            u.prev_origin=old.position.extend(0.0).to_array();
                            u.prev_forward=old.forward.extend(0.0).to_array();
                            u.prev_right=old.right.extend(0.0).to_array();
                            u.prev_up=old.up.extend(0.0).to_array();
                            let small_motion=current.position.distance(old.position)<2.0 && current.forward.dot(old.forward)>0.995;
                            // First stationary frame after movement is fully refreshed.
                            u.history=[(!unchanged&&small_motion) as u32,(unchanged&&!old.moving) as u32,phase,0];
                        }
                    }
                    last_mode=if legacy {"legacy"} else if u.history[1]!=0 {"cached"} else if u.history[0]!=0 {"reproject"} else if packet {"packet"} else {"scalar compute"};
                    queue.write_buffer(&uniform,0,bytemuck::bytes_of(&u));
                    let frame = match surface.get_current_texture() {
                        Ok(f) => f,
                        Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                            surface.configure(&device, &config);
                            return;
                        }
                        Err(wgpu::SurfaceError::Timeout) => return,
                        Err(wgpu::SurfaceError::OutOfMemory) => {
                            elwt.exit();
                            return;
                        }
                    };
                    let view = frame
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    let mut encoder = device
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
                    if !legacy {
                        let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("packet and temporal rays"),timestamp_writes:None});
                        pass.set_pipeline(&compute);
                        pass.set_bind_group(0,&compute_binds[target_index],&[]);
                        pass.dispatch_workgroups((config.width+7)/8,(config.height+7)/8,1);
                    }
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            occlusion_query_set: None,
                            timestamp_writes: None,
                        });
                        if legacy {
                            pass.set_pipeline(&pipeline);pass.set_bind_group(0,&bind,&[]);
                        }else{
                            pass.set_pipeline(&present);pass.set_bind_group(0,&present_binds[target_index],&[]);
                        }
                        pass.draw(0..3, 0..1);
                    }
                    complete.store(false,Ordering::Release);
                    gpu_start=Instant::now();
                    queue.submit(Some(encoder.finish()));
                    if legacy {previous=None;}else{previous=Some(current);target_index=1-target_index;phase=phase.wrapping_add(1);}
                    frame.present();
                    pending=true;
                    let done=complete.clone();
                    queue.on_submitted_work_done(move || {done.store(true,Ordering::Release);});
                }
                _ => {}
            },
            Event::AboutToWait => {
                // Never queue another expensive frame while GPU work is pending.
                // Poll without blocking the window's event loop.
                device.poll(wgpu::Maintain::Poll);
                if pending && complete.load(Ordering::Acquire) {
                    pending=false;
                    let now=Instant::now();
                    counter.presented(now);
                    gpu_elapsed=(now-gpu_start).as_secs_f64()*1000.0;
                    next_frame=now+cooldown;
                    if title_time.elapsed()>=Duration::from_secs(1) {
                        window.set_title(&format!("Igor Renderer | {last_mode} | packet={packet} temporal={temporal} | submit-to-completion {gpu_elapsed:.1} ms | P T B"));
                        title_time=now;
                    }
                }
                if drawable && !pending && Instant::now()>=next_frame {window.request_redraw();}
            }
            _ => {}
        }
    })?;
    Ok(())
}
