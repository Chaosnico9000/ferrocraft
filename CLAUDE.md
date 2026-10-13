# CLAUDE.md

Minecraft-Server (Java Edition 26.3) in Rust. Was er kann und der Aufbau stehen in
`README.md`.

## Bauen und prüfen

- Die Toolchain kommt aus `flake.nix` (Rust aus nixpkgs, dazu JDK 25 und jq nur
  für `scripts/gen-reports.sh`). Befehle laufen in `nix develop` bzw. über direnv.
- `cargo fmt && cargo clippy --all-targets && cargo test`: vor jedem Commit,
  Clippy ohne Warnungen.
- Gegenprobe mit einer fremden Implementierung:
  `nix shell nixpkgs#python3Packages.mcstatus -c mcstatus 127.0.0.1:25565 status`
  (und `ping`). Ein echter Client lässt sich hier nicht starten.

## Protokoll-Fakten nachschlagen, nicht raten

- **Paket-IDs** stehen in `data/<version>/packets.json`, erzeugt von Mojangs
  Datengenerator (`scripts/gen-reports.sh <version>`). Die Konstante `ID` jedes
  Pakets muss dazu passen.
- **Feldlayouts**: Seit 26.1 ist der Vanilla-Jar nicht mehr obfuskiert. Der Codec
  eines Pakets lässt sich direkt lesen:
  `unzip` den inneren `META-INF/versions/<v>/server-<v>.jar`, dann
  `javap -c -p net/minecraft/network/protocol/<state>/<Paket>.class`.
- **Protokollnummer**: `protocol_version` in `version.json` des Server-Jars
  (`PROTOCOL_VERSION` in `crates/protocol/src/lib.rs`).

## Fallen

- **Login-Disconnect ist JSON, die späteren sind NBT.** `login_disconnect` trägt
  eine Text-Komponente als JSON-String; Disconnects in Configuration und Play
  erwarten NBT. Nicht denselben Encoder verwenden.
- **Ping ohne Status-Request.** Vanilla beantwortet `ping_request` auch, wenn kein
  `status_request` vorausging (mcstatus `ping` macht genau das).
- **String-Limits zählen UTF-16-Einheiten**, nicht Bytes: `decode_string` prüft
  beides wie Vanilla (Bytes ≤ 3 × Limit, dann Zeichen).
- **Legacy-Ping** (erstes Byte `0xFE`) liest sich als unfertige Frame-Länge; der
  Timeout in `connection.rs` beendet solche Verbindungen.
