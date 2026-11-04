#!/usr/bin/env bash
# The config digest of a local image (sha256:…), read from `docker save`'s manifest.json, which Docker writes on both
# the classic and the containerd image store. The config covers the labels, the command and every layer's diff_id, so
# equal config digests mean the same image. release.yml compares it with the pushed image's; rust.yml's image
# job runs it on both runners (one store each), so it is proved before a tag depends on it.
# Usage: image_config_digest.sh <image>
set -euo pipefail
image=${1:?usage: image_config_digest.sh <image>}
dir=$(mktemp -d)
trap 'rm -rf -- "$dir"' EXIT
docker save -o "$dir/image.tar" "$image"
config=$(tar -xOf "$dir/image.tar" manifest.json | jq -r '.[0].Config')
hex=${config#blobs/sha256/}; hex=${hex%.json}
[[ $hex =~ ^[0-9a-f]{64}$ ]] || { echo "::error::no config digest in $image's manifest.json ('$config')" >&2; exit 1; }
# The blob is the config itself: it hashes to its name and lists the layers.
[ "$(tar -xOf "$dir/image.tar" "$config" | sha256sum | cut -d' ' -f1)" = "$hex" ] || { echo "::error::$config does not hash to its name" >&2; exit 1; }
tar -xOf "$dir/image.tar" "$config" | jq -e '.rootfs.diff_ids | length > 0' > /dev/null
echo "sha256:$hex"
