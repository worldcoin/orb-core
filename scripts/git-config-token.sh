#!/usr/bin/env bash
set -eux

URLS=(
    "ssh://git@github.com/worldcoin/"
    "git@github.com:worldcoin/"
    "https://github.com/worldcoin/"
)

GITHUB_TOKEN="${1:-}"

if ! git --version >/dev/null 2>&1; then
    echo "Git is not installed"
    exit 1
fi

if ! git lfs version >/dev/null 2>&1; then
    echo "Git LFS is not installed"
    exit 1
fi

# Exit if token is not provided
if [[ -z "${GITHUB_TOKEN}" ]]; then
    echo "GitHub token not provided."
    exit 1
fi

for URL in "${URLS[@]}"; do
    git config --global --add url."https://${GITHUB_TOKEN}:x-oauth-basic@github.com/worldcoin/".insteadOf "${URL}"
done

git lfs install
