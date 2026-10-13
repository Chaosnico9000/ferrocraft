# Ferrocraft

Ein Minecraft-Server (Java Edition **26.3**, Protokoll 777) in Rust, von Grund auf
geschrieben: keine Vanilla-Klassen, kein JVM-Wrapper.

## Stand

| Phase | Was | Status |
| --- | --- | --- |
| Handshake | `intention`, auch Transfer-Logins | ✅ |
| Status | Serverliste mit MOTD, Spielerzahl, Version; Ping/Pong | ✅ |
| Login | Offline-Modus, Vanilla-Offline-UUID, Versionsprüfung | ✅ (endet mit Disconnect) |
| Configuration | Known Packs, Registry-Daten, Feature Flags | ⏳ |
| Play | Join, leere Chunks, Position, Keep-Alive | ⏳ |
| Welt | Chunk-Generierung, Blöcke setzen/abbauen | ⏳ |
| Online-Modus | Verschlüsselung, Mojang-Session-Auth, Kompression | ⏳ |

Wer sich heute verbindet, landet in der Serverliste korrekt und bekommt beim
Beitreten eine lesbare Meldung statt eines Protokollfehlers.

## Starten

```sh
nix develop          # oder direnv
cargo run --release  # legt beim ersten Start ferrocraft.toml an
```

```toml
# ferrocraft.toml
bind = "0.0.0.0:25565"
motd = "Ein Ferrocraft-Server"
max_players = 20
```

Log-Level über `RUST_LOG`, z. B. `RUST_LOG=debug cargo run`.

## Aufbau

```
crates/protocol   Wire-Typen (VarInt, Strings, UUID), Framing, Pakete je Zustand
crates/server     Netzwerk (tokio, ein Task pro Verbindung), Status, Login, Config
data/26.3         packets.json aus Mojangs Datengenerator: die Quelle der Paket-IDs
scripts           gen-reports.sh: Berichte für eine neue Version erzeugen
```

Geplant ist ein fester Tick-Thread (20 TPS) für den Weltzustand, der mit den
Verbindungs-Tasks über Kanäle spricht, und Chunk-Generierung auf einem
Worker-Pool.

## Lizenz

MIT
