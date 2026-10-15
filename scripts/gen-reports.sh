#!/usr/bin/env bash
# Runs Mojang's data generator on the vanilla server jar and stores the
# protocol-relevant reports under data/<version>/: packet ids, synchronized
# registries, tags, blocks, items, loot tables and recipes. What no report
# contains (light, hardness, collision of blocks; size and attributes of
# entities) comes from scripts/BlockPhysics.java and scripts/EntityInfo.java,
# run against the same jar. Needs java, jq and cargo
# (nix develop).
#   scripts/gen-reports.sh 26.3
set -euo pipefail

version="${1:?usage: $0 <minecraft-version>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

meta="$(curl -fsS https://piston-meta.mojang.com/mc/game/version_manifest_v2.json |
  jq -r --arg v "$version" '.versions[] | select(.id == $v) | .url')"
[ -n "$meta" ] || { echo "unknown version $version" >&2; exit 1; }
curl -fsS -o "$work/server.jar" "$(curl -fsS "$meta" | jq -r .downloads.server.url)"

(cd "$work" && java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports --server --output gen >/dev/null)
# The bundler has unpacked the server and its libraries next to the jar.
classpath="$(find "$work/versions" "$work/libraries" -name '*.jar' | tr '\n' ':')"
(cd "$work" && java -cp "$classpath" "$root/scripts/BlockPhysics.java" gen/block_physics.json >/dev/null)
(cd "$work" && java -cp "$classpath" "$root/scripts/EntityInfo.java" gen/entity_info.json >/dev/null)
# The English texts, for what the console shows of translated messages.
unzip -p "$work/versions/$version/server-$version.jar" assets/minecraft/lang/en_us.json >"$work/gen/en_us.json"

mkdir -p "$root/data/$version"
cp "$work/gen/reports/packets.json" "$root/data/$version/"
(cd "$root" && cargo run -q --release -p ferrocraft-devtools --bin gen-data -- "$work/gen" "data/$version")
unzip -p "$work/server.jar" version.json | jq '{id, protocol_version, world_version, java_version}'
