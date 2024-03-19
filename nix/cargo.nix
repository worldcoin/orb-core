{ fetchurl
, formats
, lib
, ncurses
, runCommandLocal
, stdenv
, symlinkJoin
, yq
, zstd

, name
, cargoRoot
, cargoCommand
, cargoArgs ? [ ]
, cargoStepTwoArgs ? [ ]
, flavours
, rustToolchain
, buildInputs
, nativeBuildInputs
, env
, preConfigure ? null
, preBuild ? null
, postBuildFlavour ? null
, postInstall ? null
}:

let
  cargoToml = builtins.fromTOML (builtins.readFile (cargoRoot + "/Cargo.toml"));
  cargoLock = builtins.fromTOML (builtins.readFile (cargoRoot + "/Cargo.lock"));

  cargoConfig = (formats.toml { }).generate "cargo-config" {
    source = {
      crates-io = { replace-with = "nix-sources"; };
      nix-sources = { directory = unpackedDeps; };
    } // lib.listToAttrs (map (x: x.cargoConfig) gitDeps);
  };

  # Collect external dependencies from crates.io.
  cratesioDeps = map
    (dep: rec {
      inherit (dep) name version checksum;
      crate = fetchurl {
        name = "${name}-${version}-source";
        url = "https://crates.io/api/v1/crates/${name}/${version}/download";
        sha256 = checksum;
      };
    })
    (builtins.filter (builtins.hasAttr "checksum") cargoLock.package);

  # Collect external git dependencies.
  gitDeps = builtins.map
    (dep: rec {
      inherit (dep) name;
      url = matchFirst "git\\+(.*)\\?.*" dep.source;
      tag = matchFirst ".*\\?tag=(.*)#.*" dep.source;
      rev = matchFirst ".*\\?rev=(.*)#.*" dep.source;
      cargoConfig = {
        name = matchFirst "git\\+(.*\\?.*)#.*" dep.source;
        value = {
          git = url;
          replace-with = "nix-sources";
        }
        // lib.optionalAttrs (tag != null) { inherit tag; }
        // lib.optionalAttrs (rev != null) { inherit rev; };
      };
      checkout = builtins.fetchGit ({
        inherit url;
        rev = matchFirst ".*#(.*)" dep.source;
      }
      // lib.optionalAttrs (tag != null) { ref = "refs/tags/${tag}"; }
      // lib.optionalAttrs (rev != null) { allRefs = true; });
    })
    (builtins.filter
      (dep: dep ? source && lib.hasPrefix "git+" dep.source)
      cargoLock.package);

  # Store a crates.io dependency in Nix store in a form consumable by cargo.
  unpackedCratesioDep = { name, version, checksum, crate }:
    runCommandLocal "${name}-${version}-unpacked" { } ''
      mkdir -p $out
      tar -xzf ${crate} -C $out
      echo '{"package":"${checksum}","files":{}}' > $out/${name}-${version}/.cargo-checksum.json
    '';

  # Store a git dependency in Nix store in a form consumable by cargo.
  unpackedGitDep = dep:
    runCommandLocal "${cargoToml.package.name}-${dep.name}-${cargoToml.package.version}-unpacked"
      {
        inherit (dep) name url checkout;
        key = dep.tag or dep.rev;
        nativeBuildInputs = [ rustToolchain yq ];
      }
      ''
        while read -r toml; do
          pname=$(tomlq -r .package.name $toml)
          version=$(tomlq -r .package.version $toml)
          [ "$name" == "$pname" ] && [ -n "$version" ] || continue
          dest="$out/$(echo "$name-$version-$key" | sed 's|/|_|' | head -c 255)"
          mkdir -p $dest
          ln -s $(dirname $toml)/* $dest
          echo '{"package":null,"files":{}}' > $dest/.cargo-checksum.json
          exit 0
        done <<< $(find $checkout -name Cargo.toml)
        exit 1
      '';

  # Combine both crates.io and git dependencies under one directory.
  unpackedDeps = symlinkJoin {
    name = "dependencies";
    paths = (map unpackedCratesioDep cratesioDeps) ++
      (map unpackedGitDep gitDeps);
  };

  matchFirst = regex: str: lib.mapNullable lib.head (builtins.match regex str);

  cargoDerivation = { nameSuffix ? null, cargoArgs ? [ ], postBuildFlavour ? null, ... } @ args: stdenv.mkDerivation ({
    inherit cargoConfig unpackedDeps buildInputs;
    pname = lib.concatStringsSep "-" ([ cargoToml.package.name name ]
      ++ (lib.optional (nameSuffix != null) nameSuffix));
    version = cargoToml.package.version;
    nativeBuildInputs = [ rustToolchain ncurses ] ++ nativeBuildInputs;

    flavours = lib.mapAttrsToList
      (flavour: features: "${flavour}:${lib.concatStringsSep "," features}")
      flavours;

    cargoCmd = lib.concatStringsSep " " ([
      "cargo"
      cargoCommand
      "--workspace"
      "--no-default-features"
      "$features"
      "--jobs $NIX_BUILD_CORES"
    ] ++ cargoArgs);

    configurePhase = ''
      runHook preConfigure
      export SOURCE_DATE_EPOCH=1

      export RUST_TEST_THREADS=$NIX_BUILD_CORES
      export CARGO_HOME=$PWD/.cargo-home
      export CARGO_BUILD_RUSTFLAGS="$CARGO_BUILD_RUSTFLAGS --remap-path-prefix $unpackedDeps=/sources"
      mkdir -p $CARGO_HOME target
      ln -s $cargoConfig $CARGO_HOME/config
      find . -type f -exec touch {} +

      runHook postConfigure
    '';

    buildPhase = ''
      runHook preBuild
      export SOURCE_DATE_EPOCH=1

      for tuple in $flavours; do
        # See https://tldp.org/LDP/abs/html/string-manipulation.html
        flavour="''${tuple%%:*}"
        features="''${tuple##*:}"
        if [ -n "''${features}" ]; then
            features="--features ''${features}"
        fi
        cmd=$(eval echo $cargoCmd)
        echo "$(tput smso)<< SELECTED FLAVOUR: $flavour >>$(tput rmso)"
        (set -x; $cmd)
        ${lib.optionalString (postBuildFlavour != null) postBuildFlavour}
      done

      runHook postBuild
    '';
  } // args // env);

  dummyCargoToml = name: dir:
    let
      cargoToml = (builtins.fromTOML (builtins.readFile (cargoRoot + "/${dir}/Cargo.toml")));
      attrs = removeAttrs cargoToml [ "bin" "example" "lib" "test" "bench" "default-run" ]
        // { package = removeAttrs cargoToml.package [ "build" "default-run" ]; };
    in
    "${dir}:${(formats.toml { }).generate "${name}-Cargo.toml" attrs}";
  dummyCargoTomls = [ (dummyCargoToml cargoToml.package.name ".") ] ++ (map
    (member: dummyCargoToml member member)
    cargoToml.workspace.members
  );

  # Generate an empty source code hierarchy and compile only the external
  # dependencies.
  stepOne = cargoDerivation {
    inherit preConfigure preBuild cargoArgs;
    nameSuffix = "deps";

    src = runCommandLocal "dummy-src" { inherit dummyCargoTomls; } ''
      mkdir -p $out/.cargo
      ln -s ${cargoRoot + "/.cargo/config.toml"} $out/.cargo
      ln -s ${cargoRoot + "/Cargo.lock"} $out/Cargo.lock

      for tuple in $dummyCargoTomls; do
        member="''${tuple%%:*}"
        cargotoml="''${tuple##*:}"
        mkdir -p $out/$member/src
        ln -s $cargotoml $out/$member/Cargo.toml
        echo '#![no_std]' > $out/$member/src/lib.rs
        echo 'fn main() {}' > $out/$member/build.rs
      done
    '';

    installPhase = ''
      runHook preInstall
      export SOURCE_DATE_EPOCH=1

      # See: https://reproducible-builds.org/docs/archives/
      mkdir -p $out
      tar --sort=name \
        --mtime="@''${SOURCE_DATE_EPOCH}" \
        --owner=0 --group=0 --numeric-owner \
        --pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime \
        -c target | ${zstd}/bin/zstd -o $out/target.tar.zst

      runHook postInstall
    '';
  };

  # Re-use the `target` directory with pre-compiled external dependencies (from
  # the step one) and compile the real source code.
  stepTwo = cargoDerivation {
    inherit preConfigure preBuild postBuildFlavour postInstall;
    src = cargoRoot;
    cargoArgs = cargoArgs ++ cargoStepTwoArgs;

    postConfigure = ''
      ${zstd}/bin/zstd -d "${stepOne}/target.tar.zst" --stdout | tar -x
    '';

    installPhase = ''
      runHook preInstall
      export SOURCE_DATE_EPOCH=1

      mkdir -p $out

      runHook postInstall
    '';
  };

in
stepTwo
