// taskboard patch: an array-free copy of `gradient.wgsl`, used only on wasm32
// (see `triangle.rs` and `vendor/README.md` in the taskboard repo).
//
// On WebGL2, wgpu/naga turns `gradient.wgsl` into GLSL ES with local
// `vec4[8]`/`float[8]` arrays. ANGLE re-emits that GLSL with a precision on
// every declaration but no default `precision highp float;`, and it
// zero-initialises local arrays with `vec4[8](...)` constructors. Mali's GLES
// compiler (e.g. Mali-G715 under Chrome for Android) then rejects the shader:
// "S0032: no default precision defined for variable 'vec4[8]'". This version
// computes the same colour without any array: the stops are read straight from
// the packed vertex outputs by index.
//
// The vertex stage and the gradient maths are unchanged from `gradient.wgsl`.

struct GradientVertexInput {
    @location(0) v_pos: vec2<f32>,
    @location(1) @interpolate(flat) colors_1: vec4<u32>,
    @location(2) @interpolate(flat) colors_2: vec4<u32>,
    @location(3) @interpolate(flat) colors_3: vec4<u32>,
    @location(4) @interpolate(flat) colors_4: vec4<u32>,
    @location(5) @interpolate(flat) offsets: vec4<u32>,
    @location(6) direction: vec4<f32>,
}

struct GradientVertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) raw_position: vec2<f32>,
    @location(1) @interpolate(flat) colors_1: vec4<u32>,
    @location(2) @interpolate(flat) colors_2: vec4<u32>,
    @location(3) @interpolate(flat) colors_3: vec4<u32>,
    @location(4) @interpolate(flat) colors_4: vec4<u32>,
    @location(5) @interpolate(flat) offsets: vec4<u32>,
    @location(6) direction: vec4<f32>,
}

@vertex
fn gradient_vs_main(input: GradientVertexInput) -> GradientVertexOutput {
    var output: GradientVertexOutput;

    output.position = globals.transform * vec4<f32>(input.v_pos, 0.0, 1.0);
    output.raw_position = input.v_pos;
    output.colors_1 = input.colors_1;
    output.colors_2 = input.colors_2;
    output.colors_3 = input.colors_3;
    output.colors_4 = input.colors_4;
    output.offsets = input.offsets;
    output.direction = input.direction;

    return output;
}

/// Stop `i` (0..=7) of the gradient: what `colors[i]` is in `gradient.wgsl`.
fn stop_color(input: GradientVertexOutput, i: i32) -> vec4<f32> {
    var packed: vec4<u32>;
    switch (i / 2) {
        case 0: { packed = input.colors_1; }
        case 1: { packed = input.colors_2; }
        case 2: { packed = input.colors_3; }
        default: { packed = input.colors_4; }
    }
    return unpack_color(select(packed.zw, packed.xy, i % 2 == 0));
}

/// Offset of stop `i` (0..=7): what `offsets[i]` is in `gradient.wgsl`.
fn stop_offset(offsets_1: vec4<f32>, offsets_2: vec4<f32>, i: i32) -> f32 {
    if (i < 4) {
        return offsets_1[i];
    }
    return offsets_2[i - 4];
}

/// Returns the current interpolated color with a max 8-stop gradient
fn gradient(
    input: GradientVertexOutput,
    offsets_1: vec4<f32>,
    offsets_2: vec4<f32>,
    last_index: i32
) -> vec4<f32> {
    let raw_position = input.raw_position;
    let direction = input.direction;

    let start = direction.xy;
    let end = direction.zw;

    let v1 = end - start;
    let v2 = raw_position - start;
    let unit = normalize(v1);
    let coord_offset = dot(unit, v2) / length(v1);

    var color: vec4<f32>;

    let noise_granularity: f32 = 0.3/255.0;

    for (var i: i32 = 0; i < last_index; i++) {
        let curr_offset = stop_offset(offsets_1, offsets_2, i);
        let next_offset = stop_offset(offsets_1, offsets_2, i + 1);

        if (coord_offset <= stop_offset(offsets_1, offsets_2, 0)) {
            color = stop_color(input, 0);
        }

        if (curr_offset <= coord_offset && coord_offset <= next_offset) {
            let from_ = stop_color(input, i);
            let to_ = stop_color(input, i + 1);
            let factor = smoothstep(curr_offset, next_offset, coord_offset);

            color = interpolate_color(from_, to_, factor);
        }

        if (coord_offset >= stop_offset(offsets_1, offsets_2, last_index)) {
            color = stop_color(input, last_index);
        }
    }

    return color + mix(-noise_granularity, noise_granularity, random(raw_position));
}

@fragment
fn gradient_fs_main(input: GradientVertexOutput) -> @location(0) vec4<f32> {
    let offsets_1: vec4<f32> = unpack_u32(input.offsets.xy);
    let offsets_2: vec4<f32> = unpack_u32(input.offsets.zw);

    var last_index = 7;
    for (var i: i32 = 0; i <= 7; i++) {
        if (stop_offset(offsets_1, offsets_2, i) >= 1.0) {
            last_index = i;
            break;
        }
    }

    return gradient(input, offsets_1, offsets_2, last_index);
}

fn random(coords: vec2<f32>) -> f32 {
    return fract(sin(dot(coords, vec2(12.9898,78.233))) * 43758.5453);
}
