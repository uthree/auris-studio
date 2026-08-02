// Waveform bucket reduction: one invocation reduces one horizontal pixel of a clip.
//
// See `waveform.rs` for the host side. The output slot layout is
// (minimum, maximum, RMS, sample count) so the host can rebuild a partial trailing bucket
// without recomputing its length.

struct Params {
    // Samples uploaded for this dispatch.
    sample_count: u32,
    // Samples reduced into one bucket.
    samples_per_bucket: u32,
    // Buckets this dispatch is responsible for.
    bucket_count: u32,
    // WGSL requires a uniform struct size that is a multiple of 16 bytes.
    padding: u32,
}

@group(0) @binding(0) var<storage, read> samples: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let bucket = id.x;
    if (bucket >= params.bucket_count) {
        return;
    }

    let start = bucket * params.samples_per_bucket;
    if (start >= params.sample_count) {
        output[bucket] = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        return;
    }
    let end = min(start + params.samples_per_bucket, params.sample_count);

    // Seeding from the first sample rather than from +/-infinity keeps the result identical to
    // the CPU reference for buckets that are entirely positive or entirely negative.
    var lowest = samples[start];
    var highest = lowest;
    var sum_squares = lowest * lowest;
    for (var i = start + 1u; i < end; i = i + 1u) {
        let value = samples[i];
        lowest = min(lowest, value);
        highest = max(highest, value);
        sum_squares = sum_squares + value * value;
    }

    let count = f32(end - start);
    output[bucket] = vec4<f32>(lowest, highest, sqrt(sum_squares / count), count);
}
