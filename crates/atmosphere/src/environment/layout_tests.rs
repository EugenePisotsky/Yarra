//! The shaders' `EnvironmentParams` (`shaders/environment/params.wesl`) mirrors the Rust struct
//! by hand; a field added, removed or moved on one side only shifts every field after it.
use super::EnvironmentParams;

const HARNESS: &str = "
import package::shaders::environment::params::EnvironmentParams;
@group(0) @binding(0) var<storage, read> params: EnvironmentParams;
@compute @workgroup_size(1) fn main() { _ = params.layer; }
";

/// The struct's field names, in order, from its Rust source.
fn rust_fields(source: &str) -> Vec<String> {
    let start = source
        .find("pub(crate) struct EnvironmentParams {")
        .expect("the Rust struct");
    let body = &source[start..];
    let body = &body[..body.find("\n}").expect("the struct's end")];
    body.lines()
        .filter_map(|line| line.trim().strip_prefix("pub "))
        .filter_map(|field| field.split_once(':').map(|(name, _)| name.to_owned()))
        .collect()
}

#[test]
fn shader_params_match_the_rust_struct() {
    let wgsl = shader_check::Shaders::get()
        .compose_source(HARNESS, &[])
        .unwrap_or_else(|e| panic!("{e}"));
    let module = shader_check::validate(&wgsl).unwrap_or_else(|e| panic!("{e}"));
    let (members, span) = module
        .types
        .iter()
        .find_map(|(_, ty)| match &ty.inner {
            naga::TypeInner::Struct { members, span }
                if ty
                    .name
                    .as_deref()
                    .is_some_and(|n| n.ends_with("EnvironmentParams")) =>
            {
                Some((members.clone(), *span))
            }
            _ => None,
        })
        .expect("EnvironmentParams in the composed shader");
    let shader: Vec<_> = members.iter().filter_map(|m| m.name.clone()).collect();
    assert_eq!(shader, rust_fields(include_str!("mod.rs")));
    assert_eq!(span as usize, std::mem::size_of::<EnvironmentParams>());
}
