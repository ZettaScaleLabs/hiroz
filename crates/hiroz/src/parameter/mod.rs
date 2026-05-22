//! ROS 2 parameter support for hiroz native nodes.
//!
//! This module provides a complete ROS 2-compatible parameter subsystem:
//!
//! - **Parameter storage** with typed values and descriptors
//! - **Validation callbacks** for accepting/rejecting parameter changes
//! - **Standard parameter services** (get, set, describe, list, etc.)
//! - **Parameter events** published on `/parameter_events`
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────┐
//! │                         ZNode                                │
//! ├──────────────────────────────────────────────────────────────┤
//! │  ParameterService                                            │
//! │  ├── store: Arc<RwLock<ParameterStore>>                      │
//! │  ├── event_publisher: ZPub<ParameterEvent>                   │
//! │  ├── on_set_callback: validation callback                    │
//! │  └── servers:                                                │
//! │      ├── ~describe_parameters                                │
//! │      ├── ~get_parameters                                     │
//! │      ├── ~get_parameter_types                                │
//! │      ├── ~list_parameters                                    │
//! │      ├── ~set_parameters                                     │
//! │      └── ~set_parameters_atomically                          │
//! └──────────────────────────────────────────────────────────────┘
//! ```

pub mod client;
pub mod service;
pub mod store;
pub mod types;
pub mod wire_types;
pub mod yaml;

pub use client::{ParameterClient, ParameterList, ParameterTarget};
pub use service::ParameterService;
pub use types::{
    FloatingPointRange, IntegerRange, Parameter, ParameterDescriptor, ParameterType,
    ParameterValue, SetParametersResult,
};
