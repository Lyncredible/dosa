struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(0) var framebuffer: texture_2d<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> Vertex {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return Vertex(vec4<f32>(corner * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0), corner);
}

fn sample_frame(uv: vec2<f32>) -> vec4<f32> {
    let size = textureDimensions(framebuffer);
    let last_pixel = vec2<i32>(size) - vec2<i32>(1);
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), last_pixel);
    return textureLoad(framebuffer, pixel, 0);
}

@fragment
fn fragment(vertex: Vertex) -> @location(0) vec4<f32> {
    return vec4<f32>(sample_frame(vertex.uv).rgb, 1.0);
}

@fragment
fn overlay_fragment(vertex: Vertex) -> @location(0) vec4<f32> {
    return sample_frame(vertex.uv);
}
