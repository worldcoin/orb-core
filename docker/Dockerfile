FROM nixos/nix

# Enable Flakes
RUN echo "experimental-features = nix-command flakes" >> /etc/nix/nix.conf;

# Set up Cachix
ARG CACHIX_AUTH_TOKEN
RUN set -eux; \
    nix-env -iA cachix -f https://cachix.org/api/v1/install; \
    cachix authtoken $CACHIX_AUTH_TOKEN; \
    cachix use worldcoin;

# Install git-lfs
RUN nix-env -iA nixpkgs.git-lfs

# Set up user
ARG USER_ID
ARG GROUP_ID
RUN set -eux; \
    nix-env -iA nixpkgs.shadow; \
    groupadd --non-unique --gid $GROUP_ID worldcoin || true; \
    useradd --non-unique --uid $USER_ID --gid $GROUP_ID worldcoin || true; \
    mkdir -p /home/worldcoin; \
    chown -R $USER_ID:$GROUP_ID /nix /home/worldcoin;
USER $USER_ID:$GROUP_ID
ENV HOME=/home/worldcoin
WORKDIR $HOME/orb-core

# Set up nix development environment
COPY flake.nix flake.lock ./
COPY nix/ nix/
COPY scripts/git-config-token.sh scripts/git-config-token.sh
ARG GIT_HUB_TOKEN
RUN scripts/git-config-token.sh $GIT_HUB_TOKEN
RUN echo "access-tokens = github.com=${GIT_HUB_TOKEN}" >>${HOME}/.config/nix/nix.conf
RUN nix/cross.sh echo OK
RUN nix/native.sh echo OK
