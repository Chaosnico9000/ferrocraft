# Ferrocraft

Ein Minecraft-Server (Java Edition **26.3**, Protokoll 777) in Rust, von Grund auf
geschrieben: keine Vanilla-Klassen, kein JVM-Wrapper.

## Stand

| Phase | Was | Status |
| --- | --- | --- |
| Handshake | `intention`, auch Transfer-Logins | ✅ |
| Status | Serverliste mit MOTD, Spielerzahl, Version; Ping/Pong | ✅ |
| Login | Online-Modus (Mojang-Auth, Verschlüsselung), Offline-Modus, Kompression, Versionsprüfung, Spielerlimit, Doppel-Login | ✅ |
| Configuration | Known Packs, Registry-Daten, Tags, Feature Flags | ✅ |
| Play | Join, Chunks nachladen beim Laufen, Keep-Alive, Tageszeit | ✅ |
| Mehrspieler | Andere Spieler sehen (Bewegung, Kopf, Schleichen, Arm, Ausrüstung, Skins), Tab-Liste mit Ping, Chat | ✅ |
| Spielmodi | Überleben (Abbauzeit nach Werkzeug, Loot-Tabellen, Werkzeugverschleiß, Leben, Fallschaden, Hunger, Essen, Tod und Respawn), Kreativ, Abenteuer, Zuschauer | ✅ |
| Items | Fallen lassen (Q, Strg+Q, aus dem Inventar), aufsammeln, Mittelklick, Inventar-Klicks wie Vanilla (Ziehen, Shift, Zahlentasten, Doppelklick) | ✅ |
| Blöcke | Setzen und abbauen (Türen, Betten, Treppen, Doppelkisten), Türen/Falltüren/Zauntore/Hebel/Knöpfe benutzen, Licht wird neu berechnet | ✅ |
| Block-Entities | Kisten, Fässer, Spender, Trichter, Shulker-Kisten, Endertruhe, Werkbank (Rezepte von Vanilla), Schilder beschreiben | ✅ |
| Befehle | `/list`, `/gamemode`, `/time`, `/tp`, `/give`, `/kill` (die letzten für Operatoren) | ✅ |
| Welt | Speichern im Vanilla-Format samt `level.dat`, Vanilla-Welten laden | ✅ |
| Fehlt | Mobs, Öfen und Braustände, Redstone, Wasser und Lava fließen, Verzauberungen, Explosionen; alles in [`OFFENE-PUNKTE.md`](OFFENE-PUNKTE.md), auch die geplante Schicht für Fabric- und Forge-Mods | ⏳ |

Wer sich verbindet, steht in einer endlosen Superflat-Welt (Grundgestein, zwei
Lagen Erde, Gras), im Überlebensmodus oder in dem Modus aus `game_mode`, sieht
die anderen Spieler und kann bauen.
Registry-Daten und Chunks sind byte-gleich zu dem, was ein Vanilla-Server
schickt; die Pakete über andere Spieler sind gegen Mitschnitte eines
Vanilla-Servers getestet.

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
view_distance = 10           # Chunk-Radius; der Client kann ihn verkleinern
compression_threshold = 256  # ab dieser Paketgröße komprimieren, -1 = nie
world = "world"              # Weltordner; leer = nichts speichern
online_mode = true           # Spieler bei Mojang prüfen (wie Vanilla)
game_mode = "survival"       # Modus neuer Spieler: survival, creative, adventure, spectator
operators = ["Chaosnico9000"] # dürfen /gamemode, /time, /tp, /give, /kill und F3+F4
```

## Die Welt

Gespeichert wird im Format und Ordner-Layout von Vanilla 26.3: Chunks samt
Kisteninhalten und Schildtexten in
`world/dimensions/minecraft/overworld/region/r.X.Z.mca`, Spieler (Position,
Blickrichtung, Inventar, Endertruhe, Leben, Spielmodus) in
`world/players/data/<uuid>.dat`, Spawn und Weltzeit in `level.dat` und
`data/minecraft/world_clocks.dat`. Ferrocraft schreibt
nur Chunks, in denen sich etwas geändert hat, alle 5 Minuten, beim Verlassen
eines Spielers und beim Beenden (Strg+C oder SIGTERM).

Vanilla 26.3 öffnet so eine Welt ohne Fehler (geprüft mit `data get block`
für Kisten und Schilder), und Ferrocraft lädt von Vanilla geschriebene Welten
samt Biomen und gespeichertem Licht. Was Ferrocraft nicht kennt (Entities,
fremde Block-Entities, Strukturen), bleibt beim Speichern erhalten.

Log-Level über `RUST_LOG`, z. B. `RUST_LOG=debug cargo run`. Die Tick-Zeit
erscheint alle 30 s mit `RUST_LOG=ferrocraft::tick=debug`.

## Werkzeuge

```sh
# Mitschnitt: jedes Paket zwischen Client und Server mit Zustand und Namen,
# komprimierte Pakete entpackt. Der Server dahinter muss offline laufen.
cargo run -p ferrocraft-devtools --bin proxy -- 127.0.0.1:25570 127.0.0.1:25565 --log pakete.jsonl

# Bots (nur Offline-Server, zum Bauen im Kreativmodus). Rollen reihum:
# walk (läuft, springt, schleicht), build (baut Türme und reißt sie ab),
# house (Kiste, Tür, Hebel), throw (wirft Items und sammelt sie ein)
cargo run -p ferrocraft-devtools --bin bot -- 127.0.0.1:25565 --count 8 --radius 4
cargo run -p ferrocraft-devtools --bin bot -- 127.0.0.1:25565 --roles build,walk

# Daten für eine Version neu erzeugen (Paket-IDs, Registries, Tags, Blöcke,
# Items, Rezepte, Loot-Tabellen). Braucht Java: Mojangs Datengenerator und
# scripts/BlockPhysics.java (Härte, Licht, Kollisionsformen je Zustand)
# laufen gegen den Vanilla-Jar. Der Server selbst bleibt reines Rust.
scripts/gen-reports.sh 26.3
```

## Aufbau

```
crates/protocol   Wire-Typen (VarInt, Strings, UUID, BlockPos), NBT, Chunk-Format,
                  Framing mit Kompression, Verschlüsselung, Pakete je Zustand
crates/server     Netzwerk (tokio, ein Task pro Verbindung), Status, Login und
                  Mojang-Auth (RSA über aws-lc-rs), Configuration, Play,
                  game/ (Tick-Thread: Spieler, Menüs, Überleben, Items,
                  Befehle), Welt, Licht, Block-Entities, Rezepte, Loot,
                  Speichern
crates/devtools   Test-Client, gen-data (Daten aus dem Datengenerator),
                  proxy (Mitschnitt), bot
data/26.3         Aus Mojangs Datengenerator: packets.json (Paket-IDs),
                  registries.json, tags.json, blocks.json (mit Physik),
                  items.json, recipes.json, loot_tables.json, ids.json
                  (in den Server eingebettet)
scripts           gen-reports.sh: Daten für eine neue Version erzeugen
```

Der Tick-Thread läuft mit festen 20 TPS und hält den Spielzustand: Spieler,
Inventare, jede Blockänderung und den Chat, damit alle dieselbe Reihenfolge
sehen. Die Verbindungs-Tasks sprechen mit ihm nur über Kanäle. Sie melden, was
ihr Spieler tut; er schickt fertige Pakete und Änderungen zurück, und jede
Verbindung entscheidet selbst, was ihr Client davon sieht (geladene Chunks,
Sichtweite), wie `ChunkMap.TrackedEntity` bei Vanilla.

## Lizenz

MIT
