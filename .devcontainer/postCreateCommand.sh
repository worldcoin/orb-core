#!/bin/bash

set -eux

sudo chown -R vscode /workspaces/orb-core || true

# We don't directly mount the ssh directory, as we only wish to link certain files in .ssh
# So, we mount it at ~/.ssh_mirror and symlink only certain files.
mkdir -p ~/.ssh
if [ -e ~/.ssh_mirror/known_hosts ]; then
	ln -sf ~/.ssh_mirror/known_hosts ~/.ssh/
fi

if [ -e ~/.ssh_mirror/config ]; then
	ln -sf ~/.ssh_mirror/config ~/.ssh/
fi

# Get direnv to work in the bash scripts
if [ ! -e .envrc ]; then
	cp .envrc.example .envrc # Bootsrap for the user
fi
direnv allow .
eval "$(direnv export bash)"

if [ -e .devcontainer/postCreateCommand.user.sh ]; then
	.devcontainer/postCreateCommand.user.sh
fi
