//! Vulkan compute engine with CPU verification of every returned match.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use curve25519_dalek::{EdwardsPoint, Scalar};
use wgpu::util::DeviceExt;
use wgpu::{
    Adapter, Backend, Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingType, Buffer, BufferBindingType,
    BufferDescriptor, BufferUsages, CommandEncoderDescriptor, ComputePassDescriptor,
    ComputePipeline, ComputePipelineDescriptor, Device, DeviceDescriptor, DeviceType, ErrorFilter,
    Features, Instance, InstanceDescriptor, Limits, MapMode, MemoryHints,
    PipelineCompilationOptions, PipelineLayoutDescriptor, PollType, Queue, ShaderModuleDescriptor,
    ShaderSource, ShaderStages, Trace,
};

use crate::crypto::address::onion_address;
use crate::crypto::keys::ExpandedSecret;
use crate::search::cpu::FoundKey;
use crate::search::filter::{FilterSet, Pattern};

const WORKGROUP_SIZE: u32 = 128;
const OFFSET_PAIRS: u32 = 127;
const CANDIDATES_PER_GROUP: u32 = 255;
const AUTO_TUNE_WORKGROUPS: [u32; 5] = [10, 20, 32, 40, 64];
const AUTO_TUNE_ROUNDS: u32 = 64;
const RESULT_CAPACITY: u32 = 256;
const RESULT_HEADER_WORDS: usize = 2;
const RESULT_WORDS: usize = RESULT_HEADER_WORDS + RESULT_CAPACITY as usize * 2;
const RESULT_BYTES: u64 = RESULT_WORDS as u64 * 4;
const RESULT_SIZE: usize = RESULT_WORDS * 4;
const CONFIG_BYTES: u64 = 48;
const CONFIG_SIZE: usize = 48;
const POINT_BYTES: u64 = 160;
const WORKGROUP_STORAGE_BYTES: u32 = 21_504;
const ROUNDS_PER_CHUNK: u32 = 128;
const COMMITTED_BANK: u32 = 0;
const SCRATCH_BANK_A: u32 = 1;
const SCRATCH_BANK_B: u32 = 2;

struct GpuContext {
    device_name: String,
    device: Device,
    queue: Queue,
    bind_group_layout: BindGroupLayout,
    initialize_pipeline: ComputePipeline,
    search_pipeline: ComputePipeline,
    commit_pipeline: ComputePipeline,
}

struct DeviceResult {
    total_matches: u32,
    arithmetic_error: u32,
    offsets: Vec<u64>,
}

enum DispatchResult {
    Complete(Vec<FoundKey>),
    Overflow,
}

#[derive(Clone, Copy)]
struct ShaderConfig {
    workgroup_count: u32,
    pattern_count: usize,
    rounds: u32,
    base_offset: u64,
    group_start: u32,
    group_count: u32,
    source_bank: u32,
    destination_bank: u32,
}

/// One timed GPU candidate-generation measurement.
#[derive(Clone, Copy, Debug)]
pub struct GpuMeasurement {
    /// Complete candidates checked.
    pub attempts: u64,
    /// Wall-clock duration, including command submission and result readback.
    pub duration: Duration,
    /// End-to-end candidate rate.
    pub attempts_per_second: f64,
    /// Verified matches dropped after the device search completed.
    ///
    /// The exact device ring replays overflowing chunks, so this is always zero.
    pub discarded_matches: u32,
}

/// Persistent GPU search state for one secret and pattern set.
pub struct GpuEngine {
    context: Arc<GpuContext>,
    bind_group: BindGroup,
    config_buffer: Buffer,
    result_buffer: Buffer,
    readback_buffer: Buffer,
    secret: ExpandedSecret,
    filters: Arc<FilterSet>,
    workgroups: u32,
    lane_count: u32,
    base_offset: u64,
    measured_rate: f64,
}

/// Live state for an automatically tuned GPU search worker.
pub struct GpuSearchSession {
    /// Verified matches returned by the GPU worker.
    pub matches: mpsc::Receiver<FoundKey>,
    /// Cooperative stop flag.
    pub stop: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
    handle: JoinHandle<Result<u64>>,
    device_name: String,
    measured_rate: f64,
}

impl GpuSearchSession {
    /// Returns the completed candidate count.
    #[must_use]
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }

    /// Returns the complete candidate rate measured during automatic tuning.
    #[must_use]
    pub const fn measured_rate(&self) -> f64 {
        self.measured_rate
    }

    /// Returns the selected adapter name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// Returns the cooperative stop flag.
    #[must_use]
    pub fn stop_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Tries to receive one verified match without blocking.
    pub fn try_match(&self) -> Result<Option<FoundKey>> {
        match self.matches.try_recv() {
            Ok(found) => Ok(Some(found)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                bail!("GPU search worker stopped unexpectedly")
            }
        }
    }

    /// Requests shutdown, joins the worker, and returns dropped matches.
    pub fn stop_and_join(self) -> Result<u64> {
        self.stop.store(true, Ordering::Relaxed);
        self.handle
            .join()
            .map_err(|_| anyhow::anyhow!("GPU search worker panicked"))?
    }
}

impl GpuEngine {
    /// Creates a persistent engine and measures the best workgroup count.
    pub fn new(secret: &ExpandedSecret, filters: Arc<FilterSet>) -> Result<Self> {
        let context = Arc::new(GpuContext::new()?);
        self_test_context(Arc::clone(&context))?;
        let mut best = None;
        for workgroups in AUTO_TUNE_WORKGROUPS {
            let mut engine = Self::from_context(
                Arc::clone(&context),
                secret,
                Arc::clone(&filters),
                workgroups,
            )?;
            let measurement = engine.benchmark_rounds(AUTO_TUNE_ROUNDS)?;
            if best.is_none_or(|(_, best_rate)| measurement.attempts_per_second > best_rate) {
                best = Some((workgroups, measurement.attempts_per_second));
            }
        }
        let (workgroups, measured_rate) = best.expect("GPU auto-tune cases are not empty");
        let mut engine = Self::from_context(context, secret, filters, workgroups)?;
        engine.measured_rate = measured_rate;
        Ok(engine)
    }

    /// Creates an engine with an exact workgroup count for measurement.
    pub fn with_workgroups(
        secret: &ExpandedSecret,
        filters: Arc<FilterSet>,
        workgroups: u32,
    ) -> Result<Self> {
        let context = Arc::new(GpuContext::new()?);
        Self::from_context(context, secret, filters, workgroups)
    }

    fn from_context(
        context: Arc<GpuContext>,
        secret: &ExpandedSecret,
        filters: Arc<FilterSet>,
        workgroups: u32,
    ) -> Result<Self> {
        ensure!(
            workgroups > 0,
            "GPU workgroup count must be greater than zero"
        );
        let lane_count = workgroups
            .checked_mul(CANDIDATES_PER_GROUP)
            .context("GPU candidate-lane count overflow")?;

        let compressed = initial_compressed_points(secret, workgroups)?;
        let compressed_bytes = encode_compressed_points(&compressed);
        let pattern_bytes = encode_patterns(&filters);
        let initial_point_count = u64::from(workgroups) + u64::from(OFFSET_PAIRS) + 1;
        let stored_point_count = initial_point_count
            .checked_add(u64::from(workgroups) * 2)
            .context("GPU scratch-point count overflow")?;
        let points_size = stored_point_count
            .checked_mul(POINT_BYTES)
            .context("GPU point buffer size overflow")?;

        let compressed_buffer =
            context
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Onionloom compressed points"),
                    contents: &compressed_bytes,
                    usage: BufferUsages::STORAGE,
                });
        let points_buffer = context.device.create_buffer(&BufferDescriptor {
            label: Some("Onionloom extended points"),
            size: points_size,
            usage: BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let patterns_buffer =
            context
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Onionloom prefix masks"),
                    contents: &pattern_bytes,
                    usage: BufferUsages::STORAGE,
                });
        let initial_config = encode_config(ShaderConfig {
            workgroup_count: workgroups,
            pattern_count: filters.patterns().len(),
            rounds: 0,
            base_offset: 0,
            group_start: 0,
            group_count: workgroups,
            source_bank: COMMITTED_BANK,
            destination_bank: SCRATCH_BANK_A,
        })?;
        let config_buffer = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Onionloom GPU configuration"),
                contents: &initial_config,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            });
        let result_buffer = context
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Onionloom GPU result"),
                contents: &[0_u8; RESULT_SIZE],
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            });
        let readback_buffer = context.device.create_buffer(&BufferDescriptor {
            label: Some("Onionloom GPU result readback"),
            size: RESULT_BYTES,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let bind_group = context.device.create_bind_group(&BindGroupDescriptor {
            label: Some("Onionloom GPU resources"),
            layout: &context.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: compressed_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: points_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: patterns_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: config_buffer.as_entire_binding(),
                },
                BindGroupEntry {
                    binding: 4,
                    resource: result_buffer.as_entire_binding(),
                },
            ],
        });

        let mut encoder = context
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Onionloom GPU point initialization"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("Onionloom GPU point initialization"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&context.initialize_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let initialization_groups = u32::try_from(initial_point_count)
                .expect("GPU point count fits u32")
                .div_ceil(64);
            pass.dispatch_workgroups(initialization_groups, 1, 1);
        }
        context.queue.submit(Some(encoder.finish()));
        context
            .device
            .poll(PollType::Wait)
            .context("GPU initialization wait failed")?;

        Ok(Self {
            context,
            bind_group,
            config_buffer,
            result_buffer,
            readback_buffer,
            secret: secret.clone(),
            filters,
            workgroups,
            lane_count,
            base_offset: 0,
            measured_rate: 0.0,
        })
    }

    /// Returns the selected adapter name.
    #[must_use]
    pub fn device_name(&self) -> &str {
        &self.context.device_name
    }

    /// Returns the number of persistent candidate lanes.
    #[must_use]
    pub const fn lane_count(&self) -> u32 {
        self.lane_count
    }

    /// Returns the measured or explicitly selected workgroup count.
    #[must_use]
    pub const fn workgroup_count(&self) -> u32 {
        self.workgroups
    }

    /// Returns the complete candidate rate selected during automatic tuning.
    #[must_use]
    pub const fn measured_rate(&self) -> f64 {
        self.measured_rate
    }

    /// Processes one bounded chunk and CPU-verifies every device prehit.
    pub fn run_chunk(&mut self, rounds: u32) -> Result<(GpuMeasurement, Vec<FoundKey>)> {
        ensure!(rounds > 0, "GPU rounds must be greater than zero");
        let attempts = u64::from(self.lane_count)
            .checked_mul(u64::from(rounds))
            .context("GPU attempt count overflow")?;
        ensure!(
            u32::try_from(attempts).is_ok(),
            "GPU chunk is too large for exact 32-bit shader indexing"
        );
        let next_base = self
            .base_offset
            .checked_add(attempts)
            .context("GPU search offset exhausted; start a fresh search")?;

        let started = Instant::now();
        let (matching_keys, final_bank) = match self.dispatch(
            rounds,
            self.base_offset,
            0,
            self.workgroups,
            COMMITTED_BANK,
            SCRATCH_BANK_A,
        )? {
            DispatchResult::Complete(matches) => (matches, SCRATCH_BANK_A),
            DispatchResult::Overflow => self.replay_exact(rounds, self.base_offset)?,
        };
        self.commit(final_bank)?;
        let duration = started.elapsed();
        self.base_offset = next_base;
        Ok((
            GpuMeasurement {
                attempts,
                duration,
                attempts_per_second: attempts as f64 / duration.as_secs_f64(),
                discarded_matches: 0,
            },
            matching_keys,
        ))
    }

    fn replay_exact(&self, rounds: u32, base_offset: u64) -> Result<(Vec<FoundKey>, u32)> {
        let mut matches = Vec::new();
        let mut source_bank = COMMITTED_BANK;
        let mut destination_bank = SCRATCH_BANK_A;
        for round in 0..rounds {
            let round_delta = u64::from(self.lane_count)
                .checked_mul(u64::from(round))
                .context("GPU replay offset overflow")?;
            let round_base = base_offset
                .checked_add(round_delta)
                .context("GPU replay offset exhausted")?;
            self.replay_groups(
                round_base,
                0,
                self.workgroups,
                source_bank,
                destination_bank,
                &mut matches,
            )?;
            source_bank = destination_bank;
            destination_bank = if destination_bank == SCRATCH_BANK_A {
                SCRATCH_BANK_B
            } else {
                SCRATCH_BANK_A
            };
        }
        Ok((matches, source_bank))
    }

    fn replay_groups(
        &self,
        base_offset: u64,
        group_start: u32,
        group_count: u32,
        source_bank: u32,
        destination_bank: u32,
        matches: &mut Vec<FoundKey>,
    ) -> Result<()> {
        match self.dispatch(
            1,
            base_offset,
            group_start,
            group_count,
            source_bank,
            destination_bank,
        )? {
            DispatchResult::Complete(mut found) => {
                matches.append(&mut found);
                Ok(())
            }
            DispatchResult::Overflow => {
                ensure!(
                    group_count > 1,
                    "GPU result ring overflowed for one exact 255-candidate workgroup"
                );
                let left_count = group_count / 2;
                let right_start = group_start
                    .checked_add(left_count)
                    .context("GPU replay group range overflow")?;
                let right_count = group_count - left_count;
                self.replay_groups(
                    base_offset,
                    group_start,
                    left_count,
                    source_bank,
                    destination_bank,
                    matches,
                )?;
                self.replay_groups(
                    base_offset,
                    right_start,
                    right_count,
                    source_bank,
                    destination_bank,
                    matches,
                )
            }
        }
    }

    fn dispatch(
        &self,
        rounds: u32,
        base_offset: u64,
        group_start: u32,
        group_count: u32,
        source_bank: u32,
        destination_bank: u32,
    ) -> Result<DispatchResult> {
        ensure!(rounds > 0, "GPU dispatch rounds must be greater than zero");
        ensure!(group_count > 0, "GPU dispatch must contain a workgroup");
        let group_end = group_start
            .checked_add(group_count)
            .context("GPU dispatch group range overflow")?;
        ensure!(
            group_end <= self.workgroups,
            "GPU dispatch group range is outside the configured engine"
        );
        ensure!(
            source_bank <= SCRATCH_BANK_B
                && (SCRATCH_BANK_A..=SCRATCH_BANK_B).contains(&destination_bank)
                && source_bank != destination_bank,
            "GPU dispatch point-bank configuration is invalid"
        );
        let indexed_attempts = u64::from(self.lane_count)
            .checked_mul(u64::from(rounds))
            .context("GPU dispatch index range overflow")?;
        ensure!(
            u32::try_from(indexed_attempts).is_ok(),
            "GPU dispatch exceeds exact 32-bit shader indexing"
        );
        base_offset
            .checked_add(indexed_attempts)
            .context("GPU dispatch offset exhausted")?;
        let dispatched_attempts = u64::from(group_count)
            .checked_mul(u64::from(CANDIDATES_PER_GROUP))
            .and_then(|value| value.checked_mul(u64::from(rounds)))
            .context("GPU dispatched-attempt count overflow")?;

        let config = encode_config(ShaderConfig {
            workgroup_count: self.workgroups,
            pattern_count: self.filters.patterns().len(),
            rounds,
            base_offset,
            group_start,
            group_count,
            source_bank,
            destination_bank,
        })?;
        self.context
            .queue
            .write_buffer(&self.config_buffer, 0, &config);
        self.context
            .queue
            .write_buffer(&self.result_buffer, 0, &[0_u8; RESULT_SIZE]);

        let mut encoder = self
            .context
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Onionloom GPU search transaction"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("Onionloom GPU search transaction"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.context.search_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(group_count, 1, 1);
        }
        encoder.copy_buffer_to_buffer(
            &self.result_buffer,
            0,
            &self.readback_buffer,
            0,
            RESULT_BYTES,
        );
        self.context.queue.submit(Some(encoder.finish()));
        let result = read_result(&self.context.device, &self.readback_buffer)?;
        ensure!(
            result.arithmetic_error == 0,
            "GPU symmetry arithmetic encountered an exceptional point pair"
        );
        ensure!(
            u64::from(result.total_matches) <= dispatched_attempts,
            "GPU returned more prehits than candidates in the dispatched range"
        );
        if result.total_matches > RESULT_CAPACITY {
            return Ok(DispatchResult::Overflow);
        }
        ensure!(
            result.offsets.len()
                == usize::try_from(result.total_matches).expect("GPU result count fits usize"),
            "GPU result ring length does not match its prehit count"
        );

        let mut offsets = result.offsets;
        offsets.sort_unstable();
        ensure!(
            offsets.windows(2).all(|pair| pair[0] != pair[1]),
            "GPU returned a duplicate prehit offset"
        );
        let mut matches = Vec::new();
        for offset in offsets {
            ensure_offset_in_dispatch(
                offset,
                base_offset,
                self.workgroups,
                rounds,
                group_start,
                group_count,
            )?;
            let candidate = self.secret.with_offset(offset)?;
            let public = candidate.public_key();
            if self.filters.matches(&public) {
                matches.push(FoundKey {
                    secret: candidate,
                    public,
                    offset,
                });
            }
        }
        Ok(DispatchResult::Complete(matches))
    }

    fn commit(&self, source_bank: u32) -> Result<()> {
        ensure!(
            source_bank == SCRATCH_BANK_A || source_bank == SCRATCH_BANK_B,
            "GPU commit source is not a completed scratch bank"
        );
        let config = encode_config(ShaderConfig {
            workgroup_count: self.workgroups,
            pattern_count: self.filters.patterns().len(),
            rounds: 0,
            base_offset: self.base_offset,
            group_start: 0,
            group_count: self.workgroups,
            source_bank,
            destination_bank: COMMITTED_BANK,
        })?;
        self.context
            .queue
            .write_buffer(&self.config_buffer, 0, &config);
        let mut encoder = self
            .context
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("Onionloom GPU point-state commit"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("Onionloom GPU point-state commit"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.context.commit_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.workgroups.div_ceil(64), 1, 1);
        }
        self.context.queue.submit(Some(encoder.finish()));
        self.context
            .device
            .poll(PollType::Wait)
            .context("GPU point-state commit wait failed")?;
        Ok(())
    }

    /// Measures several complete search chunks and returns their median rate.
    pub fn benchmark(&mut self) -> Result<GpuMeasurement> {
        self.benchmark_rounds(ROUNDS_PER_CHUNK)
    }

    fn benchmark_rounds(&mut self, rounds: u32) -> Result<GpuMeasurement> {
        let _ = self.run_chunk(1)?;
        let mut samples = Vec::with_capacity(3);
        for _ in 0..3 {
            samples.push(self.run_chunk(rounds)?.0);
        }
        samples.sort_by(|left, right| {
            left.attempts_per_second
                .total_cmp(&right.attempts_per_second)
        });
        Ok(samples[1])
    }
}

impl GpuContext {
    fn new() -> Result<Self> {
        let backends = supported_backends();
        let instance = Instance::new(&InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let mut adapters = compatible_adapters(&instance, backends);
        ensure!(
            !adapters.is_empty(),
            "no hardware GPU supports exact shader u64 arithmetic, 128 workgroup lanes, and the required workgroup storage"
        );
        adapters.sort_by_key(adapter_priority);
        let mut failures = Vec::new();
        while let Some(adapter) = adapters.pop() {
            let information = adapter.get_info();
            match Self::from_adapter(adapter) {
                Ok(context) => return Ok(context),
                Err(error) => failures.push(format!(
                    "{} through {}: {error:#}",
                    information.name,
                    information.backend.to_str()
                )),
            }
        }
        bail!(
            "compatible GPU adapters failed validation: {}",
            failures.join("; ")
        )
    }

    fn from_adapter(adapter: Adapter) -> Result<Self> {
        let information = adapter.get_info();
        ensure!(
            matches!(
                information.device_type,
                DeviceType::DiscreteGpu | DeviceType::IntegratedGpu
            ),
            "selected adapter is not a hardware GPU"
        );

        let adapter_limits = adapter.limits();
        ensure!(
            adapter_limits.max_compute_workgroup_storage_size >= WORKGROUP_STORAGE_BYTES,
            "GPU provides too little workgroup memory for exact batch inversion"
        );
        ensure!(
            adapter_limits.max_compute_invocations_per_workgroup >= WORKGROUP_SIZE,
            "GPU provides fewer than {WORKGROUP_SIZE} workgroup lanes"
        );
        ensure!(
            adapter.features().contains(Features::SHADER_INT64),
            "GPU does not support the exact 64-bit integer symmetry shader"
        );
        let mut limits = Limits::downlevel_defaults();
        limits.max_compute_workgroup_storage_size = WORKGROUP_STORAGE_BYTES;
        limits.max_compute_invocations_per_workgroup = WORKGROUP_SIZE;
        limits.max_compute_workgroup_size_x = WORKGROUP_SIZE;
        limits.max_storage_buffers_per_shader_stage = 4;
        limits.max_buffer_size = adapter_limits.max_buffer_size.min(256 * 1024 * 1024);
        limits.max_storage_buffer_binding_size = adapter_limits
            .max_storage_buffer_binding_size
            .min(256 * 1024 * 1024);

        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("Onionloom GPU"),
            required_features: Features::SHADER_INT64,
            required_limits: limits,
            memory_hints: MemoryHints::Performance,
            trace: Trace::Off,
        }))
        .context("failed to open the selected GPU adapter")?;

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("Onionloom GPU bindings"),
            entries: &[
                storage_binding(0, true),
                storage_binding(1, false),
                storage_binding(2, true),
                BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::COMPUTE,
                    ty: BindingType::Buffer {
                        ty: BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(CONFIG_BYTES),
                    },
                    count: None,
                },
                storage_binding(4, false),
            ],
        });
        device.push_error_scope(ErrorFilter::Validation);
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("Onionloom exact Ed25519 search"),
            source: ShaderSource::Wgsl(include_str!("shaders/search.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("Onionloom GPU pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let initialize_pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("Onionloom GPU initialization"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("initialize"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });
        let search_pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("Onionloom GPU search"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("search"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });
        let commit_pipeline = device.create_compute_pipeline(&ComputePipelineDescriptor {
            label: Some("Onionloom GPU point-state commit"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("commit"),
            compilation_options: PipelineCompilationOptions::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            bail!("GPU shader validation failed: {error}");
        }

        Ok(Self {
            device_name: information.name,
            device,
            queue,
            bind_group_layout,
            initialize_pipeline,
            search_pipeline,
            commit_pipeline,
        })
    }
}

/// Verifies decompression, point addition, compression, and result offsets.
pub fn self_test() -> Result<()> {
    self_test_context(Arc::new(GpuContext::new()?))
}

fn self_test_context(context: Arc<GpuContext>) -> Result<()> {
    let secret = ExpandedSecret::generate()?;
    let expected_offset = u64::from(CANDIDATES_PER_GROUP);
    let expected_public = secret.with_offset(expected_offset)?.public_key();
    let address = onion_address(&expected_public);
    let pattern = Pattern::parse(&address[..51])?;
    let filters = Arc::new(FilterSet::new(vec![pattern])?);
    let mut engine = GpuEngine::from_context(Arc::clone(&context), &secret, filters, 1)?;
    let (_, initial_matches) = engine.run_chunk(1)?;
    ensure!(
        initial_matches.is_empty(),
        "GPU self-test prefix matched an unexpected initial point"
    );
    let (_, advanced_matches) = engine.run_chunk(1)?;
    ensure!(
        advanced_matches
            .iter()
            .map(|found| found.offset)
            .eq([expected_offset]),
        "GPU self-test did not reproduce the expected CPU point"
    );

    let all_first_characters = "abcdefghijklmnopqrstuvwxyz234567"
        .chars()
        .map(|character| Pattern::parse(&character.to_string()))
        .collect::<Result<Vec<_>>>()?;
    let filters = Arc::new(FilterSet::new(all_first_characters)?);
    let mut overflow_engine = GpuEngine::from_context(context, &secret, filters, 2)?;
    for chunk in 0_u64..2 {
        let (measurement, matches) = overflow_engine.run_chunk(1)?;
        let expected_start = chunk * u64::from(overflow_engine.lane_count());
        let expected_end = expected_start + u64::from(overflow_engine.lane_count());
        ensure!(
            measurement.attempts == u64::from(overflow_engine.lane_count())
                && measurement.discarded_matches == 0,
            "GPU overflow self-test did not account for every candidate"
        );
        ensure!(
            matches
                .iter()
                .map(|found| found.offset)
                .eq(expected_start..expected_end),
            "GPU overflow self-test did not replay every candidate exactly once"
        );
    }
    Ok(())
}

/// Starts an automatically tuned GPU search with an independent random secret.
pub fn start_search(filters: Arc<FilterSet>) -> Result<GpuSearchSession> {
    let secret = ExpandedSecret::generate()?;
    let mut engine = GpuEngine::new(&secret, filters)?;
    let device_name = engine.device_name().to_owned();
    let measured_rate = engine.measured_rate();
    let stop = Arc::new(AtomicBool::new(false));
    let attempts = Arc::new(AtomicU64::new(0));
    let worker_stop = Arc::clone(&stop);
    let worker_attempts = Arc::clone(&attempts);
    let (sender, matches) = mpsc::sync_channel(8);
    let handle = thread::Builder::new()
        .name("onionloom-gpu".to_owned())
        .spawn(move || {
            let mut dropped = 0_u64;
            'search: while !worker_stop.load(Ordering::Relaxed) {
                let (measurement, found) = engine.run_chunk(ROUNDS_PER_CHUNK)?;
                worker_attempts.fetch_add(measurement.attempts, Ordering::Relaxed);
                dropped = dropped.saturating_add(u64::from(measurement.discarded_matches));
                for found in found {
                    match sender.try_send(found) {
                        Ok(()) => {}
                        Err(mpsc::TrySendError::Full(_)) => dropped = dropped.saturating_add(1),
                        Err(mpsc::TrySendError::Disconnected(_)) => break 'search,
                    }
                }
            }
            Ok(dropped)
        })
        .context("failed to start the GPU search worker")?;
    Ok(GpuSearchSession {
        matches,
        stop,
        attempts,
        handle,
        device_name,
        measured_rate,
    })
}

fn compatible_adapters(instance: &Instance, backends: Backends) -> Vec<Adapter> {
    instance
        .enumerate_adapters(backends)
        .into_iter()
        .filter(adapter_is_compatible)
        .collect()
}

fn adapter_priority(adapter: &Adapter) -> (u8, u8) {
    let information = adapter.get_info();
    let device = match information.device_type {
        DeviceType::DiscreteGpu => 2,
        DeviceType::IntegratedGpu => 1,
        _ => 0,
    };
    let backend = match information.backend {
        Backend::Metal if cfg!(target_os = "macos") => 3,
        Backend::Dx12 if cfg!(target_os = "windows") => 3,
        Backend::Vulkan if cfg!(target_os = "linux") => 3,
        Backend::Vulkan => 2,
        _ => 1,
    };
    (device, backend)
}

pub(super) fn adapter_is_compatible(adapter: &Adapter) -> bool {
    let information = adapter.get_info();
    let limits = adapter.limits();
    matches!(
        information.device_type,
        DeviceType::DiscreteGpu | DeviceType::IntegratedGpu
    ) && adapter.features().contains(Features::SHADER_INT64)
        && limits.max_compute_workgroup_storage_size >= WORKGROUP_STORAGE_BYTES
        && limits.max_compute_invocations_per_workgroup >= WORKGROUP_SIZE
}

pub(super) const fn supported_backends() -> Backends {
    if cfg!(target_os = "windows") {
        Backends::DX12.union(Backends::VULKAN)
    } else if cfg!(target_os = "macos") {
        Backends::METAL
    } else {
        Backends::VULKAN
    }
}

fn storage_binding(binding: u32, read_only: bool) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        visibility: ShaderStages::COMPUTE,
        ty: BindingType::Buffer {
            ty: BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn initial_compressed_points(secret: &ExpandedSecret, workgroups: u32) -> Result<Vec<[u8; 32]>> {
    let point_count = usize::try_from(workgroups + OFFSET_PAIRS + 1)
        .context("GPU point count does not fit memory")?;
    let mut points = Vec::with_capacity(point_count);
    for group in 0..workgroups {
        let center_offset = group
            .checked_mul(CANDIDATES_PER_GROUP)
            .and_then(|offset| offset.checked_add(OFFSET_PAIRS))
            .context("GPU center offset overflow")?;
        points.push(secret.point_at_offset(u64::from(center_offset)));
    }
    for offset in 1..=OFFSET_PAIRS {
        let scalar = Scalar::from(u64::from(offset)) * Scalar::from(8_u64);
        points.push(EdwardsPoint::mul_base(&scalar));
    }
    let stride = workgroups
        .checked_mul(CANDIDATES_PER_GROUP)
        .context("GPU center stride overflow")?;
    let stride_scalar = Scalar::from(u64::from(stride)) * Scalar::from(8_u64);
    points.push(EdwardsPoint::mul_base(&stride_scalar));
    ensure!(
        points.len() == point_count,
        "GPU point initialization count mismatch"
    );
    Ok(points
        .iter()
        .map(EdwardsPoint::compress)
        .map(|point| point.to_bytes())
        .collect())
}

fn encode_compressed_points(points: &[[u8; 32]]) -> Vec<u8> {
    let mut output = Vec::with_capacity(points.len() * 32);
    for point in points {
        for word in point.chunks_exact(4) {
            output.extend_from_slice(
                &u32::from_le_bytes(word.try_into().expect("four-byte point word")).to_le_bytes(),
            );
        }
    }
    output
}

fn encode_patterns(filters: &FilterSet) -> Vec<u8> {
    let mut output = Vec::with_capacity(filters.patterns().len() * 64);
    for pattern in filters.patterns() {
        let (mut value, mut mask) = pattern.packed_mask();
        value[31] &= 0x7f;
        mask[31] &= 0x7f;
        for bytes in [value, mask] {
            for word in bytes.chunks_exact(4) {
                output.extend_from_slice(
                    &u32::from_le_bytes(word.try_into().expect("four-byte pattern word"))
                        .to_le_bytes(),
                );
            }
        }
    }
    output
}

fn encode_config(config: ShaderConfig) -> Result<[u8; CONFIG_SIZE]> {
    let pattern_count =
        u32::try_from(config.pattern_count).context("too many GPU search patterns")?;
    let offset_bytes = config.base_offset.to_le_bytes();
    let values = [
        config.workgroup_count,
        pattern_count,
        config.rounds,
        u32::from_le_bytes(offset_bytes[..4].try_into().expect("four-byte low offset")),
        u32::from_le_bytes(offset_bytes[4..].try_into().expect("four-byte high offset")),
        config.group_start,
        config.group_count,
        config.source_bank,
        config.destination_bank,
        0,
        0,
        0,
    ];
    let mut output = [0_u8; CONFIG_SIZE];
    for (index, value) in values.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(output)
}

fn ensure_offset_in_dispatch(
    offset: u64,
    base_offset: u64,
    workgroups: u32,
    rounds: u32,
    group_start: u32,
    group_count: u32,
) -> Result<()> {
    let chunk_length = u64::from(workgroups)
        .checked_mul(u64::from(rounds))
        .and_then(|blocks| blocks.checked_mul(u64::from(CANDIDATES_PER_GROUP)))
        .context("GPU dispatched chunk length overflow")?;
    let chunk_end = base_offset
        .checked_add(chunk_length)
        .context("GPU dispatched chunk end overflow")?;
    ensure!(
        offset >= base_offset && offset < chunk_end,
        "GPU returned a prehit outside the dispatched chunk bounds"
    );
    let delta = offset
        .checked_sub(base_offset)
        .context("GPU returned a prehit below the dispatched offset")?;
    let block = delta / u64::from(CANDIDATES_PER_GROUP);
    let round = block / u64::from(workgroups);
    let group = block % u64::from(workgroups);
    let group_end = group_start
        .checked_add(group_count)
        .context("GPU dispatch group range overflow")?;
    ensure!(
        round < u64::from(rounds)
            && group >= u64::from(group_start)
            && group < u64::from(group_end),
        "GPU returned a prehit outside the exact dispatched chunk"
    );
    Ok(())
}

fn read_result(device: &Device, buffer: &Buffer) -> Result<DeviceResult> {
    let slice = buffer.slice(..);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    slice.map_async(MapMode::Read, move |result| {
        let _ = sender.send(result);
    });
    device
        .poll(PollType::Wait)
        .context("GPU result wait failed")?;
    receiver
        .recv()
        .context("GPU result callback ended unexpectedly")?
        .context("GPU result mapping failed")?;
    let mapped = slice.get_mapped_range();
    ensure!(
        mapped.len() == RESULT_SIZE,
        "GPU returned an invalid result size"
    );
    let mut words = Vec::with_capacity(RESULT_WORDS);
    for (index, chunk) in mapped.chunks_exact(4).enumerate() {
        debug_assert_eq!(index, words.len());
        words.push(u32::from_le_bytes(
            chunk.try_into().expect("four-byte result word"),
        ));
    }
    drop(mapped);
    buffer.unmap();
    let total_matches = words[0];
    let stored_count = total_matches.min(RESULT_CAPACITY) as usize;
    let mut offsets = Vec::with_capacity(stored_count);
    for index in 0..stored_count {
        let word_index = RESULT_HEADER_WORDS + index * 2;
        offsets.push(u64::from(words[word_index]) | (u64::from(words[word_index + 1]) << 32));
    }
    Ok(DeviceResult {
        total_matches,
        arithmetic_error: words[1],
        offsets,
    })
}

#[cfg(test)]
mod tests {
    use super::{CANDIDATES_PER_GROUP, ensure_offset_in_dispatch};

    #[test]
    fn offset_range_accepts_only_dispatched_groups_and_rounds() {
        let base = u64::from(u32::MAX) + 100;
        let workgroups = 8;
        let rounds = 3;
        let group_start = 2;
        let group_count = 3;
        for round in 0..rounds {
            for group in group_start..group_start + group_count {
                let delta = u64::from(round * workgroups + group) * u64::from(CANDIDATES_PER_GROUP);
                ensure_offset_in_dispatch(
                    base + delta,
                    base,
                    workgroups,
                    rounds,
                    group_start,
                    group_count,
                )
                .expect("first candidate belongs to dispatch");
                ensure_offset_in_dispatch(
                    base + delta + u64::from(CANDIDATES_PER_GROUP - 1),
                    base,
                    workgroups,
                    rounds,
                    group_start,
                    group_count,
                )
                .expect("last candidate belongs to dispatch");
            }
        }

        assert!(
            ensure_offset_in_dispatch(base - 1, base, workgroups, rounds, group_start, group_count)
                .is_err()
        );
        assert!(
            ensure_offset_in_dispatch(
                base + u64::from(CANDIDATES_PER_GROUP),
                base,
                workgroups,
                rounds,
                group_start,
                group_count
            )
            .is_err()
        );
        assert!(
            ensure_offset_in_dispatch(
                base + u64::from(rounds * workgroups * CANDIDATES_PER_GROUP),
                base,
                workgroups,
                rounds,
                group_start,
                group_count
            )
            .is_err()
        );
    }
}
