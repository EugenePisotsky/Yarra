fn hash_2d(point: vec2<f32>) -> f32 {
    var value = fract(vec3(point.x, point.y, point.x) * 0.1031);
    value += dot(value, value.yzx + vec3(33.33));
    return fract((value.x + value.y) * value.z);
}

fn quarter_turn(value: vec2<f32>, turn: f32) -> vec2<f32> {
    if turn < 0.5 {
        return value;
    }
    if turn < 1.5 {
        return vec2(-value.y, value.x);
    }
    if turn < 2.5 {
        return -value;
    }
    return vec2(value.y, -value.x);
}

fn stochastic_vertex_turn(vertex: vec2<f32>, material_layer: i32) -> f32 {
    let layer_seed = f32(material_layer) * 19.19;
    return floor(hash_2d(vertex + vec2(layer_seed, layer_seed * 0.37)) * 4.0);
}

fn stochastic_vertex_offset(vertex: vec2<f32>, material_layer: i32) -> vec2<f32> {
    let layer_seed = f32(material_layer) * 23.71;
    let seeded = vertex + vec2(layer_seed, -layer_seed * 0.41);
    return vec2(
        hash_2d(seeded + vec2(17.0, 3.0)),
        hash_2d(seeded + vec2(5.0, 29.0)),
    ) * 31.0;
}
