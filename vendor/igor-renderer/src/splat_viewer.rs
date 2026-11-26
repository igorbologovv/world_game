use crate::{frame_counter::FrameCounter,viewer::Camera};
use igor_renderer::compute_splat::{self,ComputeSplats,Resources,HistoryCamera,SceneBuffers};
use igor_renderer::render_api::FrameUniform as Uniform;
use glam::Vec3;
use igor_renderer::{Scene,splat::{PointScene,Instance,View}};
use std::{collections::HashSet,sync::{Arc,atomic::{AtomicBool,Ordering}},time::{Instant,Duration}};
use winit::{event::{Event,WindowEvent,ElementState},event_loop::{EventLoop,ControlFlow},keyboard::{PhysicalKey,KeyCode},window::WindowBuilder};

fn depth(device:&wgpu::Device,w:u32,h:u32)->wgpu::TextureView{
    device.create_texture(&wgpu::TextureDescriptor{label:Some("splat Z buffer"),size:wgpu::Extent3d{width:w,height:h,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Depth32Float,usage:wgpu::TextureUsages::RENDER_ATTACHMENT,view_formats:&[]}).create_view(&wgpu::TextureViewDescriptor::default())
}
fn env<T:std::str::FromStr>(name:&str,default:T)->T {std::env::var(name).ok().and_then(|s|s.parse().ok()).unwrap_or(default)}
pub fn run(scene:Scene,aerial:bool,width:u32,height:u32)->Result<(),Box<dyn std::error::Error>>{
    let start=Instant::now();
    let mut camera=Camera::new(&scene,aerial);
    // Save reset positions before dropping the large original CPU geometry.
    let ground_camera=Camera::new(&scene,false);
    let aerial_camera=Camera::new(&scene,true);
    let points=PointScene::build(&scene);
    println!("Point hierarchy: {} textured raw points, {} LOD/world records, {:.1} MiB, built in {:.2?}",points.raw.len(),points.lod.len(),points.bytes() as f64/1048576.0,start.elapsed());
    let event_loop=EventLoop::new()?;
    let window=Arc::new(WindowBuilder::new().with_title("Igor Renderer - direct splats").with_inner_size(winit::dpi::PhysicalSize::new(width,height)).with_min_inner_size(winit::dpi::PhysicalSize::new(96,32)).build(&event_loop)?);
    let instance=wgpu::Instance::default();
    let surface=instance.create_surface(window.clone())?;
    let adapter=pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions{power_preference:wgpu::PowerPreference::HighPerformance,compatible_surface:Some(&surface),force_fallback_adapter:false})).ok_or("no GPU")?;
    println!("GPU: {:?}",adapter.get_info());
    let limits=adapter.limits();
    // One instance record per visible tree or selected world cell, not per point.
    let max_instances=points.max_instances().max(1);
    let instance_bytes=max_instances as u64*std::mem::size_of::<Instance>() as u64;
    let raw_bytes=(points.raw.len()*16).max(16) as u64;
    let lod_bytes=(points.lod.len()*16).max(16) as u64;
    let largest=u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
    let patch_bytes=std::mem::size_of_val(scene.geometry.patches.as_slice()) as u64;
    let material_bytes=std::mem::size_of_val(scene.geometry.materials.as_slice()) as u64;
    let atlas_bytes=scene.geometry.texture_bytes();
    if [instance_bytes,raw_bytes,lod_bytes,patch_bytes,material_bytes].into_iter().any(|n|n>largest){return Err(format!("storage buffer exceeds device limit of {largest} bytes").into());}
    let budget=env("IGOR_GPU_BUDGET_MIB",1536u64).saturating_mul(1048576);
    let choice_bytes=max_instances as u64*std::mem::size_of::<igor_renderer::splat::GpuChoice>() as u64;
    if choice_bytes>largest{return Err("GPU LOD candidate buffer exceeds device limit; try IGOR_BRICK_LOD=0".into());}
    let shadow_start=Instant::now();
    let shadow_field=points.shadow_field(limits.max_texture_dimension_2d,env("IGOR_SHADOW_METERS",0.75f32).clamp(0.5,8.0));
    let shadow_bytes=shadow_field.pixels.len() as u64;
    let shadow_transform=shadow_field.transform;
    println!("Static canopy shadow field: {}x{}, {:.1} MiB in {:.2?}",shadow_field.width,shadow_field.height,shadow_bytes as f64/1048576.0,shadow_start.elapsed());
    let fixed_bytes=raw_bytes+lod_bytes+instance_bytes+patch_bytes+material_bytes+atlas_bytes+choice_bytes+shadow_bytes;
    let screen_size=compute_splat::screen_bytes(width,height,max_instances as u64);
    if screen_size>largest{return Err("occlusion screen buffer exceeds device limit".into());}
    let total=fixed_bytes+screen_size+width as u64*height as u64*24;
    if limits.max_storage_buffers_per_shader_stage<8{return Err("compute renderer requires eight storage buffers per stage".into());}
    println!("Direct splat GPU allocation estimate: {:.1} MiB including maximum instance buffer, texture atlas, surface metadata and depth",total as f64/1048576.0);
    if total>budget {return Err("point hierarchy exceeds IGOR_GPU_BUDGET_MIB".into());}
    let (device,queue)=pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor{label:Some("direct splats"),required_features:wgpu::Features::empty(),required_limits:limits},None))?;
    let caps=surface.get_capabilities(&adapter);
    let format=caps.formats.iter().copied().find(|f|f.is_srgb()).unwrap_or(caps.formats[0]);
    let size=window.inner_size();
    let mut config=wgpu::SurfaceConfiguration{usage:wgpu::TextureUsages::RENDER_ATTACHMENT,format,width:size.width.max(1),height:size.height.max(1),present_mode:wgpu::PresentMode::Fifo,alpha_mode:caps.alpha_modes[0],view_formats:vec![],desired_maximum_frame_latency:1};
    surface.configure(&device,&config);
    let mut depth_view=depth(&device,config.width,config.height);
    let SceneBuffers{uniform,raw,lod,instances,patches,materials,atlases,sampler,shadow_view}
        =SceneBuffers::upload(&device,&queue,&scene,&points,&shadow_field);
    drop(shadow_field);
    drop(scene);
    let storage=|binding,visibility|wgpu::BindGroupLayoutEntry{binding,visibility,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None};
    let texture=|binding|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None};
    let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("splat scene"),entries:&[
        wgpu::BindGroupLayoutEntry{binding:0,visibility:wgpu::ShaderStages::VERTEX_FRAGMENT,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
        storage(1,wgpu::ShaderStages::VERTEX),storage(2,wgpu::ShaderStages::VERTEX),storage(3,wgpu::ShaderStages::VERTEX),
        storage(4,wgpu::ShaderStages::VERTEX_FRAGMENT),storage(5,wgpu::ShaderStages::FRAGMENT),
        texture(6),texture(7),wgpu::BindGroupLayoutEntry{binding:8,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None}]});
    let pipeline_layout=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:None,bind_group_layouts:&[&layout],push_constant_ranges:&[]});
    let bind=device.create_bind_group(&wgpu::BindGroupDescriptor{label:None,layout:&layout,entries:&[
        wgpu::BindGroupEntry{binding:0,resource:uniform.as_entire_binding()},wgpu::BindGroupEntry{binding:1,resource:raw.as_entire_binding()},wgpu::BindGroupEntry{binding:2,resource:lod.as_entire_binding()},wgpu::BindGroupEntry{binding:3,resource:instances.as_entire_binding()},
        wgpu::BindGroupEntry{binding:4,resource:patches.as_entire_binding()},wgpu::BindGroupEntry{binding:5,resource:materials.as_entire_binding()},
        wgpu::BindGroupEntry{binding:6,resource:wgpu::BindingResource::TextureView(&atlases[0])},wgpu::BindGroupEntry{binding:7,resource:wgpu::BindingResource::TextureView(&atlases[1])},wgpu::BindGroupEntry{binding:8,resource:wgpu::BindingResource::Sampler(&sampler)}]});
    let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("direct point projection"),source:wgpu::ShaderSource::Wgsl(include_str!("splat.wgsl").into())});
    let pipe=|vertex:&str,fragment:&str,write:bool,compare|device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{
        label:Some(fragment),layout:Some(&pipeline_layout),vertex:wgpu::VertexState{module:&shader,entry_point:vertex,buffers:&[]},
        fragment:Some(wgpu::FragmentState{module:&shader,entry_point:fragment,targets:&[Some(wgpu::ColorTargetState{format,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
        primitive:wgpu::PrimitiveState::default(),depth_stencil:Some(wgpu::DepthStencilState{format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:write,depth_compare:compare,stencil:Default::default(),bias:Default::default()}),multisample:Default::default(),multiview:None,
    });
    let sky=pipe("fullscreen_vs","sky_fs",false,wgpu::CompareFunction::Always);
    let ground=pipe("ground_vs","ground_fs",true,wgpu::CompareFunction::Less);
    let splat=pipe("splat_vs","splat_fs",true,wgpu::CompareFunction::Less);
    let hud=pipe("fullscreen_vs","hud_fs",false,wgpu::CompareFunction::Always);
    let resources=Resources{uniform:&uniform,raw:&raw,lod:&lod,instances:&instances,patches:&patches,materials:&materials,atlases:&atlases,sampler:&sampler,shadow:&shadow_view};
    let mut compute=ComputeSplats::new(&device,format,config.width,config.height,&resources);
    let mut use_compute=std::env::var("IGOR_SPLAT_LEGACY").as_deref()!=Ok("1");
    let mut occlusion=std::env::var("IGOR_OCCLUSION").as_deref()==Ok("1");
    let mut stable_lod=std::env::var("IGOR_STABLE_LOD").as_deref()!=Ok("0");
    let mut early_z=std::env::var("IGOR_EARLY_Z").as_deref()!=Ok("0");
    let mut fast_tiny=std::env::var("IGOR_FAST_TINY").as_deref()!=Ok("0");
    let mut projection_cache=std::env::var("IGOR_PROJECTION_CACHE").as_deref()!=Ok("0");
    let mut wind=std::env::var("IGOR_WIND").as_deref()!=Ok("0");
    let mut shadows=std::env::var("IGOR_SHADOWS").as_deref()!=Ok("0");
    let wind_strength=env("IGOR_WIND_STRENGTH",1.0f32).clamp(0.0,3.0);
    let shadow_strength=env("IGOR_SHADOW_STRENGTH",0.85f32).clamp(0.0,1.0);
    let mut submitted_wind=false;let mut submitted_shadows=false;
    let animation_start=Instant::now();
    let mut submitted_fast=false;let mut submitted_cache=false;
    let mut gpu_lod=std::env::var("IGOR_GPU_LOD").as_deref()==Ok("1");
    let mut submitted_gpu=false;
    let mut taa=std::env::var("IGOR_TAA").as_deref()==Ok("1");
    let mut submitted_taa=false;let mut jitter_index=0usize;
    let taa_jitter=std::env::var("IGOR_TAA_JITTER").as_deref()==Ok("1");
    let brick_lod=std::env::var("IGOR_BRICK_LOD").as_deref()==Ok("1");
    let mut lod_state=points.lod_state();
    let mut submitted_stable=false;
    let mut submitted_early=false;
    let occluder_range=env("IGOR_OCCLUDER_RANGE",96.0f32).clamp(1.0,10000.0);
    let mut submitted_occlusion=false;
    let mut submitted_compute=false;
    let mut keys=HashSet::new();
    let mut counter=FrameCounter::default();
    let mut last=Instant::now();
    let mut title_time=Instant::now();
    let complete=Arc::new(AtomicBool::new(true));
    let mut pending=false;
    let mut next_frame=Instant::now();
    let cooldown=Duration::from_millis(env("IGOR_GPU_COOLDOWN_MS",4u64));
    let normal_budget=env("IGOR_POINT_BUDGET",3_500_000u64).max(1);
    let stress_budget=env("IGOR_STRESS_POINT_BUDGET",30_000_000u64).max(1);
    let mut point_budget=normal_budget;
    let mut target_px=env("IGOR_LOD_PIXELS",0.75f32).clamp(0.25,512.0);
    let mut drawable=true;
    let mut reset=None;
    let mut stats=(0u64,0u64,0u64,0u64,0u64,0usize,0.0f32,0.0f64);
    let mut submitted=Instant::now();
    println!("Direct projection: B wind; V shadows; O Hi-Z; L stable LOD; E early Z; C compute/legacy; +/- LOD target; H toggle stress budget; R ground/aerial; point budget={point_budget}, target={target_px:.2}px. No ray tracing.");
    event_loop.run(move|event,elwt|{
        elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now()+Duration::from_millis(2)));
        match event{
            Event::WindowEvent{event,window_id} if window_id==window.id()=>match event{
                WindowEvent::CloseRequested=>elwt.exit(),
                WindowEvent::Focused(false)=>keys.clear(),
                WindowEvent::KeyboardInput{event,..}=>{
                    if let PhysicalKey::Code(k)=event.physical_key{
                        if event.state==ElementState::Pressed{
                            keys.insert(k);
                            if k==KeyCode::Escape{elwt.exit();}
                            if !event.repeat {
                                match k{
                                    KeyCode::KeyR=>reset=Some(camera.position.y<30.0),
                                    KeyCode::KeyC=>{use_compute=!use_compute;compute.reset_history();counter=FrameCounter::default();println!("compute={use_compute}");},
                                    KeyCode::KeyO=>{occlusion=!occlusion;counter=FrameCounter::default();println!("occlusion={occlusion}");},
                                    KeyCode::KeyL=>{stable_lod=!stable_lod;lod_state=points.lod_state();counter=FrameCounter::default();println!("stable LOD={stable_lod}");},
                                    KeyCode::KeyE=>{early_z=!early_z;counter=FrameCounter::default();println!("early Z={early_z}");},
                                    KeyCode::KeyF=>{fast_tiny=!fast_tiny;counter=FrameCounter::default();println!("fast subpixel={fast_tiny}");},
                                    KeyCode::KeyP=>{projection_cache=!projection_cache;counter=FrameCounter::default();println!("projection cache={projection_cache}");},
                                    KeyCode::KeyB=>{wind=!wind;compute.reset_history();counter=FrameCounter::default();println!("wind={wind}");},
                                    KeyCode::KeyV=>{shadows=!shadows;compute.reset_history();counter=FrameCounter::default();println!("shadows={shadows}");},
                                    KeyCode::KeyG=>{gpu_lod=!gpu_lod;compute.reset_history();counter=FrameCounter::default();println!("GPU LOD={gpu_lod}");},
                                    KeyCode::KeyT=>{taa=!taa;compute.reset_history();jitter_index=0;counter=FrameCounter::default();println!("TAA={taa}");},
                                    KeyCode::KeyH=>{point_budget=if point_budget==normal_budget{stress_budget}else{normal_budget};println!("point budget={point_budget}");},
                                    KeyCode::Equal|KeyCode::NumpadAdd=>target_px=(target_px/1.25).max(0.25),
                                    KeyCode::Minus|KeyCode::NumpadSubtract=>target_px=(target_px*1.25).min(512.0),
                                    _=>{}
                                }
                            }
                        }else{keys.remove(&k);}
                    }
                }
                WindowEvent::Resized(size)=>{
                    drawable=size.width>0&&size.height>0;
                    if drawable{
                        let bytes=compute_splat::screen_bytes(size.width,size.height,max_instances as u64);
                        if bytes>largest || fixed_bytes+bytes+u64::from(size.width)*u64::from(size.height)*24>budget {eprintln!("resized screen exceeds GPU budget");drawable=false;return;}
                        config.width=size.width;config.height=size.height;
                        surface.configure(&device,&config);
                        depth_view=depth(&device,config.width,config.height);
                        let r=Resources{uniform:&uniform,raw:&raw,lod:&lod,instances:&instances,patches:&patches,materials:&materials,atlases:&atlases,sampler:&sampler,shadow:&shadow_view};
                        compute.resize(&device,config.width,config.height,&r);
                    }
                }
                WindowEvent::RedrawRequested if drawable&&!pending&&Instant::now()>=next_frame=>{
                    let now=Instant::now();let dt=(now-last).as_secs_f32().min(0.1);last=now;
                    if let Some(aerial)=reset.take(){camera=if aerial{aerial_camera.clone()}else{ground_camera.clone()};lod_state=points.lod_state();compute.reset_history();jitter_index=0;}
                    let held=|k|keys.contains(&k);
                    let speed=if held(KeyCode::ControlLeft){120.0}else{16.0};
                    if held(KeyCode::ArrowLeft){camera.yaw-=dt*1.5;}
                    if held(KeyCode::ArrowRight){camera.yaw+=dt*1.5;}
                    if held(KeyCode::ArrowUp){camera.pitch+=dt*1.5;}
                    if held(KeyCode::ArrowDown){camera.pitch-=dt*1.5;}
                    camera.pitch=camera.pitch.clamp(-1.5,1.5);
                    let (f,r,_)=camera.basis();
                    for (k,v) in [(KeyCode::KeyW,f),(KeyCode::KeyS,-f),(KeyCode::KeyD,r),(KeyCode::KeyA,-r),(KeyCode::Space,Vec3::Y),(KeyCode::ShiftLeft,Vec3::NEG_Y)]{if held(k){camera.position+=v*speed*dt;}}
                    let (f,r,u)=camera.basis();
                    let view=View{origin:camera.position,forward:f,right:r,up:u,width:config.width,height:config.height};
                    let s0=Instant::now();
                    let selected=if stable_lod{points.select_stable(view,target_px,point_budget,&mut lod_state)}else{points.select(view,target_px,point_budget)};
                    let selection_ms=s0.elapsed().as_secs_f64()*1000.0;
                    if !selected.budget_met {println!("point budget could not be met; skipping oversized frame");next_frame=Instant::now()+Duration::from_millis(100);return;}
                    if !selected.instances.is_empty(){queue.write_buffer(&instances,0,bytemuck::cast_slice(&selected.instances));}
                    let (job_count,cache_active)=if use_compute{
                        let r=Resources{uniform:&uniform,raw:&raw,lod:&lod,instances:&instances,patches:&patches,materials:&materials,atlases:&atlases,sampler:&sampler,shadow:&shadow_view};
                        let cache_budget=budget.saturating_sub(fixed_bytes+compute_splat::screen_bytes(config.width,config.height,max_instances as u64)+u64::from(config.width)*u64::from(config.height)*24+16*1048576);
                        match compute.prepare(&device,&queue,&r,&selected,projection_cache&&config.width<32768&&config.height<32768,cache_budget,gpu_lod){Ok(n)=>n,Err(e)=>{eprintln!("{e}");next_frame=Instant::now()+Duration::from_millis(100);return;}}
                    }else{(0,false)};
                    let far=(points.extent.length()+camera.position.length()+1024.0).max(2048.0);
                    let instance_count=selected.instances.len() as u32;
                    let use_occlusion=use_compute&&occlusion;
                    let occluder_depth=if use_occlusion {
                        selected.instances.iter().map(|i|(Vec3::from_slice(&i.bound)-camera.position).dot(f)-i.bound[3]).fold(f32::INFINITY,f32::min).max(0.05)+occluder_range
                    }else{0.0};
                    let taa_active=use_compute&&taa;
                    // Eight centered Halton(2,3) offsets in pixel units.
                    let jitter=[[0.0,-0.166667],[-0.25,0.166667],[0.25,-0.388889],[-0.375,-0.055556],[0.125,0.277778],[-0.125,-0.277778],[0.375,0.055556],[-0.4375,0.388889]];
                    let j=if taa_active&&taa_jitter{jitter[jitter_index%8]}else{[0.0,0.0]};
                    let ubo=Uniform{origin:camera.position.extend(0.0).to_array(),forward:f.extend(0.0).to_array(),right:r.extend(j[0]).to_array(),up:u.extend(j[1]).to_array(),screen:[config.width as f32,config.height as f32,2.0*(30f32.to_radians()).tan()/config.height as f32,((early_z as u32)|((fast_tiny as u32)<<1)|((cache_active as u32)<<2)|((taa_active as u32)<<3)|(((use_compute&&wind) as u32)<<4)|(((use_compute&&shadows) as u32)<<5)) as f32],extent:points.extent.extend(far).to_array(),hud:counter.glyphs(),params:[job_count,use_occlusion as u32,instance_count,occluder_depth.to_bits()],effects:[animation_start.elapsed().as_secs_f32(),wind_strength,shadow_strength,0.0],shadow_transform};
                    queue.write_buffer(&uniform,0,bytemuck::bytes_of(&ubo));
                    let frame=match surface.get_current_texture(){Ok(f)=>f,Err(wgpu::SurfaceError::Lost|wgpu::SurfaceError::Outdated)=>{surface.configure(&device,&config);return;},Err(wgpu::SurfaceError::Timeout)=>return,Err(wgpu::SurfaceError::OutOfMemory)=>{elwt.exit();return;}};
                    compute.prepare_history(&queue,HistoryCamera{origin:ubo.origin,forward:ubo.forward,right:ubo.right,up:ubo.up,screen:ubo.screen,valid:[0;4]},taa_active);
                    if taa_active{jitter_index=(jitter_index+1)%8;}
                    let color=frame.texture.create_view(&Default::default());
                    let mut encoder=device.create_command_encoder(&Default::default());
                    if use_compute {
                        compute.encode(&mut encoder,&color,config.width,config.height,job_count,instance_count,use_occlusion,gpu_lod,taa_active);
                    }else{
                        let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor{
                            label:Some("direct projection and Z test"),color_attachments:&[Some(wgpu::RenderPassColorAttachment{view:&color,resolve_target:None,ops:wgpu::Operations{load:wgpu::LoadOp::Clear(wgpu::Color::BLACK),store:wgpu::StoreOp::Store}})],
                            depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment{view:&depth_view,depth_ops:Some(wgpu::Operations{load:wgpu::LoadOp::Clear(1.0),store:wgpu::StoreOp::Discard}),stencil_ops:None}),occlusion_query_set:None,timestamp_writes:None});
                        pass.set_bind_group(0,&bind,&[]);
                        pass.set_pipeline(&sky);pass.draw(0..3,0..1);
                        pass.set_pipeline(&ground);pass.draw(0..6,0..1);
                        pass.set_pipeline(&splat);
                        for d in &selected.draws {pass.draw(0..d.point_count*6,d.start..d.end);}
                        pass.set_pipeline(&hud);pass.draw(0..3,0..1);
                    }
                    stats=(selected.points,selected.raw_points,selected.visited,selected.world_splats,selected.trees,selected.draws.len(),selected.threshold,selection_ms);
                    submitted_occlusion=use_occlusion;submitted_compute=use_compute;submitted_stable=stable_lod;submitted_early=early_z;submitted_fast=use_compute&&fast_tiny;submitted_cache=cache_active;submitted_gpu=use_compute&&gpu_lod;submitted_taa=taa_active;submitted_wind=use_compute&&wind;submitted_shadows=use_compute&&shadows;
                    complete.store(false,Ordering::Release);submitted=Instant::now();
                    queue.submit(Some(encoder.finish()));frame.present();pending=true;
                    let done=complete.clone();queue.on_submitted_work_done(move||done.store(true,Ordering::Release));
                }
                _=>{}
            },
            Event::AboutToWait=>{
                device.poll(wgpu::Maintain::Poll);
                if pending&&complete.load(Ordering::Acquire){
                    pending=false;let now=Instant::now();counter.presented(now);next_frame=now+cooldown;
                    if title_time.elapsed()>=Duration::from_secs(1){
                        let (n,raw_n,cells,world,trees,draws,lod_px,cpu)=stats;
                        let wall=(now-submitted).as_secs_f64()*1000.0;
                        let mode=if submitted_compute {if submitted_occlusion{"compute Hi-Z"}else{"compute no-cull"}}else{"legacy quads"};
                        let lod_mode=if submitted_stable{"stable"}else{"instant"};
                        let z_mode=if submitted_compute&&submitted_early{"early-Z"}else{"late-Z"};
                        let occ=if submitted_occlusion {
                            compute.read_stats(&device).map(|[near,near_pts,culled,culled_pts]|format!(" | culled {culled} instances / {culled_pts} pts | near {near}/{near_pts} pts")).unwrap_or_else(||" | stats unavailable".into())
                        }else{String::new()};
                        let msg=format!("Igor | {mode} | {lod_mode} LOD | {z_mode} | fast:{} cache:{} bricks:{} gpuLOD:{} TAA:{} jitter:{} wind:{} shadows:{} | {n} selected splats ({raw_n} textured) | {trees} trees + {world} blocks | {cells} cells | {draws} LOD batches | LOD {lod_px:.2}px | select {cpu:.2}ms | submit {wall:.2}ms{occ}",submitted_fast as u8,submitted_cache as u8,brick_lod as u8,submitted_gpu as u8,submitted_taa as u8,(submitted_taa&&taa_jitter) as u8,submitted_wind as u8,submitted_shadows as u8);
                        window.set_title(&msg);println!("{msg}");title_time=now;
                    }
                }
                if drawable&&!pending&&Instant::now()>=next_frame{window.request_redraw();}
            }
            _=>{}
        }
    })?;
    Ok(())
}
