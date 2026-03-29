#!/usr/bin/env bash
set -euo pipefail

# Ensure working directory is clean
if [[ -n $(git status --porcelain) ]]; then
  echo "error: working directory is not clean" >&2
  exit 1
fi

# Read version from Cargo.toml
version=$(cargo metadata --format-version 1 --no-deps | grep -o '"version":"[^"]*"' | head -1 | cut -d'"' -f4)
if [[ -z "$version" ]]; then
  echo "error: could not read version from Cargo.toml" >&2
  exit 1
fi

# Run cargo tests to ensure everything is working before publishing
echo "Running tests..."
cargo clean
cargo test --verbose --all

echo "Trying to publish version $version..."

tag="v${version}"
head_sha=$(git rev-parse HEAD)
need_to_create_tag=false

# Check if tag already exists
if git rev-parse "$tag" &>/dev/null; then
  tag_sha=$(git rev-parse "$tag^{commit}")
  if [[ "$tag_sha" != "$head_sha" ]]; then
    echo "error: tag $tag already exists but points to a different commit" >&2
    echo "  tag:  $tag_sha" >&2
    echo "  HEAD: $head_sha" >&2
    exit 1
  fi
  echo "Tag $tag already exists at HEAD"
else
  need_to_create_tag=true
  echo "Tag $tag does not exist and will be created at HEAD"
fi

# Ask user to confirm publishing
read -p "Are you sure you want to publish version $version? [y/N] " -n 1 -r

if [[ ! $REPLY =~ ^[Yy]$ ]]; then
  echo "Aborting."
  exit 0
fi

if $need_to_create_tag; then
  echo "creating tag $tag at HEAD..."
  git tag "$tag"
fi

echo "publishing once-arc $version to crates.io..."
cargo publish

echo "pushing tag $tag..."
git push origin "$tag"

echo "done"
