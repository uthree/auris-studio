//! Adapter discovery and the compute plumbing shared by every kernel in this crate.
//!
//! A [`GpuContext`] owns one wgpu device plus a cache of compiled compute pipelines. Creating
//! one is the only place this crate talks to the driver directly; everything else goes through
//! [`GpuContext::run_reduction`].
//!
//! Nothing here panics. wgpu's default behaviour is the opposite — an uncaptured device error
//! aborts the process — so the device is created with a logging error handler, and each
//! dispatch runs inside error scopes that turn a driver complaint into `None`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Invocations per compute workgroup.
///
/// 64 is a multiple of the native SIMD width of every backend we target (Apple/AMD wavefronts
/// are 32 or 64, Nvidia warps are 32) and is within the 256-invocation minimum that the WebGPU
/// downlevel limits guarantee, so one workgroup size works everywhere.
const WORKGROUP_SIZE: u32 = 64;

/// Bytes per output slot. Every kernel writes one `vec4<f32>` per slot.
const SLOT_BYTES: usize = 16;

/// Entry point name every kernel in this crate uses.
const ENTRY_POINT: &str = "main";

/// Hard ceiling on the samples and slots a single dispatch may address.
///
/// Both kernels index their input with WGSL `u32` arithmetic — `bucket * samples_per_bucket`,
/// `first_active + id.x * span`, `i + 2u` — and the host narrows the chunk length to a `u32`
/// when it fills the uniform block. WGSL `u32` overflow wraps silently instead of trapping, so a
/// dispatch sized anywhere near `u32::MAX` would read wrapped-around indices and return quietly
/// wrong numbers rather than failing. This is not purely theoretical: Metal reports
/// `max_storage_buffer_binding_size` as the machine's whole unified memory, so a large Mac can
/// advertise a binding big enough to hold more than `u32::MAX` samples.
///
/// Half the `u32` range leaves headroom for the `+ span` terms and is still 8 GiB of `f32`,
/// far more than a chunk this crate would ever want to upload in one go.
const MAX_ADDRESSABLE: u64 = (u32::MAX / 2) as u64;

/// A wgpu device plus the compiled kernels running on it.
///
/// Cheap to share: all wgpu handles are internally reference counted, and the pipeline cache is
/// behind a mutex, so a single context can be held by the UI and used from any thread.
pub struct GpuContext {
    // Held so the instance outlives the adapter it produced, and so a future device-picker UI
    // can enumerate adapters without rebuilding it.
    _instance: wgpu::Instance,
    adapter_info: wgpu::AdapterInfo,
    max_input_samples: usize,
    max_output_slots: usize,
    device: wgpu::Device,
    queue: wgpu::Queue,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: Mutex<HashMap<&'static str, wgpu::ComputePipeline>>,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &self.adapter_info.name)
            .field("backend", &self.backend())
            .finish()
    }
}

impl GpuContext {
    /// Opens the system's preferred adapter, or returns `None` when there is none.
    ///
    /// Blocks the calling thread — wgpu's adapter and device requests are futures even on
    /// native backends. Call this once during start-up, never from the audio thread.
    ///
    /// Failure is expected and not an error: headless CI machines, remote sessions and
    /// unsupported drivers all land here. Every reason is logged at debug level and the caller
    /// simply keeps using the CPU path.
    pub fn new() -> Option<GpuContext> {
        // `Instance::new` panics if no backend was compiled for this target, and this is the
        // documented way to ask before it does.
        if wgpu::Instance::enabled_backend_features().is_empty() {
            log::debug!("auris-gpu: no wgpu backend is compiled for this target");
            return None;
        }

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            // Waveform reductions are throughput-bound and run in bursts while the user is
            // waiting for a redraw, so the fast adapter is the right default even on laptops.
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }));
        let adapter = match adapter {
            Ok(adapter) => adapter,
            Err(error) => {
                log::debug!("auris-gpu: no adapter available ({error})");
                return None;
            }
        };

        let adapter_info = adapter.get_info();
        // Ask for the adapter's real limits rather than the conservative defaults: the default
        // 128 MiB storage binding would force extra chunking on files this crate is built for.
        let limits = adapter.limits();
        let requested = wgpu::DeviceDescriptor {
            label: Some("auris-gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: limits.clone(),
            ..Default::default()
        };
        let (device, queue) = match pollster::block_on(adapter.request_device(&requested)) {
            Ok(pair) => pair,
            Err(error) => {
                log::debug!(
                    "auris-gpu: adapter `{}` refused a device ({error})",
                    adapter_info.name
                );
                return None;
            }
        };

        // Without a handler wgpu treats any error that escapes an error scope as fatal and
        // panics on whichever thread raised it. Downgrade that to a log line.
        device.on_uncaptured_error(Arc::new(|error| {
            log::error!("auris-gpu: uncaptured wgpu error: {error}");
        }));

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("auris-gpu.reduction"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("auris-gpu.reduction"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        log::debug!(
            "auris-gpu: using `{}` on the {} backend",
            adapter_info.name,
            adapter_info.backend.to_str()
        );

        // Both bounds are folded down to a value the shaders' u32 index arithmetic can express;
        // see `MAX_ADDRESSABLE`.
        let storage_bytes = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let max_input_samples =
            (storage_bytes / size_of::<f32>() as u64).min(MAX_ADDRESSABLE) as usize;
        let max_output_slots = (storage_bytes / SLOT_BYTES as u64)
            .min(limits.max_compute_workgroups_per_dimension as u64 * WORKGROUP_SIZE as u64)
            .min(MAX_ADDRESSABLE) as usize;

        Some(GpuContext {
            _instance: instance,
            adapter_info,
            max_input_samples,
            max_output_slots,
            device,
            queue,
            bind_group_layout,
            pipeline_layout,
            pipelines: Mutex::new(HashMap::new()),
        })
    }

    /// Name of the adapter in use, for display in the UI.
    pub fn adapter_name(&self) -> String {
        self.adapter_info.name.clone()
    }

    /// Graphics backend in use: `"metal"`, `"vulkan"`, `"dx12"`, `"gl"` or `"webgpu"`.
    pub fn backend(&self) -> &'static str {
        self.adapter_info.backend.to_str()
    }

    /// Largest number of `f32` samples that may be uploaded in one dispatch.
    ///
    /// Callers split their input into chunks of at most this size. The bound comes from the
    /// adapter's storage-binding and buffer-size limits, whichever is smaller, further capped
    /// so that a chunk length always fits the `u32` index arithmetic the kernels use.
    pub fn max_input_samples(&self) -> usize {
        self.max_input_samples
    }

    /// Largest number of output slots one dispatch may produce.
    ///
    /// Bounded by the storage binding size, by how many workgroups a single
    /// `dispatch_workgroups` call may launch along one axis, and by the same `u32` cap as
    /// [`Self::max_input_samples`].
    pub fn max_output_slots(&self) -> usize {
        self.max_output_slots
    }

    /// Shrinks the per-dispatch limits so tests can reach the chunking paths.
    ///
    /// A real adapter advertises gigabyte-sized bindings, so the multi-dispatch code in
    /// `waveform.rs` and `analysis.rs` — chunk overlap, bucket-boundary splitting, the final
    /// partial chunk — would otherwise only run for inputs far too large to put in a test.
    /// Passing [`usize::MAX`] leaves a bound alone; neither can be raised above what the
    /// device actually supports.
    #[cfg(test)]
    pub(crate) fn constrain_for_test(&mut self, max_input_samples: usize, max_output_slots: usize) {
        self.max_input_samples = self.max_input_samples.min(max_input_samples);
        self.max_output_slots = self.max_output_slots.min(max_output_slots);
    }

    /// Runs one reduction kernel and reads the result back.
    ///
    /// `shader_source` must be WGSL declaring exactly these bindings in group 0 and a
    /// `@compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id: vec3<u32>)`
    /// that writes `output[id.x]` for every `id.x` below `output_slots`:
    ///
    /// ```wgsl
    /// @group(0) @binding(0) var<storage, read>       samples: array<f32>;
    /// @group(0) @binding(1) var<storage, read_write> output:  array<vec4<f32>>;
    /// @group(0) @binding(2) var<uniform>             params:  Params;
    /// ```
    ///
    /// `params` is uploaded verbatim to binding 2, so its Rust and WGSL layouts must agree;
    /// WGSL requires a uniform struct size that is a multiple of 16 bytes.
    ///
    /// Returns `None` — after logging at debug level — if the request exceeds the device's
    /// limits, if wgpu reports any validation, out-of-memory or internal error, or if the
    /// readback fails. It never panics, so the caller can always fall back to the CPU.
    pub fn run_reduction<P: bytemuck::Pod>(
        &self,
        label: &'static str,
        shader_source: &'static str,
        input: &[f32],
        params: P,
        output_slots: usize,
    ) -> Option<Vec<[f32; 4]>> {
        if output_slots == 0 {
            return Some(Vec::new());
        }
        if input.is_empty() {
            log::debug!("auris-gpu: {label} asked for {output_slots} slots from no input");
            return None;
        }
        if input.len() > self.max_input_samples() {
            log::debug!(
                "auris-gpu: {label} input of {} samples exceeds the device limit of {}",
                input.len(),
                self.max_input_samples()
            );
            return None;
        }
        if output_slots > self.max_output_slots() {
            log::debug!(
                "auris-gpu: {label} wants {output_slots} output slots, limit is {}",
                self.max_output_slots()
            );
            return None;
        }

        // wgpu reports errors asynchronously into whichever scope is innermost for this
        // filter, so cover all three kinds and pop them in reverse order of pushing.
        let guards = [
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::Validation,
        ]
        .map(|filter| self.device.push_error_scope(filter));

        let result = self.dispatch(label, shader_source, input, params, output_slots);

        let mut reported = false;
        for guard in guards.into_iter().rev() {
            if let Some(error) = pollster::block_on(guard.pop()) {
                log::debug!("auris-gpu: {label} failed: {error}");
                reported = true;
            }
        }
        if reported { None } else { result }
    }

    /// The body of [`Self::run_reduction`], with error reporting left to the caller.
    fn dispatch<P: bytemuck::Pod>(
        &self,
        label: &'static str,
        shader_source: &'static str,
        input: &[f32],
        params: P,
        output_slots: usize,
    ) -> Option<Vec<[f32; 4]>> {
        let pipeline = self.pipeline(label, shader_source);
        let output_bytes = (output_slots * SLOT_BYTES) as u64;

        let input_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size_of_val(input) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&input_buffer, 0, bytemuck::cast_slice(input));

        let params_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: size_of::<P>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params_buffer, 0, bytemuck::bytes_of(&params));

        let output_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: output_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        // A STORAGE buffer cannot also be MAP_READ, so results land in a staging buffer.
        let readback_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: output_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(output_slots.div_ceil(WORKGROUP_SIZE as usize) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output_buffer, 0, &readback_buffer, 0, output_bytes);
        self.queue.submit([encoder.finish()]);

        self.read_back(&readback_buffer, output_slots)
    }

    /// Maps `buffer`, copies it out as slots and unmaps it again.
    fn read_back(&self, buffer: &wgpu::Buffer, slots: usize) -> Option<Vec<[f32; 4]>> {
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            // The receiver is alive until this function returns, but a dropped channel must
            // still not panic here: this runs on a wgpu-internal callback path.
            let _ = sender.send(result);
        });

        if let Err(error) = self.device.poll(wgpu::PollType::wait_indefinitely()) {
            log::debug!("auris-gpu: waiting for the GPU failed: {error}");
            return None;
        }
        match receiver.try_recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                log::debug!("auris-gpu: buffer mapping failed: {error}");
                return None;
            }
            Err(error) => {
                log::debug!("auris-gpu: buffer mapping never completed: {error}");
                return None;
            }
        }

        let slots = match buffer.get_mapped_range(..) {
            Ok(view) => match bytemuck::try_cast_slice::<u8, [f32; 4]>(&view) {
                // Defend against a short read rather than indexing past the end later.
                Ok(values) if values.len() == slots => Some(values.to_vec()),
                Ok(values) => {
                    log::debug!(
                        "auris-gpu: expected {slots} result slots, read {}",
                        values.len()
                    );
                    None
                }
                Err(error) => {
                    log::debug!("auris-gpu: readback was not a slot array: {error}");
                    None
                }
            },
            Err(error) => {
                log::debug!("auris-gpu: mapped range unavailable: {error}");
                None
            }
        };
        buffer.unmap();
        slots
    }

    /// Returns the compiled pipeline for `label`, compiling and caching it on first use.
    fn pipeline(&self, label: &'static str, shader_source: &'static str) -> wgpu::ComputePipeline {
        let mut cache = match self.pipelines.lock() {
            Ok(cache) => cache,
            // Poisoning only means some other thread panicked while holding the lock; the map
            // is a plain cache and is still consistent, so refusing to use the GPU for the
            // rest of the process would be a worse outcome than carrying on.
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(pipeline) = cache.get(label) {
            return pipeline.clone();
        }

        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(shader_source.into()),
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&self.pipeline_layout),
                module: &module,
                entry_point: Some(ENTRY_POINT),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            });
        cache.insert(label, pipeline.clone());
        pipeline
    }
}

/// A storage-buffer entry in the shared reduction bind group layout.
fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_reports_its_device_or_is_absent() {
        let Some(context) = GpuContext::new() else {
            return;
        };
        assert!(!context.adapter_name().is_empty());
        assert!(
            ["noop", "vulkan", "metal", "dx12", "gl", "webgpu"].contains(&context.backend()),
            "unexpected backend {}",
            context.backend()
        );
    }

    #[test]
    fn limits_leave_room_for_a_realistic_file() {
        let Some(context) = GpuContext::new() else {
            return;
        };
        // The WebGPU downlevel floor is a 128 MiB storage binding, i.e. 32 Mi samples, which is
        // about eleven minutes of one 48 kHz channel. Anything less means our chunking maths is
        // reading the wrong limit.
        assert!(
            context.max_input_samples() >= 32 * 1024 * 1024,
            "max_input_samples was {}",
            context.max_input_samples()
        );
        assert!(context.max_output_slots() >= 65_535);
    }

    #[test]
    fn empty_input_yields_no_slots() {
        let Some(context) = GpuContext::new() else {
            return;
        };
        let slots = context
            .run_reduction("auris-gpu.test.empty", "", &[], [0u32; 4], 0)
            .expect("an empty reduction should succeed trivially");
        assert!(slots.is_empty());
    }

    /// A minimal kernel in the shape [`GpuContext::run_reduction`] documents: it sums `span`
    /// samples per slot and echoes the uniform back, so a wrong binding order, a mis-sized
    /// uniform or a short readback all show up as wrong numbers rather than as a silent pass.
    const SUM_KERNEL: &str = r#"
struct Params { span: u32, tag: u32, pad0: u32, pad1: u32 }
@group(0) @binding(0) var<storage, read> samples: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let start = id.x * params.span;
    if (start >= arrayLength(&samples)) {
        return;
    }
    let end = min(start + params.span, arrayLength(&samples));
    var total = 0.0;
    for (var i = start; i < end; i = i + 1u) {
        total = total + samples[i];
    }
    output[id.x] = vec4<f32>(total, f32(end - start), f32(params.tag), 0.0);
}
"#;

    #[repr(C)]
    #[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
    struct SumParams {
        span: u32,
        tag: u32,
        pad0: u32,
        pad1: u32,
    }

    #[test]
    fn run_reduction_executes_a_custom_kernel() {
        let Some(context) = GpuContext::new() else {
            return;
        };
        // 10 samples in spans of 4 leaves a short final slot, so the slot count must round up
        // and the last slot must report the length it really covered.
        let input: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        let params = SumParams {
            span: 4,
            tag: 7,
            pad0: 0,
            pad1: 0,
        };
        let slots = context
            .run_reduction("auris-gpu.test.sum", SUM_KERNEL, &input, params, 3)
            .expect("a working adapter should run the kernel");

        assert_eq!(slots.len(), 3);
        assert_eq!(slots[0], [1.0 + 2.0 + 3.0 + 4.0, 4.0, 7.0, 0.0]);
        assert_eq!(slots[1], [5.0 + 6.0 + 7.0 + 8.0, 4.0, 7.0, 0.0]);
        assert_eq!(slots[2], [9.0 + 10.0, 2.0, 7.0, 0.0]);
    }

    #[test]
    fn oversized_requests_are_refused_rather_than_dispatched() {
        let Some(mut context) = GpuContext::new() else {
            return;
        };
        context.constrain_for_test(8, 2);
        assert_eq!(context.max_input_samples(), 8);
        assert_eq!(context.max_output_slots(), 2);

        let params = SumParams {
            span: 4,
            tag: 0,
            pad0: 0,
            pad1: 0,
        };
        assert!(
            context
                .run_reduction("auris-gpu.test.sum", SUM_KERNEL, &[0.0; 9], params, 2)
                .is_none(),
            "an input past the sample limit must be refused"
        );
        assert!(
            context
                .run_reduction("auris-gpu.test.sum", SUM_KERNEL, &[0.0; 8], params, 3)
                .is_none(),
            "more slots than the limit must be refused"
        );
        // The refusals must not have poisoned the context for a request that does fit.
        assert!(
            context
                .run_reduction("auris-gpu.test.sum", SUM_KERNEL, &[1.0; 8], params, 2)
                .is_some()
        );
    }
}
