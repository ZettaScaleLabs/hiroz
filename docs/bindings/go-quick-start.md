# Go Quick Start

Get a Go publisher and subscriber running in five minutes.

## Prerequisites

- Go 1.23+
- An Eclipse Zenoh router — see [Networking](../user-guide/networking.md)

No Rust toolchain required when using the pre-built library.

## 1. Get the library

### Option A — Pre-built library (recommended)

Download the static library and C header for your platform from the [Releases page](https://github.com/ZettaScaleLabs/hiroz/releases):

| Platform | Jazzy / Kilted / Rolling | Humble |
|---|---|---|
| Linux x86_64 | `libhiroz-jazzy-x86_64-unknown-linux-gnu.a` | `libhiroz-humble-x86_64-unknown-linux-gnu.a` |
| Linux aarch64 | `libhiroz-jazzy-aarch64-unknown-linux-gnu.a` | `libhiroz-humble-aarch64-unknown-linux-gnu.a` |
| macOS aarch64 | `libhiroz-jazzy-aarch64-apple-darwin.a` | `libhiroz-humble-aarch64-apple-darwin.a` |

Each release also includes `hiroz_ffi.h` (the C header required for CGO).

```bash
# Clone the repo for the Go package source — no Rust build needed
git clone https://github.com/ZettaScaleLabs/hiroz.git
cd hiroz

# Download the pre-built library and header — replace <version> and pick your platform file
curl -Lo crates/hiroz-go/libhiroz.a \
  https://github.com/ZettaScaleLabs/hiroz/releases/download/<version>/libhiroz-jazzy-x86_64-unknown-linux-gnu.a
curl -Lo crates/hiroz-go/hiroz/hiroz_ffi.h \
  https://github.com/ZettaScaleLabs/hiroz/releases/download/<version>/hiroz_ffi.h
```

### Option B — Build from source

Requires Rust 1.85+, `cbindgen`, and `just`:

```bash
git clone https://github.com/ZettaScaleLabs/hiroz.git
cd hiroz
just -f crates/hiroz-go/justfile quickstart
```

This generates message types, compiles `libhiroz.a`, and verifies both are present.

## 2. Write a publisher

Create `hello_pub/main.go`:

```go
package main

import (
    "fmt"
    "log"
    "time"

    "github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/hiroz"
    "github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/std_msgs"
)

func main() {
    ctx, err := hiroz.NewContext().WithDomainID(0).Build()
    if err != nil {
        log.Fatal(err)
    }
    defer ctx.Close()

    node, err := ctx.CreateNode("go_talker").Build()
    if err != nil {
        log.Fatal(err)
    }
    defer node.Close()

    pub, err := node.CreatePublisher("chatter").Build(&std_msgs.String{})
    if err != nil {
        log.Fatal(err)
    }
    defer pub.Close()

    for i := 0; ; i++ {
        msg := &std_msgs.String{Data: fmt.Sprintf("Hello #%d", i)}
        if err := pub.Publish(msg); err != nil {
            log.Printf("publish error: %v", err)
        }
        fmt.Printf("Published: %s\n", msg.Data)
        time.Sleep(500 * time.Millisecond)
    }
}
```

Create `hello_pub/go.mod` — the `replace` directive points Go to the local `hiroz` package:

```text
module hello_pub

go 1.23

require github.com/ZettaScaleLabs/hiroz/crates/hiroz-go v0.0.0

replace github.com/ZettaScaleLabs/hiroz/crates/hiroz-go => /path/to/hiroz/crates/hiroz-go
```

Replace `/path/to/hiroz` with the absolute path where you cloned the repo.

## 3. Write a subscriber

Create `hello_sub/main.go`:

```go
package main

import (
    "log"

    "github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/hiroz"
    "github.com/ZettaScaleLabs/hiroz/crates/hiroz-go/generated/std_msgs"
)

func main() {
    ctx, err := hiroz.NewContext().WithDomainID(0).Build()
    if err != nil {
        log.Fatal(err)
    }
    defer ctx.Close()

    node, err := ctx.CreateNode("go_listener").Build()
    if err != nil {
        log.Fatal(err)
    }
    defer node.Close()

    _, err = node.CreateSubscriber("chatter").
        BuildWithCallback(&std_msgs.String{}, func(data []byte) {
            msg := &std_msgs.String{}
            if err := msg.DeserializeCDR(data); err != nil {
                log.Printf("deserialize error: %v", err)
                return
            }
            log.Printf("Received: %s", msg.Data)
        })
    if err != nil {
        log.Fatal(err)
    }

    select {} // block forever
}
```

Create `hello_sub/go.mod` with the same `replace` directive as above.

## 4. Run

You need a Zenoh router running first — it acts as the rendezvous point for all nodes.

**Start the router** (pick one):

```bash
# Option A: download zenohd from https://github.com/eclipse-zenoh/zenoh/releases
./zenohd

# Option B: install via cargo (one-time)
cargo install zenohd && zenohd
```

**Run the subscriber and publisher** — set `CGO_LDFLAGS` to point at the library:

```bash
HIROZ=/path/to/hiroz

# Terminal 2: subscriber
cd hello_sub
CGO_LDFLAGS="-L$HIROZ/crates/hiroz-go -lhiroz -lm" \
CGO_CFLAGS="-I$HIROZ/crates/hiroz-go/hiroz" \
go run main.go

# Terminal 3: publisher
cd hello_pub
CGO_LDFLAGS="-L$HIROZ/crates/hiroz-go -lhiroz -lm" \
CGO_CFLAGS="-I$HIROZ/crates/hiroz-go/hiroz" \
go run main.go
```

You should see the subscriber printing messages from the publisher.

!!! tip
    Set `HIROZ` in your shell profile to avoid repeating the path.

## What's next

- **[Go Bindings](./go.md)** — full API reference: typed helpers, graph introspection, QoS, error handling
- **[Message Generation](../user-guide/message-generation.md)** — generate types from a full ROS 2 install
- **[ROS 2 Interoperability](../user-guide/interop.md)** — connect a hiroz subscriber to a live ROS 2 talker
