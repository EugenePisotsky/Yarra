// Stable integer hashes. Every random choice is a function of seeds and lattice cells, never of
// dispatch or append order.

fn hash32(value: u32) -> u32 {
    var x = value;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    return x ^ (x >> 16u);
}

fn hash_cell(seed: u32, cell: vec2<i32>, salt: u32) -> u32 {
    return hash32(
        seed
        ^ (bitcast<u32>(cell.x) * 0x8da6b343u)
        ^ (bitcast<u32>(cell.y) * 0xd8163841u)
        ^ salt,
    );
}

fn random01(value: u32) -> f32 {
    return f32(hash32(value)) * (1.0 / 4294967295.0);
}
