{
  description = "Orb Core";

  inputs = {
    utils.url = "github:numtide/flake-utils";
    nixpkgs.url = "nixpkgs/nixos-23.05";
    nixpkgs-old = {
      url = "nixpkgs/nixos-20.09";
      flake = false;
    };
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };

	# FOSS:
    #  iris = {
    #    url = "github:worldcoin/iris/v1.6.1";
    #    flake = false;
    #  };
    # rgb-net = {
    #   url = "github:worldcoin/rgb-net/v2.0.2";
    #   flake = false;
    # };
    seekSdk = {
      url = "github:worldcoin/seek-thermal-sdk";
      flake = false;
    };
  };

  outputs = { self, utils, nixpkgs, nixpkgs-old, fenix, /* iris, rgb-net, */ seekSdk }:
    utils.lib.eachDefaultSystem (system:
      let
        flavours = {
          prod = [ ];
          stage = [ "stage" ];
        };

        rustChannel = {
          channel = "1.75";
          # To find the following hash for future versions, just put an empty
          # string and let Nix fail.
          sha256 = "SXRtAuO4IqNOQq+nLbrsDFbVk+3aVA8NNpSZsKlVH/8=";
        };
        rustFmtChannel = {
          channel = "nightly";
          date = "2023-12-28";
          sha256 = "4HfRQx49hyuJ8IjBWSty3OXCLOmeaZF5qZAXW6QiQNI=";
        };

        # Regular native environment.
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ opencvOverlay ];
        };

        opencvOverlay = self: super: {
          opencv4 = (super.callPackage (import ./nix/opencv.nix) { });
        };
        oldPkgs = import nixpkgs-old {
          inherit system;
          overlays = [ opencvOverlay ];
        };
        # Regular aarch64 environment.
        oldPkgsArm = import nixpkgs-old { system = "aarch64-linux"; };
        # Cross-compilation environment.
        oldPkgsCross = if system == "aarch64-linux" then oldPkgs else
        import nixpkgs-old {
          inherit system;
          crossSystem.config = "aarch64-unknown-linux-gnu";
          overlays = [
            (self: super: {
              # A trick to fetch dependencies from binary cache of regular
              # aarch64 packages. This reduces amount of dependencies cross-
              # compiling.
              inherit (oldPkgsArm)
                alsaLib
                gst_all_1
                zeromq
                ;
            })
            opencvOverlay
          ];
        };

        # Build dependencies. Can be cross-compiled or native (for tests).
        mkBuildDeps = pkgs: with pkgs; [
          alsaLib.dev
          gst_all_1.gst-plugins-base.dev
          gst_all_1.gstreamer.dev
          opencv4
          openssl
          zeromq
        ];
        # Common host dependencies.
        hostDeps = [
          # Nix requires loaders to be wrapped so Nix can edit the ELF RPATH.
          # Unfortunately the pkgs.lld package is the unwrapped version while
          # the wrapped one is described here:
          # https://github.com/NixOS/nixpkgs/issues/24744#issuecomment-488169652
          # and here:
          # https://matklad.github.io/2022/03/14/rpath-or-why-lld-doesnt-work-on-nixos.html
          # Long story short, use the .bintools package rather the plain .lld.
          # Also make sure if you need the .lldClang prefix as the plain
          # .bintools is also unwrapped.
          oldPkgs.llvmPackages_12.lldClang.bintools
          pkgs.fd
          pkgs.grcov
          pkgs.lcov
          pkgs.patchelf
          pkgs.pkg-config
          pkgs.protobuf
          pkgs.shellcheck
        ];
        # Host dependencies when compiling natively.
        nativeHostDeps = [
          # (pkgs.callPackage (import ./nix/python.nix) { inherit iris rgb-net; })
          pkgs.clang
        ];
        # Host dependencies when cross-compiling.
        crossHostDeps = if system == "aarch64-linux" then [ oldPkgs.clang ] else [
          oldPkgs.python38
          pkgs.stdenv.cc
          pkgs.clang
        ];

        mkNativeEnv = pkgs:
          let targetPlatform = pkgs.stdenv.targetPlatform.config; in {
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            EXTRA_CLANG_CFLAGS = with pkgs.stdenv.cc;
              builtins.toString ([ "-nostdinc" ] ++ builtins.map (path: "-isystem ${path}") [
                "${pkgs.clang}/resource-root/include"
                "${cc}/include/c++/${cc.version}"
                "${cc}/include/c++/${cc.version}/${targetPlatform}"
                "${cc}/lib/gcc/${targetPlatform}/${cc.version}/include"
                "${cc}/lib/gcc/${targetPlatform}/${cc.version}/include-fixed"
                "${pkgs.glibc.dev}/include"
              ]);
            CARGO_BUILD_TARGET = targetPlatform;
            PYO3_CROSS_LIB_DIR = "${pkgs.python38}/lib";
            HOST_TRIPLE = pkgs.stdenv.buildPlatform.config;
          };
        # Environment variables when compiling natively.
        nativeEnv = mkNativeEnv pkgs;
        # Environment variables when cross-compiling.
        crossEnv = if system == "aarch64-linux" then mkNativeEnv oldPkgs else
        let targetPlatform = oldPkgsCross.stdenv.targetPlatform.config; in {
          LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
          EXTRA_CLANG_CFLAGS = with oldPkgsCross.stdenv.cc;
            builtins.toString ([ "-nostdinc" ] ++ builtins.map (path: "-isystem ${path}") [
              "${pkgs.clang}/resource-root/include"
              "${cc}/${targetPlatform}/include/c++/${cc.version}"
              "${cc}/${targetPlatform}/include/c++/${cc.version}/${targetPlatform}"
              "${cc}/lib/gcc/${targetPlatform}/${cc.version}/include"
              "${cc}/lib/gcc/${targetPlatform}/${cc.version}/include-fixed"
              "${cc}/${targetPlatform}/sys-include"
              "${oldPkgsCross.glibcCross.dev}/include"
            ]);
          CARGO_BUILD_TARGET = targetPlatform;
          CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = "${oldPkgsCross.stdenv.cc}/bin/${targetPlatform}-gcc";
          PYO3_CROSS_LIB_DIR = "${oldPkgsArm.python38}/lib";
          HOST_TRIPLE = oldPkgsCross.stdenv.buildPlatform.config;
        };
        # Common environment variables.
        env = {
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          PYO3_PYTHON = "${pkgs.python38}/bin/python";
          SEEK_SDK_PATH = "${seekSdk}/Seek_Thermal_SDK_4.1.0.0";
        };

        # Development tools.
        rustToolchain = with fenix.packages.${system}; combine
          ((with toolchainOf rustChannel; [
            cargo
            clippy
            llvm-tools-preview
            rust-src
            rustc
          ]) ++ (with targets.${oldPkgsCross.stdenv.targetPlatform.config}.toolchainOf rustChannel; [
            rust-std
          ]));
        rustFmt = (fenix.packages.${system}.toolchainOf rustFmtChannel).rustfmt;
        rustAnalyzer = fenix.packages.${system}.rust-analyzer;

        callCargo = args: (pkgs.callPackage (import ./nix/cargo.nix) ({
          inherit rustToolchain;
          cargoRoot = ./.;
          preConfigure = ''touch git_version'';
        } // args));
        callCargoCross = args: callCargo ({
          inherit (oldPkgsCross) stdenv;
          buildInputs = mkBuildDeps oldPkgsCross;
          nativeBuildInputs = hostDeps ++ crossHostDeps;
          env = env // crossEnv;
        } // args);
        callCargoNative = args: callCargo ({
          inherit (pkgs) stdenv;
          buildInputs = mkBuildDeps pkgs;
          nativeBuildInputs = hostDeps ++ nativeHostDeps;
          env = env // nativeEnv;
        } // args);

        mkShell = buildPkgs: extraHostDeps: extraEnv: buildPkgs.mkShell ({
          buildInputs = mkBuildDeps buildPkgs;
          nativeBuildInputs = [
            rustAnalyzer
            rustFmt
            rustToolchain
            pkgs.gnuplot
            pkgs.teleport
            (pkgs.writeShellScriptBin "ci" ''nix/ci.sh "$@"'')
          ] ++ hostDeps ++ extraHostDeps;
        } // env // extraEnv);

      in
      {
        packages = {
          clippy = callCargoCross {
            name = "clippy";
            cargoCommand = "clippy";
            cargoStepTwoArgs = [ "--tests" "-- --deny warnings" ];
            inherit flavours;
          };

          check_debug_report_version = callCargoNative {
            name = "check_debug_report_version";
            cargoCommand = "build";
            cargoStepTwoArgs = [ "--bin debug-report-schema" ];
            flavours = { inherit (flavours) prod; };
            postInstall = ''
              target/$CARGO_BUILD_TARGET/debug/debug-report-schema check-version
              target/$CARGO_BUILD_TARGET/debug/debug-report-schema export
              cp debug_report_schema.json debug_report_schema.csv $out/
            '';
          };

          test = callCargoNative {
            name = "test";
            cargoCommand = "test";
            flavours = { inherit (flavours) prod; };
          };

          doc = callCargoNative {
            name = "doc";
            cargoCommand = "doc";
            cargoArgs = [ "--no-deps" ];
            cargoStepTwoArgs = [ "--document-private-items" ];
            flavours = { inherit (flavours) prod; };
            preBuild = ''export RUSTDOCFLAGS="-Dwarnings"'';
            postInstall = ''cp -r target/doc/* $out/'';
          };

          build = callCargoCross {
            name = "build";
            cargoCommand = "build";
            cargoArgs = [ "--release" ];
            cargoStepTwoArgs = [
              ''$([ "$flavour" == "prod" ] || [ "$flavour" == "stage" ] && echo "--config profile.release.lto=true")''
            ];
            inherit flavours;
            postBuildFlavour = ''
              mkdir -p $out/$flavour
              fd . target/$CARGO_BUILD_TARGET/release \
                --exact-depth 1 \
                --type executable \
                --exec cp '{}' $out/$flavour/
            '';
            postInstall = ''
              fd . $out \
                --type executable \
                --exec patchelf --set-interpreter /lib/ld-linux-aarch64.so.1 '{}'
            '';
          };
        };

        devShells = rec {
          cross = mkShell oldPkgsCross crossHostDeps (crossEnv // { name = "orb-core-cross"; });
          native = mkShell pkgs nativeHostDeps (nativeEnv // { name = "orb-core-native"; });
          livestream = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              gst_all_1.gst-libav.dev
              gst_all_1.gst-plugins-base.dev
              gst_all_1.gstreamer.dev
            ];
          };
          default = cross;
        };

        formatter = pkgs.nixpkgs-fmt;
      }
    );
}
