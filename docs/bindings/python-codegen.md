# Python Code Generation Internals

This chapter explains how hiroz generates Python bindings for ROS 2 messages, including the architecture, generated code structure, and design rationale.

## Architecture Overview

hiroz uses a **hybrid approach**: Python classes for ergonomics, Rust for serialization performance. The architecture uses **derive macros** for automatic Python-Rust conversion of arbitrarily nested message types.

```mermaid
graph TB
accTitle: Python bindings code generation architecture from msg files to runtime
accDescr: At build time hiroz-codegen parses msg files, resolves dependencies, and generates both Python msgspec structs and Rust PyO3 bindings; at runtime Python objects are converted to Rust structs and serialized to CDR bytes via derive macros.
    subgraph "Build Time (cargo build)"
        ROS[".msg/.srv files"] --> Parser["hiroz-codegen<br/>Parser"]
        Parser --> Resolver["Dependency<br/>Resolver"]
        Resolver --> PyGen["Python Generator"]
        Resolver --> RsGen["Rust Generator"]
        PyGen --> PyFiles["*.py files<br/>(msgspec structs)"]
        RsGen --> RsFile["Rust structs with<br/>derive macros"]
        RsGen --> PyO3File["python_bindings.rs<br/>(serialize/deserialize)"]
    end

    subgraph "Derive Macros (hiroz-derive)"
        Derive["FromPyMessage<br/>IntoPyMessage"]
        Derive -->|"generates"| Extract["Field extraction code"]
        Derive -->|"generates"| Construct["Object construction code"]
    end

    subgraph "Runtime"
        PyFiles --> PyObj["Python Object"]
        PyObj -->|"FromPyMessage::from_py()"| RsStruct["Rust Struct"]
        RsStruct -->|"hiroz-cdr::to_vec()"| CDR["CDR Bytes"]
        CDR -->|"Eclipse Zenoh"| Network["Network"]
    end
```

### Key Components

| Component | Location | Purpose |
|-----------|----------|---------|
| **Python Generator** | `hiroz-codegen/src/generator/python.rs` | Generates msgspec struct classes |
| **Rust Generator** | `hiroz-codegen/src/generator/rust.rs` | Generates Rust structs with derive attributes |
| **PyO3 Generator** | `hiroz-codegen/src/python_msgspec_generator.rs` | Generates serialize/deserialize wrapper functions |
| **Derive Macros** | `hiroz-derive/src/lib.rs` | `FromPyMessage` and `IntoPyMessage` derive macros |
| **Bridge Traits** | `hiroz/src/python_bridge.rs` | Core traits for Python-Rust conversion |
| **Build Script** | `hiroz-msgs/build.rs` | Orchestrates code generation at build time |
| **Generated Python** | `hiroz-msgs/python/hiroz_msgs_py/types/*.py` | One file per ROS package |
| **Generated Rust** | `$OUT_DIR/python_bindings.rs` | Compiled into `hiroz-msgs` crate |

## Generated Code Examples

### Python: msgspec Structs

The codegen tool generates a frozen `msgspec.Struct` for each ROS message:

```python
# Generated from std_msgs/msg/String.msg
class String(msgspec.Struct, frozen=True, kw_only=True):
    data: str = ""

    __msgtype__: ClassVar[str] = 'std_msgs/msg/String'
    __hash__: ClassVar[str] = 'RIHS01_df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18'
```

Nested messages use forward references:

```python
# Generated from geometry_msgs/msg/Twist.msg
class Twist(msgspec.Struct, frozen=True, kw_only=True):
    linear: "geometry_msgs.Vector3 | None" = None
    angular: "geometry_msgs.Vector3 | None" = None

    __msgtype__: ClassVar[str] = 'geometry_msgs/msg/Twist'
    __hash__: ClassVar[str] = 'RIHS01_9c45bf16fe0983d80e3cfe750d6835843d265a9a6c46bd2e609fcddde6fb8d2a'
```

Service request/response types use the service type hash in `__hash__`:

```python
# Generated from example_interfaces/srv/AddTwoInts.srv
class AddTwoIntsRequest(msgspec.Struct, frozen=True, kw_only=True):
    a: int = 0
    b: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/AddTwoIntsRequest'
    __hash__: ClassVar[str] = 'RIHS01_e118de6bf5eeb66a2491b5bda11202e7b68f198d6f67922cf30364858239c81a'

class AddTwoIntsResponse(msgspec.Struct, frozen=True, kw_only=True):
    sum: int = 0

    __msgtype__: ClassVar[str] = 'example_interfaces/msg/AddTwoIntsResponse'
    __hash__: ClassVar[str] = 'RIHS01_e118de6bf5eeb66a2491b5bda11202e7b68f198d6f67922cf30364858239c81a'
```

For service types, `__hash__` contains the service type hash (computed from the combined request/response definition). Both request and response share the same hash since they belong to the same service. This differs from regular messages where `__hash__` contains the individual message type hash.

### Rust: Generated Structs with Derive Macros

The Rust code generator adds derive attributes to message structs:

```rust
// Generated from std_msgs/msg/String.msg
#[derive(Debug, Clone, Default, ::serde::Serialize, ::serde::Deserialize)]
#[cfg_attr(feature = "python_registry", derive(::hiroz_derive::FromPyMessage, ::hiroz_derive::IntoPyMessage))]
#[cfg_attr(feature = "python_registry", ros_msg(module = "hiroz_msgs_py.types.std_msgs"))]
pub struct String {
    pub data: std::string::String,
}
```

For nested messages with optional fields:

```rust
// Generated from geometry_msgs/msg/Twist.msg
#[derive(Debug, Clone, Default, ::serde::Serialize, ::serde::Deserialize)]
#[cfg_attr(feature = "python_registry", derive(::hiroz_derive::FromPyMessage, ::hiroz_derive::IntoPyMessage))]
#[cfg_attr(feature = "python_registry", ros_msg(module = "hiroz_msgs_py.types.geometry_msgs"))]
pub struct Twist {
    pub linear: Vector3,
    pub angular: Vector3,
}
```

### Derive Macros: Zero-Copy Byte Array Handling

Fields annotated with `#[ros_msg(zbuf)]` (applied automatically by the code generator for `uint8[]`/`byte[]` fields) get special treatment in the derive macros:

**`FromPyMessage`** (Python → Rust) tries three paths in order:

1. **ZBufView** — extracts the inner `ZBuf` via `clone()` (reference-counted, zero-copy)
2. **`bytes`/`bytearray`** — copies into a new `ZBuf`
3. **`list[int]`** — fallback extraction

```rust
// Generated by FromPyMessage derive macro for #[ros_msg(zbuf)] fields
let py_attr = obj.getattr("data")?;
if let Ok(view) = py_attr.downcast::<ZBufView>() {
    view.borrow().zbuf().clone()     // Arc clone — no data copy
} else if let Ok(bytes) = py_attr.downcast::<PyBytes>() {
    ZBuf::from(bytes.as_bytes().to_vec())
} else if let Ok(bytearray) = py_attr.downcast::<PyByteArray>() {
    ZBuf::from(unsafe { bytearray.as_bytes() }.to_vec())
} else {
    let bytes: Vec<u8> = py_attr.extract()?;
    ZBuf::from(bytes)
}
```

**`IntoPyMessage`** (Rust → Python) creates a `ZBufView` instead of copying into `PyBytes`:

```rust
// Generated by IntoPyMessage derive macro for #[ros_msg(zbuf)] fields
let zbuf_view = ZBufView::new(self.data.clone());
let py_view = Py::new(py, zbuf_view)?;
kwargs.set_item("data", py_view)?;
```

This means you can pass a received message's byte array fields directly to a new message for re-publishing without any data copy.

### Rust: PyO3 Serialize/Deserialize Functions

The serialize/deserialize functions use the derive macro traits:

```rust
// Generated serialize function - uses FromPyMessage trait
#[pyfunction]
pub fn serialize_string(py: Python, msg: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    use ::pyo3::types::PyAnyMethods;
    use ::hiroz::python_bridge::FromPyMessage;

    // Derive macro handles all field extraction recursively
    let rust_msg = <ros::std_msgs::String>::from_py(msg)?;

    // CDR serialization with 4-byte encapsulation header
    let mut cdr_data = hiroz_cdr::to_vec::<_, hiroz_cdr::LittleEndian>(&rust_msg, 256)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let mut result = vec![0x00, 0x01, 0x00, 0x00]; // CDR encapsulation header
    result.append(&mut cdr_data);
    Ok(result)
}

// Generated deserialize function - uses IntoPyMessage trait
#[pyfunction]
pub fn deserialize_string(py: Python, bytes: &[u8]) -> PyResult<PyObject> {
    use ::hiroz::python_bridge::IntoPyMessage;

    // Skip 4-byte CDR encapsulation header
    let cdr_data = &bytes[4..];
    let (rust_msg, _): (ros::std_msgs::String, _) = hiroz_cdr::from_bytes::<_, hiroz_cdr::LittleEndian>(cdr_data)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

    // Derive macro handles all field construction recursively
    rust_msg.into_py_message(py)
}
```

## Data Flow

### Publishing (Python to Wire)

```mermaid
sequenceDiagram
accTitle: Python to wire publishing data flow through PyO3 and CDR
accDescr: The Python app creates a msgspec message, passes it to PyO3 which extracts each field, constructs a Rust struct, serializes it to CDR bytes, and sends them over the Zenoh network.
    participant App as Python App
    participant Msg as msgspec.Struct
    participant PyO3 as PyO3 Layer
    participant Rust as Rust Struct
    participant CDR as CDR Encoder
    participant Net as Zenoh Network

    App->>Msg: Create message
    App->>PyO3: publish(msg)
    PyO3->>PyO3: getattr() each field
    PyO3->>Rust: Construct Rust struct
    Rust->>CDR: cdr::serialize()
    CDR->>Net: Send bytes
```

### Subscribing (Wire to Python)

```mermaid
sequenceDiagram
accTitle: Wire to Python subscribing data flow with zero-copy deserialization
accDescr: Zenoh delivers CDR bytes which are zero-copy deserialized into a Rust struct; PyO3 builds keyword arguments and constructs the msgspec Python object returned to the application.
    participant Net as Zenoh Network
    participant CDR as CDR Decoder
    participant Rust as Rust Struct
    participant PyO3 as PyO3 Layer
    participant Msg as msgspec.Struct
    participant App as Python App

    Net->>CDR: Receive bytes
    Note over CDR: Store ZBuf in thread-local<br/>for zero-copy deser
    CDR->>Rust: cdr::deserialize()
    Note over CDR,Rust: byte[] fields → ZSlice sub-view<br/>(zero-copy)
    Rust->>PyO3: Build PyDict kwargs
    Note over PyO3: byte[] → ZBufView<br/>(buffer protocol)
    PyO3->>Msg: Call class(**kwargs)
    Msg->>App: Return message
```

## Type Mapping

### ROS to Python Type Mapping

| ROS Type | Python Type | Notes |
|----------|-------------|-------|
| `bool` | `bool` | |
| `byte`, `uint8`, `int8`, `int16`, `uint16`, `int32`, `uint32`, `int64`, `uint64` | `int` | Python has arbitrary precision |
| `float32`, `float64` | `float` | |
| `string` | `str` | |
| `T[]` (unbounded array) | `list[T]` | |
| `T[N]` (fixed array) | `list[T]` | Size not enforced in Python |
| `uint8[]` / `byte[]` | `ZBufView` | Zero-copy buffer protocol (on receive); accepts `bytes`, `bytearray`, or `ZBufView` on publish |
| Nested message | `"pkg.MsgName \| None"` | Forward reference string |


## Design: Hybrid Python + Rust with Derive Macros

```text
┌─────────────────────────────────────────────────────────────┐
│                    Current Architecture                      │
├─────────────────────────────────────────────────────────────┤
│  Python Layer          │  Rust Layer (via PyO3)             │
│  ─────────────         │  ──────────────────────            │
│  • msgspec structs     │  • Derive macros                   │
│  • Type annotations    │  • Trait-based conversion          │
│  • IDE support         │  • CDR serialization               │
│  • User-facing API     │  • Zenoh transport                 │
│                        │  • Memory management               │
└─────────────────────────────────────────────────────────────┘
```

**Features:**

- Fast serialization (Rust CDR implementation)
- Type-safe Python classes with IDE support
- Immutable messages (`frozen=True`)
- Automatic type hash validation
- Zero-copy byte array handling: `ZBufView` exposes received data via buffer protocol without copying; `FromPyMessage` detects `ZBufView` inputs and extracts the inner buffer via reference counting
- **Derive macros handle arbitrary nesting depth automatically**
- **None handling for optional nested fields**
- **Minimal generated code (traits do the work)**


## Build Process

### Step-by-step Flow

```mermaid
flowchart TD
accTitle: Python bindings build process from cargo build to compiled output
accDescr: Running cargo build triggers build.rs which discovers ROS packages, parses msg files, resolves dependencies, then generates both Python type files and a Rust PyO3 module compiled into hiroz-msgs.
    A["cargo build hiroz-msgs<br/>--features python_registry"] --> B["build.rs executes"]
    B --> C["Discover ROS packages<br/>(AMENT_PREFIX_PATH or bundled)"]
    C --> D["Parse .msg/.srv files"]
    D --> E["Resolve dependencies<br/>(type hashes, nested types)"]
    E --> F["Generate Python files"]
    E --> G["Generate Rust PyO3 module"]
    F --> H["hiroz-msgs/python/<br/>hiroz_msgs_py/types/*.py"]
    G --> I["$OUT_DIR/python_bindings.rs"]
    I --> J["include!() in lib.rs"]
    J --> K["Compiled into hiroz-msgs"]
```

### Output Locations

| Output | Path | Committed? |
|--------|------|------------|
| Python msgspec classes | `hiroz-msgs/python/hiroz_msgs_py/types/*.py` | Yes |
| Rust PyO3 bindings | `target/.../hiroz-msgs-.../out/python_bindings.rs` | No (generated) |

### Feature Flags

The `python_registry` feature gate controls Python codegen:


```rust
// lib.rs
#[cfg(feature = "python_registry")]
include!(concat!(env!("OUT_DIR"), "/python_bindings.rs"));
```

The `hiroz-derive` crate provides the derive macros, and `hiroz/python` enables the `FromPyMessage` and `IntoPyMessage` traits.
