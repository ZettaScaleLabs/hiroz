# Troubleshooting

**Quick solutions to common hiroz build and runtime issues.** Click on any question to expand the answer.

!!! tip
    Most issues fall into three categories: build configuration, runtime connectivity, or ROS 2 integration.

## Build Issues

??? question "Build fails with 'package not found' or missing ROS 2 packages"
    **Root Cause:** ROS 2 environment not sourced or packages not installed.

    **Solutions:**

    1. **Source ROS 2 environment:**
       ```bash
       source /opt/ros/jazzy/setup.bash
       # or for rolling:
       source /opt/ros/rolling/setup.bash
       ```

    2. **Verify environment variables:**
       ```bash
       echo $AMENT_PREFIX_PATH
       echo $CMAKE_PREFIX_PATH
       ```

    3. **Check package installation:**
       ```bash
       ros2 pkg prefix example_interfaces
       # If fails, install:
       sudo apt install ros-jazzy-example-interfaces
       ```

    4. **Clean and rebuild:**
       ```bash
       cargo clean -p hiroz-msgs
       cargo build -p hiroz-msgs
       ```

    **Common Error Messages:**

    | Error | Solution |
    |-------|----------|
    | "Package X not found" | Source ROS 2 environment |
    | "Cannot find ament_index" | Install ROS 2 or use bundled msgs |
    | "AMENT_PREFIX_PATH not set" | Run `source /opt/ros/jazzy/setup.bash` |

??? question "Compiler error: cannot find crate hiroz_msgs"
    **Root Cause:** `hiroz-msgs` is not part of default workspace members.

    **Solution:**

    ```bash
    # Build hiroz-msgs explicitly
    cargo build -p hiroz-msgs

    # Then build your example
    cargo build --example z_srvcli
    ```

    **Note:** `hiroz-msgs` is excluded from default builds to avoid requiring ROS 2 for core development. Build it explicitly when needed.

??? question "Build takes too long to complete"
    **Solutions:**

    ```bash
    # Use parallel builds (automatic on most systems)
    cargo build -j $(nproc)

    # Build only what you need
    cargo build -p hiroz-msgs --features std_msgs,geometry_msgs
    ```

??? question "Linker errors during build (especially with rcl-z)"
    **Solution:**

    ```bash
    # Clear cache and rebuild
    cargo clean
    source /opt/ros/jazzy/setup.bash
    cargo build -p rcl-z
    ```

    **Warning:** After changing feature flags or updating ROS 2, run `cargo clean -p hiroz-msgs` to force message regeneration.

## Runtime Issues

??? question "Publishers and subscribers on different processes don't communicate"
    **Root Cause:** Eclipse Zenoh router not running or nodes not configured correctly.

    **Solution:**

    1. **Ensure the router is running:**
       ```bash
       zenohd   # pre-built binary from https://github.com/eclipse-zenoh/zenoh/releases
       # or: cargo install zenohd && zenohd
       ```

    2. **Verify your code connects to the router:**
       ```rust
       let ctx = ZContextBuilder::default()
           .with_connect_endpoints(["tcp/127.0.0.1:7447"])  // must match router address
           .build()?;
       ```

??? question "Router fails to start with 'address already in use'"
    **Root Cause:** Another process is using port 7447.

    **Solutions:**

    1. **Stop the conflicting process** — find it with `lsof -i :7447` (Linux/macOS) or `netstat -ano | findstr 7447` (Windows), then kill it.

    2. **Use a custom port:**

       Start the router on a different port:
       ```bash
       zenohd --listen "tcp/[::]:7448"
       ```

       Then connect your code to that port:
       ```rust
       let ctx = ZContextBuilder::default()
           .with_connect_endpoints(["tcp/127.0.0.1:7448"])
           .build()?;
       ```

??? question "Can I skip the router and use peer-to-peer mode?"
    **No** — hiroz requires a Zenoh router for reliable operation and ROS 2 interoperability. Peer-to-peer mode is not supported.

    If you don't want to install a router locally, the easiest alternative is Docker:
    ```bash
    docker run --init --net host eclipse/zenoh:latest
    ```

    See [Networking](../user-guide/networking.md) for all router options, including apt/brew install and pre-built binaries.

??? question "Multi-segment topics like /robot/sensors/camera don't work"
    **Symptom:** Publisher publishes to `/robot/sensors/camera` but subscriber never receives messages.

    **Root Cause:** Old versions of hiroz (before 0.1.0) incorrectly mangled slashes in topic key expressions, breaking multi-segment topic routing.

    **Solution:** Update to hiroz 0.1.0+ which correctly preserves internal slashes in topic key expressions.

    **Verify the fix:**

    ```bash
    # Enable debug logging to see key expressions
    RUST_LOG=hiroz=debug cargo run --example z_pubsub
    ```

    Look for key expressions in the output:

    | Key Expression | Status |
    |---------------|--------|
    | `0/robot/sensors/camera/...` | ✅ Correct (slashes preserved) |
    | `0/robot%sensors%camera/...` | ❌ Wrong (slashes mangled) |

    **Technical Details:**

    - **Topic key expressions** should use `strip_slashes()`: removes leading/trailing slashes, preserves internal slashes
    - **Liveliness tokens** should use `mangle_name()`: replaces all `/` with `%`
    - This matches the behavior of [`rmw_zenoh_cpp`](https://github.com/ros2/rmw_zenoh)

    See [Key Expression Formats](../experimental/keyexpr-formats.md#key-expression-behavior-important) for details.

??? question "ROS 2 nodes don't receive messages from hiroz (type hash mismatch)"
    **Root Cause:** Type hash mismatch — hiroz and the ROS 2 node disagree on the message definition's RIHS01 hash.

    **Diagnosis:**

    Enable debug logging to see the key expressions being advertised:

    ```bash
    RUST_LOG=hiroz=debug cargo run --example z_pubsub
    ```

    Look for the hash in the key expression output:

    ```
    [PUB] Key expression: 0/chatter/std_msgs::msg::dds_::String_/RIHS01_<hash>
    ```

    Compare the `RIHS01_<hash>` with what the ROS 2 node advertises (visible with `ZENOH_LOGS=debug` on the ROS 2 side). If the hashes differ, the nodes will not exchange messages.

    **Common causes:**

    | Cause | Fix |
    |-------|-----|
    | Wrong package used (e.g. `action_tutorials_interfaces` vs `example_interfaces`) | Use the exact package the ROS 2 node uses — check with `ros2 interface show` |
    | Message definition mismatch (extra/missing field) | Verify field names match: `ros2 interface show <pkg>/msg/<Msg>` |
    | hiroz-msgs generated from stale `.msg` files | `cargo clean -p hiroz-msgs && cargo build -p hiroz-msgs` |

    **Verify the correct type:**

    ```bash
    ros2 interface show std_msgs/msg/String
    ```

    Then check that `hiroz-codegen/assets/jazzy/std_msgs/msg/String.msg` matches exactly.

## Resources

- **[Building Guide](../getting-started/building.md)** - Correct build procedures
- **[Networking](../user-guide/networking.md)** - Zenoh router setup
- **[Feature Flags](./feature-flags.md)** - Available features
- **[GitHub Issues](https://github.com/ZettaScaleLabs/hiroz/issues)** - Report bugs

**Most issues are environmental. Verify your setup matches the build scenario requirements before diving deeper.**
