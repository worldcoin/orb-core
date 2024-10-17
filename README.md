[![CI](https://github.com/worldcoin/orb-core/actions/workflows/ci.yml/badge.svg)](https://github.com/worldcoin/orb-core/actions/workflows/ci.yml)

# Orb Core

NOTE: This FOSS release of orb-core is a fork of the internal repo, priv-orb-core.
The 

This repository contains the libraries and binaries running on the orb.

The binaries controlling the orb are found in [`src/bin/`]:

+ [`src/bin/orb-core.rs`]: the production binary, which runs signups in the field;
+ [`src/bin/orb-backend-connect.rs`]: a binary to ensure backend connectivity by scanning a WiFi QR code and establishing the WiFi connection as long as the backend is not reachable

[`src/bin/`]: src/bin/

[`src/bin/orb-core.rs`]: src/bin/orb-core.rs

[`src/bin/orb-backend-connect.rs`]: src/bin/src/bin/orb-backend-connect.rs

## Development Environment

The development environment, which includes pinned rust toolchain packages and
other build dependencies, is made using [Nix]. There are 3 options:

1. *(recommended)* Use Nix environment directly. You can install [NixOS] or
   install Nix package manager on other Linux distro. This way you will get less
   latency during development cycle and better integration with IDEs and other
   external tools.

2. Use docker wrapper. This can be used on any OS with docker installed. The
   downside is that rust-analyzer will not work through docker.

3. Use VSCode's devcontainer. This is similar to option (2) with the difference
   that rust-analyzer and all tools are properly working.

4. Use the devcontainer cli directly. Similar to (3) but it doesn't require using
   vscode.

### Vendoring Proprietary SDKs

Although all of Worldcoin's code in this repo is open source, some of the
sensors on the orb rely on proprietary SDKs provided by their hardware vendors.
Luckily, these are accessible without any cost.

To get started, you will need to download these SDKs. The process for this
depends on if you are officially affiliated with Worldcoin.

#### If you have access to Worldcoin private repos

1. Create a [personal access token][pac] from github to allow you to use
   private git repos over HTTPS.
2. Append the following to your `~/.config/nix/nix.conf`:
   ```
   access-tokens = github.com=github_pat_YOUR_ACCESS_TOKEN_HERE
   ```
3. Test everything works so far by running `nix flake metadata
   github:worldcoin/priv-orb-core`. You should see a tree of info. If not, you
   probably don't have your personal access token set up right - post in
   #public-orb-software on slack for help.

#### If you don't have access to Worldcoin private repos

Seek Thermal SDK:
1. Go to https://developer.thermal.com and  create a developer account.
2. Download the 4.1.0.0 version of the SDK (its in the developer forums).
3. Extract its contents, and note down the dir that *contains* the
   `Seek_Thermal_SDK_4.1.0.0` dir. Save this in an environment variable of your
   choice, such as `SEEK_SDK_OVERRIDE`.
4. modify your `.envrc` like this: `use flake --override-input seekSdk
   "$SEEK_SDK_OVERRIDE"`

Royale SDK:
1. Go to https://pmdtec.com/en/download-sdk/ and create an account
2. Download the SDK
3. Extract its contents, and note down its dir. Save this in an env var of your
   choice, such as `$ROYALE_SDK_OVERRIDE`.
4. modify your `.envrc` like this: `use flake --override-input seekSdk "$ROYALE_SDK_OVERRIDE"`

### Installation

#### Direnv

It's recommended to install [direnv] (v2.30+ for flake support) and integrate it
with your shell. When installed, the development environment will be loaded
automatically when you `cd` into the repo.

Move `.envrc.example` to `.envrc` (this is your private config, it shouldn't be
pushed to git):

    cp .envrc.example .envrc

##### (optional) Not using direnv

To load the environment manually:

1. Make sure that every environment variable from `.envrc.example` is loaded
   into your shell by other means.

2. Run `nix develop`.

#### Cachix

If you have a cachix cache, you can edit `.envrc` and uncomment the following
line.

    # export CACHIX_AUTH_TOKEN=<TOKEN>

With Cachix, you'll be able to reuse our private binary cache from GitHub
Actions CI. This will reduce time spent for compiling custom nix packages from
sources.

#### (Option 1) NixOS

1. **Additional NixOS configuration**

    Edit your NixOS configuration (`/etc/nixos/configuration.nix` by default),
    and add the following options:

        nix = {
          trustedUsers = [ "@wheel" ]; # your user should belong to the `wheel` group
          extraOptions = ''
            experimental-features = nix-command flakes
          '';
        };

2. **Set up Cachix**

    Authenticate Cachix (`$CACHIX_AUTH_TOKEN` should be already loaded into your
    shell by [direnv]):

        nix-shell -p cachix --run 'cachix authtoken $CACHIX_AUTH_TOKEN'

    Configure Worldcoin binary cache:

        nix-shell -p cachix --run 'cachix use worldcoin'

#### (Option 1) Nix on other Linux distro

NOTE: Despite the Nix package manager is available for MacOS, the current
environment currently doesn't work on MacOS.

1. **Install Nix**

    Visit [NixOS Downloads](https://nixos.org/download.html) page and follow the
    instructions to install "Nix: the package manager".

2. **Enable Flakes**

    Edit either `~/.config/nix/nix.conf` or `/etc/nix/nix.conf` and add:

        experimental-features = nix-command flakes

3. **Set up Cachix**

    Install Cachix client:

        nix-env -iA cachix -f https://cachix.org/api/v1/install

    Authenticate Cachix (`$CACHIX_AUTH_TOKEN` should be already loaded into your
    shell by [direnv]):

        cachix authtoken $CACHIX_AUTH_TOKEN

    Configure Worldcoin binary cache:

        cachix use worldcoin
4. **Add Github auth token**

        echo "access-tokens = github.com=${GIT_HUB_TOKEN}" >>${HOME}/.config/nix/nix.conf

#### (Option 2) Docker wrapper

This works on MacOS, including M1.

1. **Build docker image**

    Run the following script to build the docker image, with Nix inside
    (`$CACHIX_AUTH_TOKEN` should be loaded into your shell by [direnv]):

        docker/build.sh

    *NOTE* Keep in mind that the docker image should be rebuilt after each
    change to `flake.lock`, `flake.nix`, or `nix/*.nix`.

2. **Prepend every command with the wrapper**

    Every command referenced in this readme should be prepended with
    `docker/run.sh` or `docker/run.sh nix/cross.sh`. For example:

    * `cargo fmt` becomes `docker/run.sh nix/cross.sh cargo fmt`

    * `cargo clippy --all` becomes `docker/run.sh nix/cross.sh cargo clippy
      --all`

    * `nix/native.sh cargo test --all` becomes `docker/run.sh nix/native.sh
      cargo test --all` (note no `nix/cross.sh`)

#### (Option 3) VSCode with Docker

This works on MacOS, including M1, or any other environment.

1. **Environment setup before launching VSCode**

    You need to export `$CACHIX_AUTH_TOKEN` in your local environment. If you
    are on MacOS, `zsh` is the default shell and thus you should add the
    variable in `~/.zshenv`.

2. **Build docker image**

    When VSCode is launched/pointed in your local copy of this repository,
    VSCode should automatically detect the `devcontainer` configuration and
    prompted you to open the repository inside a new docker container.
    Everything, including building and configurations should automatically work.
    If something doesn't, it's a bug and should be reported.

3. **Have fun!**

    When the container is loaded and is running, you can use VSCode's terminal
    or your own one. We have added some nice plugins to make the experience as
    pleasant as possible (e.g. a spell check, rust-analyzer, formatters, etc).
    Just don't forget to use `nix/release.sh` to make the final executable
    compatible with the orb!

#### (Option 4) Devcontainer CLI

This works on MacOS, including M1, or any other environment.

1. **Install nodejs**

   The devcontainer cli uses nodejs instead of a real programming language, so
   first install a version manager for node. We suggest 
   [fnm](https://github.com/Schniz/fnm), but you could use `nvm` instead. Follow
   the instructions for the version manager to add it to your path and 
   [shell](https://github.com/Schniz/fnm#shell-setup), and install the latest LTS
   of nodejs. For `fnm`, this is `fnm default <version>`. Now try running
   `node --version` and `npm` to see if everything is working.

2. **Install devcontainer cli**

   Once node and npm is installed, run `npm install -g @devcontainers/cli` to
   install the devcontainer cli.

3. **Build and run your devcontainer**
   
   You can now build and run the actual devcontainer with 
   `.devcontainer/run.sh [command]`. This will build the devcontainer and start it
   if it is not already running, then attach to it, much like `docker exec`.

Note that you may create `.devcontainer/postCreateCommand.user.sh` to customize what
gets placed in your devcontainer in the `devcontainer up` command. Read 
[this](https://code.visualstudio.com/docs/devcontainers/create-dev-container#_rebuild)
if you want more info.


### Usage

The development environment consists of "cross" (for cross-compilation)
environment and "native" (not cross-compiling) environments. If you use Nix,
"cross" environment is loaded by default, otherwise it can be accessed with
`nix/cross.sh <COMMAND>` script. "native" environment is accessed with
`nix/native.sh <COMMAND>`.

The root crate has cargo features for each supported Orb version. The most
recent version is activated by default to streamline the development process.
Here is an example how to activate a different version:

    cargo clippy --all --no-default-features --features v2_x_x

We try to keep the environment in a shape, where you can use standard Rust
workflows. For example `cargo clippy` to run lints for the root crate of the
workspace with default features enabled, or `cargo fmt`, which runs rustfmt from
a different Rust toolchain. However there is an exception for `cargo build
--release`. You need to use `nix/release.sh` instead (see below).

#### Rust Analyzer

The default environment has fully configured analyzer by default path
`rust-analyzer`. A Rust-capable IDE should pick it up out-of-the-box when ran
from a shell with the environment.

#### Rustfmt

We use rustfmt with a custom configuration including some nightly options. It's
already configured in the development environment and accessible by default
paths like `cargo fmt` and `rustfmt`. Your editor should pick it up
automatically.

#### `ci` command

When inside Nix shell, you can use `ci` command to run frequently used commands
with correct arguments. For example to run clippy and tests:

    ci clippy test

Check all available commands with:

    ci help

#### Clippy

We always check our code with clippy. Here is how to run it:

    ci clippy

#### Running tests

We don't cross-compile tests for performance reasons. They should be run in a
separate environment:

    ci test

#### Documentation

To build `rustdoc` documentation locally and open it in the browser:

    ci doc --open

#### Release builds

Nix hard-codes a custom path to the Linux dynamic loader. There is a separate
script, which compiles binaries in the release mode and fixes the dynamic
loader:

    nix/release.sh --all

or to compile a particular binary

    nix/release.sh --package orb --bin binary-name

Compiled and fixed binaries are located at
`target/aarch64-unknown-linux-gnu/release`.

[Nix]: https://nixos.org
[NixOS]: https://nixos.org
[direnv]: https://direnv.net

#### Running CI checks locally

To run all CI checks locally, you can use `nix/ci.sh` script:

    nix/ci.sh --server fmt clippy test build

## Notes on MacOS and M1 Macs

Note that due to the containers sharing a filesystem with the host, running
`cargo` through docker is painfully slow. Docker Desktop has a 2 experimental
features: `Virtualization Framework` and `VirtioFS`, which makes this less
painful. As of Docker Desktop v4.12.0 performance has been increased
significantly with cases reaching a 2-3x speedup in compiling. To enable both
`Virtualization Framework` and `VirtioFS`, go to Preference > Settings >
Experimental Features and enable both options.

The best way to build on M1 Macs is probably to set up a virtual environment
with Linux installed in it.

## Gather and display code coverage

To get test code coverage run `scripts/get-coverage.sh`. This script will
generate and `lcov.info` file in the root directory of this repository. You can
then use `lcov --summary lcov.info` or any other `lcov` compatible tool, in
order to read the coverage details.

For VSCode users, we have pre-installed the `Coverage Gutters` plugin to load
coverage information. From the VSCode command palette run `Coverage Gutters:
Display Coverage` to manually load `lcov.info`. `Coverage Gutters` also supports
automatic watchers for automatically reloading coverage information.

Lastly, `scripts/get-coverage.sh` uses [grcov](https://github.com/mozilla/grcov)
to generate coverage data. `grcov` also supports generating reports in HTML. To
enable this functionality run `scripts/get-coverage.sh -H`.


## Auxiliary binaries

### Standalone biometric pipeline

You can run the biometric pipeline directly on saved images:

``` shell
nix/release.sh --bin compute_iris_code
```

To run:

0. Kill any running Orb Core binary
1. Copy the output to your Orb: `scp ./target/aarch64-unknown-linux-gnu/release/compute_iris_code worldcoin@orb:/home/worldcoin/`
2. On the Orb, set the network capability: `sudo setcap cap_net_raw+ep /home/worldcoin/compute_iris_code`
3. `source ./venv/bin/activate && ./compute_iris_code <image_paths>`

The image path must contain these files:

- ir_left.png
- ir_right.png
- face.png

We expect the formats/resolutions to match those that the Orb captures.

The results are written to `codes.json` on the Orb.

## Updating dependencies

Running just `cargo update` is enough.

## Troubleshooting

### Can't fetch a submodule

If you get an error that looks like:

``` sh
error: failed to get `orb-qr-link` as a dependency of package `orb v0.1.0 (/home/dan/repos/worldcoin/orb-core)`
```

try again after setting `export CARGO_NET_GIT_FETCH_WITH_CLI=true`.

### WPA supplicant related issues when executing binaries

Note: The Jetson system image should ship with a `wpa-supplicant-interface`
already setup. This information in this section should only be relevant if
you're changing this existing configuration.

Solution: `wpa-supplicant-interface` needs to be built, copied to the orb, and
be available in `$PATH` with the correct bits set:

```sh
$ nix/release.sh --package wpa-supplicant-interface
$ scp target/aarch64-unknown-linux-gnu/release/wpa-supplicant-interface worldcoin@<local orb IP address>:.
```

Next on the orb:

```sh
$ sudo mount -o remount,rw /
$ sudo cp wpa-supplicant-interface /usr/local/bin
# Ensure that the correct bits are set
$ sudo chown root:root /usr/local/bin/wpa-supplicant-interface
$ sudo chmod ug+s /usr/local/bin/wpa-supplicant-interface
# Ensure that /usr/local/bin is in PATH, otherwise:
$ export PATH="/usr/local/bin:$PATH"
```

Errors usually look like this:

```sh
[22-04-29 15:03:02.417 +00:00] T["tokio-runtime-worker"] ERROR [src/observer.rs:694] Status request failed: error sending request for url (https://api.worldcoin.dev/api/v1/orbs/d4fd59409be8db2c920e77e48e83ed21f45f2a14f232e46f1fc49cbe52a0fd17): error trying to connect: dns error: cancelled

Caused by:
    0: error trying to connect: dns error: cancelled
    1: dns error: cancelled
    2: cancelled
Error: `wpa-supplicant-interface` terminated unsuccessfully
```

### Missing GITHUB_TOKEN

During devcontainer setup, you get:
```
You need to provide the GITHUB_TOKEN variable in your environment!
```

In this case you need to create a GitHub personal access token (PAT) and place it in your environment (`~/.zshenv` or `~./bashrc`) under the name `GITHUB_TOKEN` .


## License

Unless otherwise specified, all code in this repository is dual-licensed under
either:

- MIT License ([LICENSE-MIT](LICENSE-MIT))
- Apache License, Version 2.0, with LLVM Exceptions
  ([LICENSE-APACHE](LICENSE-APACHE))

at your option. This means you may select the license you prefer to use.

Any contribution intentionally submitted for inclusion in the work by you, as
defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.
