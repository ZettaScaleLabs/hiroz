{
  description = "hiroz: Native Rust ROS 2 implementation using Zenoh";

  inputs = {
    nix-ros-overlay.url = "github:lopsided98/nix-ros-overlay";
    nixpkgs.follows = "nix-ros-overlay/nixpkgs";
    rust-overlay.url = "github:oxalica/rust-overlay";
    git-hooks.url = "github:cachix/git-hooks.nix";
    systems.url = "github:nix-systems/default";
  };

  outputs =
    {
      self,
      nix-ros-overlay,
      nixpkgs,
      rust-overlay,
      git-hooks,
      systems,
    }:
    nix-ros-overlay.inputs.flake-utils.lib.eachDefaultSystem (
      system:
      let
        # List of supported ROS distros (via nix-ros-overlay)
        # Note: Iron (May 2023 - Nov 2024, EOL) is not available in nix-ros-overlay
        # but can be used if installed manually
        distros = [
          "jazzy" # (May 2024 - May 2029, LTS) <-- Default
          "humble" # (May 2022 - May 2027, LTS)
          "kilted" # (May 2025 - Nov 2026)
          "lyrical" # (May 2026 - May 2031, LTS)
          "rolling" # continuous release, no EOL
        ];
        # Only include distros present in the pinned nix-ros-overlay; newer distros
        # (lyrical, rolling) may not be cached on the CI worker yet.
        availableDistros = builtins.filter (d: pkgs.rosPackages ? ${d}) distros;

        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            nix-ros-overlay.overlays.default
            rust-overlay.overlays.default
          ];
        };

        rustToolchain = pkgs.rust-bin.stable."1.91.0".default.override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "llvm-tools-preview"
          ];
        };

        # CI-only toolchain — adds the wasm32-wasip2 sysroot for building WASM
        # plugins. Kept separate so everyday `nix develop` shells don't pay the
        # WASM sysroot download cost (~100-500 MB).
        rustToolchainWasm = rustToolchain.override {
          targets = [ "wasm32-wasip2" ];
        };

        rustfmtNightly = pkgs.rust-bin.nightly.latest.rustfmt;

        # Override rustfmt to use nightly
        rustfmt-nightly-bin = pkgs.writeShellScriptBin "rustfmt" ''
          exec ${rustfmtNightly}/bin/rustfmt "$@"
        '';

        # Factory to create environment for a specific ROS distro
        mkRosEnv =
          rosDistro:
          let
            rosDeps = {
              rcl = with pkgs.rosPackages.${rosDistro}; [
                rcl
                rcl-interfaces
                rclcpp
                rcutils
                demo-nodes-py
                demo-nodes-cpp
                action-tutorials-cpp
              ];

              messages = with pkgs.rosPackages.${rosDistro}; [
                std-msgs
                geometry-msgs
                sensor-msgs
                example-interfaces
                common-interfaces
                rosidl-default-generators
                rosidl-default-runtime
                rosidl-adapter
                rosidl-typesupport-fastrtps-c
                rosidl-typesupport-fastrtps-cpp
              ];

              # Test-only message packages
              testMessages = with pkgs.rosPackages.${rosDistro}; [
                test-msgs
              ];

              # CLI tools needed for interop tests (ros2 topic/param/node/service)
              testCli = with pkgs.rosPackages.${rosDistro}; [
                ros2cli
                ros2topic
                ros2node
                ros2param
                ros2service
                ros2action
                ros2pkg
                rmw-zenoh-cpp
              ];

              devExtras = with pkgs.rosPackages.${rosDistro}; [
                ament-cmake-core
                ros-core
                rclpy
                rmw
                rmw-implementation
                rmw-zenoh-cpp
                rmw-cyclonedds-cpp
                ament-cmake
                ament-cmake-gtest
                ament-lint-auto
                ament-lint-common
                launch
                launch-testing
                ros2cli
                osrf-testing-tools-cpp
                mimick-vendor
                performance-test-fixture
                python-cmake-module
              ];
            };
          in
          {
            # Development environment with all dependencies including test messages
            # KEY CHANGE: Disable wrappers to prevent Store paths from being forced to the front
            dev = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.rcl ++ rosDeps.messages ++ rosDeps.testMessages ++ rosDeps.devExtras;
              wrapPrograms = false;
            };

            # Core RCL only
            rcl = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.rcl;
              wrapPrograms = false;
            };

            # Runtime messages only
            msgs = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.messages;
              wrapPrograms = false;
            };

            # Build environment with runtime messages but NO test messages
            build = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.rcl ++ rosDeps.messages;
              wrapPrograms = false;
            };

            # Test environment for core tests only (no test_msgs)
            testCore = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.rcl ++ rosDeps.messages;
              wrapPrograms = false;
            };

            # Test environment with test messages and CLI tools (for all tests)
            testFull = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.rcl ++ rosDeps.messages ++ rosDeps.testMessages ++ rosDeps.testCli;
              wrapPrograms = false;
            };

            # Individual store paths for testFull — used to build a correct AMENT_PREFIX_PATH
            # that preserves each package's own ament index (buildEnv merging drops index entries).
            testFullPaths = rosDeps.rcl ++ rosDeps.messages ++ rosDeps.testMessages ++ rosDeps.testCli;
          };

        # Colcon configuration
        colconDefaults = pkgs.writeText "colcon-defaults.json" (
          builtins.toJSON {
            build = {
              parallel-workers = 4;
              symlink-install = true;
              cmake-args = [
                "-DCMAKE_BUILD_TYPE=RelWithDebInfo"
                "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON"
              ];
            };
            test = {
              parallel-workers = 1;
              event-handlers = [
                "console_cohesion+"
                "console_direct+"
              ];
            };
          }
        );

        # Common build tools
        commonBuildInputs = with pkgs; [
          rustToolchain
          sccache
          clang
          llvmPackages.libclang
          llvmPackages.bintools
          pkg-config
          nushell
          protobuf
          markdownlint-cli
          colcon
          just # Task runner (replaces Makefile)
          # Ensure python is available since we unwrapped the ROS env
          python3
          go # Go toolchain (latest stable)
        ];

        # Development tools
        devTools = with pkgs; [
          cargo-edit
          cargo-watch
          clang-tools
          rust-analyzer
          nixfmt-rfc-style
          gdb
          gopls # Go language server
          gotools # Go tools (goimports, etc.)
          delve # Go debugger
        ];

        # Python tools (hiroz-py bindings)
        pythonTools = with pkgs; [
          maturin
          uv
          python3
          ruff
          python3Packages.mypy
          python3Packages.pytest
          python3Packages.pytest-cov
          python3Packages.build
          python3Packages.pip
        ];

        # Documentation tools
        docTools = with pkgs; [
          vale
          (python3.withPackages (
            ps: with ps; [
              mkdocs
              mkdocs-material
              mkdocs-material-extensions
              pymdown-extensions
            ]
          ))
          git-cliff # conventional-commit changelog generation
        ];

        # Test tools
        testTools = with pkgs; [
          cargo-nextest
        ];

        # Environment variables for Rust/C++ interop
        commonEnvVars = rec {
          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          CLANG_PATH = "${pkgs.llvmPackages.clang}/bin/clang";
          RUST_BACKTRACE = "1";
          RMW_IMPLEMENTATION = "rmw_zenoh_cpp";
          RUSTC_WRAPPER = "${pkgs.sccache}/bin/sccache";
          COLCON_DEFAULTS_FILE = "${colconDefaults}";
          CARGO_BUILD_JOBS = "4";
          MAKEFLAGS = "-j4";
        };

        # Export environment variables as shell commands
        exportEnvVars = pkgs.lib.concatStringsSep "\n" (
          pkgs.lib.mapAttrsToList (name: value: "export ${name}=\"${value}\"") commonEnvVars
        );

        # Base shell configuration factory
        mkDevShell =
          {
            name,
            packages,
            banner ? "",
            extraShellHook ? "",
            # Extra env vars set as mkShell attributes (exported by `nix print-dev-env`).
            extraEnvVars ? { },
            rosEnvPath ? null,
            # Individual package store paths — when provided, each is added to AMENT_PREFIX_PATH
            # separately so their ament indexes survive (buildEnv merging drops index entries).
            rosEnvPaths ? [ ],
            pythonVersion ? pkgs.python3, # To determine site-packages path
            rosDistro ? null,
          }:
          pkgs.mkShell (
            {
              inherit name packages;

              # KEY CHANGE: Manually construct the environment using SUFFIX logic
              # rosEnvPath is the Nix Store path. We append it to existing vars.
              shellHook = ''
                ${exportEnvVars}

                ${
                  if rosEnvPath != null then
                    ''
                      # --suffix logic: Add Nix Store paths to the END of the lists.
                      # This ensures your workspace (which you source via setup.bash) stays at the front.

                      export LD_LIBRARY_PATH="$LD_LIBRARY_PATH:${rosEnvPath}/lib"
                      export PYTHONPATH="$PYTHONPATH:${rosEnvPath}/lib/${pythonVersion.libPrefix}/site-packages"
                      export CMAKE_PREFIX_PATH="$CMAKE_PREFIX_PATH:${rosEnvPath}"
                      export AMENT_PREFIX_PATH="$AMENT_PREFIX_PATH:${rosEnvPath}"
                      export ROS_PACKAGE_PATH="$ROS_PACKAGE_PATH:${rosEnvPath}/share"
                      export GZ_CONFIG_PATH="$GZ_CONFIG_PATH:${rosEnvPath}/share/gz"

                      # These are usually static, so simple export is fine
                      ${if rosDistro != null then "export ROS_DISTRO=${rosDistro}" else ""}
                      export ROS_VERSION=2
                      export ROS_PYTHON_VERSION=3
                    ''
                  else
                    ""
                }

                ${
                  # Per-package AMENT_PREFIX_PATH: collapsed into one export so nix develop
                  # doesn't evaluate 26 separate shell statements (each increments SHLVL).
                  pkgs.lib.optionalString (rosEnvPaths != [ ]) ''
                    export AMENT_PREFIX_PATH="$AMENT_PREFIX_PATH:${pkgs.lib.concatStringsSep ":" rosEnvPaths}"
                  ''
                }

                ${extraShellHook}
                ${if banner != "" then banner else ""}
              '';
              hardeningDisable = [ "all" ];
            }
            // extraEnvVars
          );

        # Helper to create shells for a specific ROS distro
        mkRosShells =
          rosDistro:
          let
            rosEnv = mkRosEnv rosDistro;
            # Capture the python version used by this distro to get correct site-packages
            # (Assuming standard python3 for now, but safer to pull from rosPackages if it varies)
            pythonVer = pkgs.python3;
          in
          {
            default = mkDevShell {
              name = "hiroz-dev-${rosDistro}";
              packages = [
                rustfmt-nightly-bin
              ]
              ++ commonBuildInputs
              ++ devTools
              ++ pythonTools
              ++ docTools
              ++ testTools
              ++ [ rosEnv.dev ]
              ++ pre-commit-check.enabledPackages;
              rosEnvPath = rosEnv.dev;
              pythonVersion = pythonVer;
              rosDistro = rosDistro;
              extraShellHook = ''
                ${pre-commit-check.shellHook}
              '';
              banner = ''
                echo "🦀 hiroz development environment (with ROS)"
                echo "ROS 2 Distribution: ${rosDistro}"
                echo "Rust: $(rustc --version)"
                echo "⚠️  Note: Nix Store paths are appended. Source your workspace setup.bash to overlay."
              '';
            };

            ci = mkDevShell {
              name = "hiroz-ci-${rosDistro}";
              packages = commonBuildInputs ++ pythonTools ++ docTools ++ testTools ++ [ rosEnv.testFull ];
              rosEnvPath = rosEnv.testFull;
              rosEnvPaths = rosEnv.testFullPaths;
              pythonVersion = pythonVer;
              rosDistro = rosDistro;
              extraShellHook = '''';
            };
          };

        # Generate shells for available distros only
        allDistroShells = builtins.listToAttrs (
          builtins.map (distro: {
            name = distro;
            value = mkRosShells distro;
          }) availableDistros
        );
        # Pre-commit hooks configuration
        mkdocsPkg = builtins.elemAt docTools 1;

        pre-commit-check = import ./nix/pre-commit.nix {
          inherit
            pkgs
            git-hooks
            system
            rustfmtNightly
            rustToolchain
            docTools
            mkdocsPkg
            ;
        };
      in
      {
        # Pre-commit checks
        checks = {
          inherit pre-commit-check;
        };

        # Development shells
        devShells = {
          # Default: first distro in the list with ROS
          default = allDistroShells.${builtins.head distros}.default;

          # Without ROS
          pureRust = mkDevShell {
            name = "hiroz-pure-rust";
            packages = [
              rustfmt-nightly-bin
            ]
            ++ commonBuildInputs
            ++ devTools
            ++ pythonTools
            ++ docTools
            ++ testTools
            ++ pre-commit-check.enabledPackages;
            extraShellHook = ''
              ${pre-commit-check.shellHook}
            '';
            banner = ''
              echo "🦀 hiroz development environment (pure Rust)"
              echo "Rust: $(rustc --version)"
            '';
          };

          # CI without ROS — includes wasm32-wasip2 sysroot for WASM plugin builds.
          # Uses rustToolchainWasm so the WASM target is only fetched in CI, not
          # in the default developer shell.
          pureRust-ci = mkDevShell {
            name = "hiroz-ci-pure-rust";
            packages =
              # Replace the default rustToolchain with the WASM-capable variant.
              [ rustToolchainWasm ]
              ++ (builtins.filter (p: p != rustToolchain) commonBuildInputs)
              ++ pythonTools
              ++ docTools
              ++ testTools;
            extraShellHook = '''';
          };

          # Bridge interop test environment (Jazzy + Humble side-by-side).
          # Used by `cargo test -p hiroz-tests --features bridge-interop-tests,jazzy`.
          ros-bridge-interop =
            let
              humbleEnv = mkRosEnv "humble";
              jazzyEnv = mkRosEnv "jazzy";
              pythonVer = pkgs.python3;
              humbleRos2 = pkgs.writeShellScriptBin "humble-ros2" ''
                export AMENT_PREFIX_PATH="${humbleEnv.dev}"
                export ROS_PACKAGE_PATH="${humbleEnv.dev}/share"
                export PYTHONPATH="${humbleEnv.dev}/lib/${pythonVer.libPrefix}/site-packages"
                export LD_LIBRARY_PATH="${humbleEnv.dev}/lib"
                export ROS_DISTRO="humble"
                export ROS_VERSION="2"
                export ROS_PYTHON_VERSION="3"
                exec "${humbleEnv.dev}/bin/ros2" "$@"
              '';
            in
            mkDevShell {
              name = "ros-bridge-interop";
              packages =
                commonBuildInputs
                ++ testTools
                ++ [
                  jazzyEnv.dev
                  humbleRos2
                ];
              rosEnvPath = jazzyEnv.dev;
              pythonVersion = pythonVer;
              rosDistro = "jazzy";
              extraEnvVars = {
                HUMBLE_ROS2 = "${humbleRos2}/bin/humble-ros2";
              };
            };

          # CI shell for bridge interop + hz-comparison tests.
          # Same as ros-bridge-interop but with:
          # - rustToolchain replaced by rustToolchainWasm (adds wasm32-wasip2 target)
          # - pre-built `hu` binary injected
          # The benchmark command must build hu-meter.wasm and set HU_PLUGIN_PATH itself
          # (nix develop --command does not run shellHook).
          bridge-interop-ci = (self.devShells.${system}.ros-bridge-interop).overrideAttrs (old: {
            buildInputs = [
              rustToolchainWasm
            ]
            ++ (builtins.filter (p: p != rustToolchain) (old.buildInputs or [ ]))
            ++ [ self.packages.${system}.hu ];
          });

        }
        # Add per-distro dev shells (ros-jazzy, ros-rolling, ...)
        // (builtins.listToAttrs (
          builtins.map (distro: {
            name = "ros-${distro}";
            value = allDistroShells.${distro}.default;
          }) availableDistros
        ))
        # Add per-distro CI shells (ros-jazzy-ci, ros-rolling-ci, ...)
        // (builtins.listToAttrs (
          builtins.map (distro: {
            name = "ros-${distro}-ci";
            value = allDistroShells.${distro}.ci;
          }) availableDistros
        ));

        packages = rec {
          hu = pkgs.rustPlatform.buildRustPackage {
            pname = "hu";
            version = "0.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.protobuf
            ];
            cargoBuildFlags = [
              "-p"
              "hiroz-union"
            ];
            cargoInstallFlags = [
              "--bin"
              "hu"
            ];
            doCheck = false;
            RUSTFLAGS = "";
          };
          default = hu;
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );

  nixConfig = {
    extra-substituters = [
      "https://ros.cachix.org"
      "https://hiroz.cachix.org"
    ];
    extra-trusted-public-keys = [
      "ros.cachix.org-1:dSyZxI8geDCJrwgvCOHDoAfOm5sV1wCPjBkKL+38Rvo="
      "hiroz.cachix.org-1:wKJuqEckTG0DL3Df7Ly9OVsg5S5TGBHtvlPGs+vlqrY="
    ];
  };
}
