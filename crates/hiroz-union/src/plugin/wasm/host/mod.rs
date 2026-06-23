//! WIT bindings and host implementation submodules.

pub mod graph;
pub mod render;
pub mod ros;
pub mod transport;

use wasmtime::component::bindgen;

bindgen!({
    world: "hu-plugin",
    path: "wit/hu-plugin.wit",
});
