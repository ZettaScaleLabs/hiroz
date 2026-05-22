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
          "rolling" # continuous release, no EOL
        ];

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

            # Test environment with test messages (for all tests)
            testFull = pkgs.rosPackages.${rosDistro}.buildEnv {
              paths = rosDeps.rcl ++ rosDeps.messages ++ rosDeps.testMessages;
              wrapPrograms = false;
            };
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
              pythonVersion = pythonVer;
              rosDistro = rosDistro;
              extraShellHook = '''';
            };
          };

        # Generate shells for all distros
        allDistroShells = builtins.listToAttrs (
          builtins.map (distro: {
            name = distro;
            value = mkRosShells distro;
          }) distros
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

          # CI without ROS
          pureRust-ci = mkDevShell {
            name = "hiroz-ci-pure-rust";
            packages = commonBuildInputs ++ pythonTools ++ docTools ++ testTools;
            extraShellHook = '''';
          };

          # Bridge interop: Jazzy build + test environment with Humble tools accessible via
          # the `humble-ros2` wrapper script (env var HUMBLE_ROS2).
          # Tests call binaries directly — no `nix develop` subprocess invocations.
          ros-bridge-interop =
            let
              humbleEnv = mkRosEnv "humble";
              jazzyEnv = mkRosEnv "jazzy";
              pythonVer = pkgs.python3;
              # Wrapper script: invokes ros2 inside the Humble environment.
              # Overrides AMENT_PREFIX_PATH / ROS_DISTRO so Humble packages take precedence
              # over anything inherited from the outer Jazzy shell.
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
              # Jazzy dev env gives us: build deps + ros2cli + rmw_zenoh_cpp for `ros2 topic list`
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
              # HUMBLE_ROS2 is set as a mkShell attribute so it is exported by
              # `nix print-dev-env` (unlike shellHook which is not captured).
              extraEnvVars = {
                HUMBLE_ROS2 = "${humbleRos2}/bin/humble-ros2";
              };
            };
        }
        # Add per-distro dev shells (ros-jazzy, ros-rolling, ...)
        // (builtins.listToAttrs (
          builtins.map (distro: {
            name = "ros-${distro}";
            value = allDistroShells.${distro}.default;
          }) distros
        ))
        # Add per-distro CI shells (ros-jazzy-ci, ros-rolling-ci, ...)
        // (builtins.listToAttrs (
          builtins.map (distro: {
            name = "ros-${distro}-ci";
            value = allDistroShells.${distro}.ci;
          }) distros
        ));

        formatter = pkgs.nixfmt-rfc-style;
      }
    );

  nixConfig = {
    extra-substituters = [ "https://ros.cachix.org" ];
    extra-trusted-public-keys = [ "ros.cachix.org-1:dSyZxI8geDCJrwgvCOHDoAfOm5sV1wCPjBkKL+38Rvo=" ];
  };
}
