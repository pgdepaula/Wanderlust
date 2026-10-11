@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) index: u32) -> Vertex {
    let positions = array<vec2<f32>, 3>(vec2(-1.0,-1.0), vec2(3.0,-1.0), vec2(-1.0,3.0));
    let p = positions[index];
    var output: Vertex;
    output.position = vec4(p, 0.0, 1.0);
    output.uv = vec2((p.x + 1.0) * 0.5, (1.0 - p.y) * 0.5);
    return output;
}
@fragment fn fs_main(v: Vertex) -> @location(0) vec4<f32> {
    return textureSampleLevel(image, image_sampler, v.uv, 0.0);
}
