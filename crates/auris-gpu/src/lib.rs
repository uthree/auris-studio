//! Optional GPU compute acceleration for Auris Studio.
//!
//! Everything in this crate is an *optimisation*. [`GpuContext::new`] returns `None` when no
//! adapter is available, every kernel returns `None` rather than panicking when wgpu reports a
//! problem, and every public entry point has a CPU implementation that produces the same
//! numbers. A build with no working GPU behaves identically, only slower.
//!
//! # When the GPU is used
//!
//! Only for **offline, whole-file reductions**:
//!
//! * [`waveform::compute_peaks`] — a fixed min/max/RMS overview computed once when a decoded
//!   source is installed and cached by the session. Zoom and scroll re-bin the cached result.
//! * [`analysis::analyze_loudness`] — peak, RMS and an inter-sample peak estimate over a whole
//!   rendered mix. The session's analysis command and automatic balance pass use the
//!   GPU-preferring wrapper and fall back to the CPU reference. The export dialog reports
//!   [`AudioBuffer::peak`](auris_core::AudioBuffer::peak), the loudest stored sample. Wiring the
//!   GPU path into export means giving `auris_session::RenderJob` a context it deliberately does
//!   not have: it is built to be self-contained and handed to a worker thread. Left here rather
//!   than deleted because the kernel is finished and tested against its own CPU reference.
//!
//! Both are embarrassingly parallel: the output for one bucket depends on no other bucket, so
//! the work maps onto thousands of invocations with no cross-thread communication.
//!
//! **Realtime DSP is deliberately not on that list.** At 48 kHz a 256-frame block must be
//! rendered in 5.3 ms, and the engine budgets a fraction of that for any single processor.
//! A GPU round trip — upload, dispatch, fence, map, read back — costs on the order of a
//! millisecond even when the arithmetic is free, because it is dominated by driver submission
//! and PCIe/unified-memory synchronisation latency rather than by throughput. Per-block DSP
//! would therefore be slower *and* would drag a GPU driver onto the CoreAudio callback thread,
//! where it may allocate, lock and block. Instruments and effects stay on the CPU.
//!
//! # Adding a kernel
//!
//! Kernels in this crate all have the same shape: read a big `array<f32>`, write one
//! `vec4<f32>` per output slot, no communication between slots. That shape is implemented once
//! by [`GpuContext::run_reduction`], so a new pass needs only three pieces:
//!
//! 1. A WGSL source string with a `main` entry point and the bindings described on
//!    [`GpuContext::run_reduction`]. Store it in your module with `include_str!` or a `const`.
//! 2. A `#[repr(C)]`, [`bytemuck::Pod`] parameter struct whose size is a multiple of 16 bytes
//!    (a WGSL `uniform` requirement), passed straight through to `@binding(2)`.
//! 3. A host function that splits the input into chunks no larger than
//!    [`GpuContext::max_input_samples`], calls `run_reduction` per chunk, and folds the
//!    `[f32; 4]` slots into whatever the caller wants.
//!
//! Write the CPU version first and make it the reference the GPU version is tested against;
//! see [`waveform::compute_peaks_cpu`] for the pattern. Pipelines are cached by label inside
//! the context, so a kernel is compiled at most once per process.

#![warn(missing_docs)]

pub mod analysis;
pub mod context;
pub mod waveform;

pub use analysis::{LoudnessAnalysis, analyze_loudness, analyze_loudness_cpu};
pub use context::GpuContext;
pub use waveform::{WaveformBucket, WaveformPeaks, compute_peaks, compute_peaks_cpu};
