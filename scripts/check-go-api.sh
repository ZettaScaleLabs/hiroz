#!/usr/bin/env bash
# Build matching native/Go APIs, generate example types, and check public entry points.
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo_root/target}"
if [[ "$CARGO_TARGET_DIR" != /* ]]; then
    export CARGO_TARGET_DIR="$repo_root/$CARGO_TARGET_DIR"
fi
mkdir -p "$repo_root/_tmp"

cargo build -p hiroz --features ffi
# Keep a matching library while other worktrees use the same Cargo build cache.
native_test_lib=$(mktemp -d "$CARGO_TARGET_DIR/go-api-check.XXXXXX")
trap 'rm -rf "$native_test_lib"' EXIT
cp "$CARGO_TARGET_DIR/debug/libhiroz.a" "$native_test_lib/libhiroz.a"
cargo test --locked -p hiroz --features ffi \
    --test ffi_subscriber_lifetime --test dynamic_bounded_schema --test ffi_service_timeout --test volatile_subscriber_order \
    --test ffi_action_lifetime --test action_client_lifetime --test endpoint_lifetime \
    --test service_readiness --test concurrent_schema_discovery \
    --test schema_discovery_malformed_reply --test graph \
    --test concurrent_transport_declarations
cargo run -p hiroz-codegen --bin export_json -- \
    --assets crates/hiroz-codegen/assets/jazzy \
    --output "$repo_root/_tmp/go-api-manifest.json" \
    --packages builtin_interfaces,service_msgs,unique_identifier_msgs,action_msgs,std_msgs,geometry_msgs,example_interfaces

(
    cd crates/hiroz-codegen-go
    go test ./...
    go run . -input "$repo_root/_tmp/go-api-manifest.json" \
        -output "$repo_root/crates/hiroz-go/generated" \
        -prefix github.com/ZettaScaleLabs/hiroz/crates/hiroz-go
)

export CGO_LDFLAGS="-L$native_test_lib ${CGO_LDFLAGS:-}"
(
    cd crates/hiroz-go
    go test -race -tags generated_api ./... ./testdata
    go vet -tags generated_api ./... ./testdata
    # Compile ROS interop tests without requiring a running router or ROS install.
    go test -c -tags integration -o "$repo_root/_tmp/go-interop.test" ./interop_tests
)
(
    cd crates/hiroz-go/examples/production_service
    go test ./...
    go vet ./...
)

echo "Go API checks passed. ROS 2 interop requires the separate integration environment."
