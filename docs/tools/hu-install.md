# Installing hu

`hu` ships as two separate things, and you need both:

- the **`hu` binary** — the plugin host, the TUI, `stream`, `router`, `web` and `plugin` management;
- the **plugins** — `hu_meter.wasm` and `hu_monitor.wasm`. `hu meter` and `hu monitor` are not built into the binary. Without the plugins those subcommands do not exist.

Everything here works with no ROS 2 install. `hu` only needs to reach a Zenoh router.

## Quickest path

Set `HU_RELEASE_BASE` to the release you are installing from, and pass the same version twice — once so `curl` finds the installer, once so the installer finds the assets:

<!-- repro: skip needs a published release and network access to it -->
```bash
BASE=https://github.com/ZettaScaleLabs/hiroz/releases/download/v0.1.0
curl -fsSL "$BASE/install-hu.sh" -o install-hu.sh
HU_RELEASE_BASE="$BASE" HU_VERSION=0.1.0 sh install-hu.sh
```

**Download the installer, then run it — do not pipe it into a shell.** Two failure modes look like success if you pipe. A wrong URL makes `curl -fsSL` fail silently, `sh` then reads empty input and exits 0, so you see nothing and no error. And a connection that drops mid-transfer still executes every complete line that arrived, which can leave `hu` installed with no plugins. Downloading first makes `curl`'s exit status stop the install, and gives `sh` a complete file.

That downloads the binary and the plugins, verifies both against `SHA256SUMS`, installs `hu` to `~/.local/bin/` and the plugins to `~/.local/share/hu/plugins/`.

`HU_RELEASE_BASE` is not optional here, and the reason is worth knowing: piping the script through `curl` sets nothing inside it. Without that variable the installer falls back to its own built-in host, so you would fetch the script from one place and its assets from another — and the download would fail against a host you may not even be able to reach.

**The base is the release's download directory, and its shape differs per channel.** GitHub publishes the whole workspace on `v<version>` tags, so the path ends `/releases/download/v<version>`. Releases cut on the `hu`-only `hu-v<version>` tags end `/releases/download/hu-v<version>` instead. Point `HU_RELEASE_BASE` at whichever one you were given; nothing below the base differs between them.

Set `--prefix` (or `HU_PREFIX`) to install somewhere other than `~/.local`. `hu` looks for plugins next to its own binary — under `<prefix>/share/hu/plugins` — as well as in `~/.local/share/hu/plugins`, so a prefixed install finds its own plugins.

Verify:

<!-- repro: timeout 10 -->
```bash
hu --version
hu plugin list
```

`hu plugin list` must show `meter` and `monitor`. If it is empty, the plugins did not install and every `hu meter` / `hu monitor` command will fail.

## Credentials

Whether you need a token depends on the release host, so the installer does not decide for you. It reads `$HU_RELEASE_TOKEN` from the environment and **never** carries one of its own. It never reads a credential from a file.

If it finds one it sends it with every download. If it finds none it proceeds without one — a public release host needs no credential, and refusing up front would block installing from, say, a public GitHub release that anyone can `curl`.

A missing credential therefore surfaces as a failed download, not as an early refusal, because that is the first point at which the host's requirement is actually known. The failure message names the variable to set. Nothing is ever installed from an error page: `curl --fail` makes an HTTP error an error, so a 401 or a 404 body is never written to disk and never unpacked.

If you have no account, use the offline path below — it needs no network and no credential.

## Offline install

Someone hands you the release files; you install from a directory:

<!-- repro: skip needs ./hu-release, a directory of release files the reader supplies -->
```bash
install-hu.sh --offline ./hu-release
```

The directory needs at least `SHA256SUMS` and the binary tarball for your platform. Include `hu-plugins-<ver>.tar.gz` to get `meter` and `monitor` too. Checksums are still enforced — a file with no entry in `SHA256SUMS` is refused, because an unlisted file is exactly what a substituted file looks like.

## Manual install

Verify the download first. `sha256sum -c` exits non-zero on a mismatch, so stop here if it does — do not extract a file that failed this check:

<!-- repro: skip needs the release tarballs and their SHA256SUMS already downloaded -->
```bash
sha256sum -c SHA256SUMS
```

Then extract and install:

<!-- repro: skip needs the release tarballs already downloaded and verified -->
```bash
tar -xzf hu-0.1.0-x86_64-unknown-linux-gnu.tar.gz
install -Dm755 hu ~/.local/bin/hu

mkdir -p ~/.local/share/hu/plugins
tar -xzf hu-plugins-0.1.0.tar.gz -C ~/.local/share/hu/plugins
```

## Installing plugins individually

`hu plugin install` accepts a local file, a URL, or a name from a release index:

<!-- repro: skip needs a downloaded .wasm, a reachable URL, or a configured registry -->
```bash
hu plugin install ./hu_meter-0.1.0.wasm
hu plugin install https://example.invalid/hu_meter-0.1.0.wasm
hu plugin install meter --registry "$BASE/hu-plugins-0.1.0.json"   # $BASE as above
```

In every case the file is checked before it is accepted:

- if a checksum is available — from the index, or from a `.sha256` sitting next to the file — it must match;
- the file must compile as a WASM component, checked in a temporary location so a broken plugin is never briefly discoverable;
- when installing by name, the index's WIT world must match the one this `hu` hosts, so a plugin built for a different `hu` is refused with a readable message instead of a link error later.

Remove one with:

<!-- repro: skip requires a plugin to be installed first -->
```bash
hu plugin uninstall meter
```

`hu plugin install` writes only to `~/.local/share/hu/plugins/`. Directories on `$HU_PLUGIN_PATH` are left alone — those point at build trees during development and are not the installer's to manage.

### What the checksums do and do not buy you

They protect against a corrupted or accidentally substituted download. They are **not** authenticity: nothing is signed, there is no registry that vouches for a publisher, and a plugin's declared permissions are self-reported by the plugin itself rather than enforced against it. Installing a plugin means trusting whoever wrote it.

## Development builds

To run plugins you are building from source, point `$HU_PLUGIN_PATH` at the build output instead of installing:

```bash
export HU_PLUGIN_PATH=$PWD/crates/hiroz-union/plugins/target/wasm32-wasip2/release
```

`$HU_PLUGIN_PATH` is searched before `~/.local/share/hu/plugins/`, so a development build shadows an installed one of the same name. `hu plugin list` shows the path each plugin was loaded from, plus a `SOURCE` column — `unmanaged` means it was not installed by `hu plugin install`.

## Uninstalling

<!-- repro: skip removes the installed hu the rest of this suite runs against -->
```bash
rm ~/.local/bin/hu
rm -rf ~/.local/share/hu
```

## Platform coverage

| Platform | Published |
|---|---|
| Linux x86_64 | yes |
| Linux aarch64 | yes, when built |
| macOS aarch64 | yes, when built |
| macOS x86_64 | no — build from source |
| Windows | no |

The plugins are `wasm32-wasip2` and platform-independent: one `.wasm` runs everywhere `hu` does.

A release covers only the platforms its build legs produced. If a tarball is missing from a release, it was not produced for that version.
