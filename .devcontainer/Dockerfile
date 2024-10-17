# See here for image contents: https://github.com/microsoft/vscode-dev-containers/tree/v0.245.2/containers/ubuntu/.devcontainer/base.Dockerfile
# FROM --platform=linux/amd64 mcr.microsoft.com/vscode/devcontainers/base:ubuntu-22.04
FROM mcr.microsoft.com/vscode/devcontainers/base:ubuntu-22.04

RUN apt-get update && export DEBIAN_FRONTEND=noninteractive && \
    apt-get -y install --no-install-recommends \
    direnv \
    xz-utils

# TODO(androant): For some reason some Nix users require to read this file and fails if it can't.
RUN if [ -f "/tmp/build-features/devcontainer-features.builtin.env" ]; then \
    chmod a+r /tmp/build-features/devcontainer-features.builtin.env; \
    fi

USER vscode
ENV USER="vscode"

# The Nix daemon won't work with vscode-ubuntu as systemd is disabled, thus we
# install Nix in single user more
RUN /bin/bash -c "sh <(curl -L https://nixos.org/nix/install) --no-daemon"

# As we have a local Nix installation, we need to edit the local config
RUN mkdir -p ${HOME}/.config/nix
RUN echo "experimental-features = nix-command flakes" >>"${HOME}/.config/nix/nix.conf"

# Unfortunately docker doesn't load the user's env variables automatically.
# Similar to:
# https://github.com/NixOS/nix/blob/0eb9946e1d3621cfc2fcffc9378dba334b25fb26/docker.nix#L259-L264
ENV PATH="${PATH}:/home/vscode/.nix-profile/bin"

RUN nix-env -iA nixpkgs.cachix
RUN nix-env -iA nixpkgs.shadow
RUN nix-env -iA nixpkgs.nix-direnv nixpkgs.nixpkgs-fmt nixpkgs.nil
RUN nix-env -iA nixpkgs.git-lfs

# Setup cachix
ARG CACHIX_AUTH_TOKEN
ENV CACHIX_AUTH_TOKEN="${CACHIX_AUTH_TOKEN}"
RUN if [ -z "${CACHIX_AUTH_TOKEN}" ]; then \
    echo "You need to provide the CACHIX_AUTH_TOKEN variable in your environment!"; \
    exit 1; \
    fi
RUN cachix authtoken "${CACHIX_AUTH_TOKEN}"
RUN cachix use worldcoin

ARG GITHUB_TOKEN
ENV GITHUB_TOKEN="${GITHUB_TOKEN}"
RUN if [ -z "${GITHUB_TOKEN}" ]; then \
    echo "You need to provide the GITHUB_TOKEN variable in your environment!"; \
    exit 1; \
    fi
# Git-LFS in Nix requires the following configuration. Look: fetch_from_private_git_hub_with_lfs.nix
ENV GIT_CONFIG_KEY_0="url.https://${GITHUB_TOKEN}:x-oauth-basic@github.com/worldcoin/.insteadOf"
ENV GIT_CONFIG_VALUE_0="https://github.com/worldcoin/"
ENV GIT_CONFIG_COUNT="1"
RUN echo "access-tokens = github.com=${GITHUB_TOKEN}" >>"${HOME}/.config/nix/nix.conf"
# Check that the token actually works
RUN nix flake metadata github:worldcoin/priv-orb-core

# Setup the Nix development environment
COPY ./flake.nix ./flake.lock ./scripts/git-config-token.sh /tmp/
COPY ./nix /tmp/nix

# For some reason, pytorch installation needs access to /root/miniforge3/lib.
# TODO: Remove this when we migrate from pytorch.
RUN sudo chmod 777 /root

RUN /bin/bash /tmp/nix/cross.sh echo OK
RUN /bin/bash /tmp/nix/native.sh echo OK

# Setup user's ~/.gitconfig after nix as we have observed issues with git-lfs. For some reason git-lfs config in
# ~/.gitconfig or /etc/gitconfig is interfering with Nix and makes git-lfs failing when Nix is trying to download AI
# models. In case you see errors in your manual commands on nix (e.g. if you are downloading a new model), use
# `export GIT_CONFIG_NOSYSTEM=1` or `export GIT_CONFIG_NOGLOBAL=1` to make Nix ignore the global or system git config.
RUN /bin/bash /tmp/git-config-token.sh ${GITHUB_TOKEN}

ARG CONTAINER_WORKSPACE_FOLDER
RUN /bin/bash -c "git config --global --add safe.directory ${CONTAINER_WORKSPACE_FOLDER}"

# Downloading Rust index takes time. We cache it during build time.
RUN /bin/bash /tmp/nix/cross.sh cargo new hello
RUN echo 'tokio = { version = "1", features = ["full"] }' >>/tmp/hello/Cargo.toml
RUN /bin/bash /tmp/nix/cross.sh cargo fetch --manifest-path /tmp/hello/Cargo.toml

ARG CONTAINER_WORKSPACE_FOLDER
# Configure direnv to allow .envrc of orb-core automatically.
RUN mkdir -p "${HOME}/.config/direnv"
RUN echo "[whitelist]" >>"${HOME}/.config/direnv/direnv.toml"
RUN echo "prefix = [ \"${CONTAINER_WORKSPACE_FOLDER}\" ]" >>"${HOME}/.config/direnv/direnv.toml"

# Setup nix-direnv.
RUN echo "source ${HOME}/.nix-profile/share/nix-direnv/direnvrc" >>"${HOME}/.direnvrc"
# Now we need to setup the shell hooks that enable/disable the env when we change directories.
RUN echo 'eval "$(direnv hook bash)"' >>"${HOME}/.bashrc"
RUN echo 'eval "$(direnv hook zsh)"' >>"${HOME}/.zshrc"
