// Whole-buffer loudness reduction: one invocation reduces one contiguous span of samples.
//
// See `analysis.rs` for the host side, which sums the partial results in f64. The output slot
// layout is (minimum, maximum, sum of squares, inter-sample peak estimate).
//
// Indices are relative to the uploaded slice, which may start one sample before and end two
// samples after the range being reduced so that the interpolator's neighbours are real samples
// even when a long channel is split across several dispatches.

struct Params {
    // Samples uploaded for this dispatch.
    sample_count: u32,
    // First uploaded sample this dispatch reduces.
    first_active: u32,
    // One past the last uploaded sample this dispatch reduces.
    last_active: u32,
    // Samples reduced by one invocation.
    span: u32,
}

@group(0) @binding(0) var<storage, read> samples: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> params: Params;

// Reads a sample, clamping to the uploaded range. Matches `sample_clamped` in `analysis.rs`.
fn sample_at(index: u32) -> f32 {
    return samples[min(index, params.sample_count - 1u)];
}

// Catmull-Rom interpolation through four consecutive samples, evaluated between p1 and p2.
// Must match `catmull_rom` in `analysis.rs` exactly, including the order of operations.
fn catmull_rom(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = 2.0 * p1;
    let b = p2 - p0;
    let c = 2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3;
    let d = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    return 0.5 * (a + b * t + c * t * t + d * t * t * t);
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let start = params.first_active + id.x * params.span;
    // Invocations past the end of the work have no output slot of their own, so they must
    // return before writing anything.
    if (start >= params.last_active) {
        return;
    }
    let end = min(start + params.span, params.last_active);

    var lowest = samples[start];
    var highest = lowest;
    var sum_squares = 0.0;
    var true_peak = 0.0;
    for (var i = start; i < end; i = i + 1u) {
        let value = samples[i];
        lowest = min(lowest, value);
        highest = max(highest, value);
        sum_squares = sum_squares + value * value;
        true_peak = max(true_peak, abs(value));

        let p0 = sample_at(max(i, 1u) - 1u);
        let p2 = sample_at(i + 1u);
        let p3 = sample_at(i + 2u);
        true_peak = max(true_peak, abs(catmull_rom(p0, value, p2, p3, 0.25)));
        true_peak = max(true_peak, abs(catmull_rom(p0, value, p2, p3, 0.5)));
        true_peak = max(true_peak, abs(catmull_rom(p0, value, p2, p3, 0.75)));
    }

    output[id.x] = vec4<f32>(lowest, highest, sum_squares, true_peak);
}
