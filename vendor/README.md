# Vendored crates

## `iced_wgpu` 0.14.0

Used through `[patch.crates-io]` in the workspace `Cargo.toml`. The first
commit that added it is an unmodified copy from crates.io, so
`git diff <that commit> -- vendor/iced_wgpu` shows the whole patch:

- `src/shader/triangle/gradient_webgl.wgsl` (new): a copy of
  `triangle/gradient.wgsl` whose fragment stage reads the 8 gradient stops by
  index from the packed vertex outputs instead of building
  `array<vec4<f32>, 8>`/`array<f32, 8>` locals. Same maths, no arrays.
- `src/triangle.rs`: a `gradient_wgsl!()` macro picks that file on
  `target_arch = "wasm32"` and the upstream `gradient.wgsl` everywhere else,
  so native builds compile exactly the upstream shader.

Why: over plain HTTP the browser has no WebGPU, so wgpu uses WebGL2 and naga
emits GLSL ES. ANGLE re-emits that GLSL for the driver with a precision on
every declaration but no default `precision highp float;`, and writes the
arrays as `vec4[8](...)` constructors (it also zero-fills any local array that
way). Mali's compiler rejects those: `S0032: no default precision defined for
variable 'vec4[8]'`, and wgpu panics creating
`iced_wgpu.triangle.gradient.pipeline`. See
`crates/taskboard-wasm-spike/SPIKE.md` for details.

The other iced shaders don't need this. `quad/gradient.wgsl` has the same
arrays, but iced already skips that pipeline on wasm32. The remaining arrays
(`blit.wgsl`, `image.wgsl`) are in vertex shaders, where GLSL ES predeclares
`highp float`.

Drop this patch when iced upgrades past 0.14 if upstream has fixed it: delete
`vendor/iced_wgpu`, the `[patch.crates-io]` entry and the `exclude`, and test
the web build on a Mali phone over plain HTTP.
