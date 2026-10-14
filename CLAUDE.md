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
  (und `ping`). Ein echter Client lässt sich hier nicht starten; für mehrere
  Spieler gibt es `bot` (devtools), der Test-Client der Integrationstests liegt
  in `crates/devtools/src/client.rs`.

## Protokoll-Fakten nachschlagen, nicht raten

- **Paket-IDs** stehen in `data/<version>/packets.json`, erzeugt von Mojangs
  Datengenerator (`scripts/gen-reports.sh <version>`). Die Konstante `ID` jedes
  Pakets muss dazu passen.
- **Feldlayouts**: Seit 26.1 ist der Vanilla-Jar nicht mehr obfuskiert. Der Codec
  eines Pakets lässt sich direkt lesen:
  `unzip` den inneren `META-INF/versions/<v>/server-<v>.jar`, dann
  `javap -c -p net/minecraft/network/protocol/<state>/<Paket>.class`.
  Lesbarer geht es mit einem Decompiler (z. B. Vineflower), auch für den
  Client-Jar, der ebenfalls nicht obfuskiert ist. Dort steht, was der Client
  mit einem Paket macht (z. B. `LevelLoadTracker`: wann der Ladebildschirm
  verschwindet).
- **Protokollnummer**: `protocol_version` in `version.json` des Server-Jars
  (`PROTOCOL_VERSION` in `crates/protocol/src/lib.rs`).
- **Registries, Tags, Block-IDs** stehen in `data/<version>/` und kommen aus
  dem Datengenerator (`scripts/gen-reports.sh`). Nur die Liste der
  synchronisierten Registries (`SYNCED_REGISTRIES` in `gen-data.rs`) ist von
  Hand aus `RegistryDataLoader` übernommen und muss bei jeder Version geprüft
  werden.
- **Gegenprobe gegen Vanilla**: einen Vanilla-Server offline starten, den
  `proxy` davorhängen und mit `bot` Spieler hineinschicken. So sieht man, was
  Vanilla wirklich schickt; die Tests in `crates/protocol` vergleichen mit
  solchen Mitschnitten.
- **Weltdateien gegen Vanilla prüfen**: eine von Ferrocraft geschriebene
  Regionsdatei in eine Vanilla-Welt legen, Vanilla starten und per Konsole
  `forceload add …` und `execute if block X Y Z <zustand> run say OK` fragen.
  Das Log zeigt auch, wenn Vanilla Teile eines Chunks nicht lesen kann.
- **Online-Modus gegen Vanilla prüfen**: mit dem Test-Client
  (`Client::try_login`) bei einem Vanilla-Server im Online-Modus anmelden.
  Ohne Mojang-Konto antwortet Vanilla mit einer verschlüsselten
  `unverified_username`-Meldung; lässt sie sich lesen, passen RSA und
  AES/CFB8 zu Java.

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
- **BitSets sind Byte-Arrays** (`BitSet.toByteArray`, ohne Null-Bytes am
  Ende), nicht Long-Arrays wie in älteren Protokoll-Dokus. Betrifft die
  Licht-Masken.
- **Felder, die ältere Protokoll-Dokus nicht kennen**: `login_finished` trägt
  nach dem Profil eine Session-UUID, `login` (Play) ein `onlineMode`-Bool vor `enforcesSecureChat`,
  `accept_teleportation` die Position und Rotation nach dem Teleport.
- **Der Client liest seinen Spielmodus aus der Tab-Liste.** Ohne
  `player_info_update` für sich selbst ist `isCreative()` beim Client falsch.
- **Paletten-Container ohne Längenpräfix** (seit 1.21.5): die Zahl der Longs
  folgt aus der Bitbreite. Einträge gehen nie über eine Long-Grenze.
- **Chunks werden mit Puffer 2 geschickt**: `ChunkTrackingView.contains` zählt
  Nachbar-Chunks mit; bei Sichtweite 10 sind das 473 Chunks, nicht 385.
- **Welt-Layout in 26.3**: Chunks liegen in
  `dimensions/minecraft/overworld/region`, Spieler in `players/data`, nicht
  mehr in `region` und `playerdata` wie in älteren Versionen.
- **Blockzustände in Chunks (26.3)**: der Standardzustand eines Blocks ist
  nur sein Name (`"minecraft:stone"`), alle anderen `{id, properties}` in
  Kleinbuchstaben. Ältere Welten haben `{Name, Properties}`; gelesen wird
  beides.
- **NBT-Listen mit gemischten Typen**: Vanilla 26.3 schreibt sie als
  Liste von Compounds und packt jedes andere Element in `{"": wert}`. Eine
  Palette aus Namen und `{id, properties}` ist so eine Liste.
- **Bewegung anderer Entities**: `move_entity_*` tragen ein `VecDelta`; das
  Flag-`VarInt` davor enthält die Schrittzahl. Vanilla schickt für Spieler
  einen Schritt über zwei Ticks (`Stepped`), nicht `Linear`.
- **Blockaktionen bestätigen**: Nach `use_item_on` und `player_action` erst
  die echten Blockzustände schicken (auch für abgelehnte Aktionen), dann
  `block_changed_ack`. Sonst behält der Client seine Vorhersage.
- **Mojangs Endpunkte** stehen im Discovery-Dokument
  `https://discovery.minecraftservices.com/minecraft/client` (Authlib 10).
  `hasJoined` liegt weiterhin bei `sessionserver.mojang.com`.
- **`player_action`-IDs haben sich verschoben**: 26.3 hat
  `CHANGE_DESTROY_DIRECTION` = 1, dahinter ABORT = 2, STOP = 3, DROP_ALL = 4,
  DROP = 5, RELEASE_USE = 6, SWAP = 7.
- **Blockphysik kommt aus dem Jar, nicht aus den Reports**: Härte, Werkzeugpflicht,
  Licht, Kollisions- und Verdeckungsformen je Zustand liest
  `scripts/BlockPhysics.java` (läuft in `gen-reports.sh` gegen den
  Vanilla-Jar). Vanillas Datengenerator liefert das nicht.
- **RSA-Schlüssel mit 2048 Bit**: aws-lc-rs erzeugt keine 1024-Bit-Schlüssel
  wie Vanilla; der Client nimmt jede Größe. Der Test-Client verschlüsselt mit
  eigenem PKCS#1 (`devtools/src/rsa.rs`), weil aws-lc Vanillas 1024-Bit-Schlüssel
  ablehnt.
- **`set_time` trägt Uhren**: neben der Spielzeit eine Liste
  `(world_clock-ID, Stand)`; ohne die Overworld-Uhr steht die Sonne still.
- **Schildseite**: `open_sign_editor` und `sign_update` zählen 1 als Vorderseite.
- **Doppelkisten**: die erste Hälfte im Menü ist die mit `type=right`
  (`DoubleBlockCombiner`), nicht die angeklickte.
- **Respawn**: nach `respawn` wartet der Client wie beim Join auf
  `game_event` 13 (Chunks laden), sonst bleibt der Ladebildschirm stehen.
- **Kreativ-Slots prüfen die Stapelgröße**: mehr als `max_stack_size` (Schilder 16)
  lehnt der Server ab wie Vanilla.
- **Entity-Daten für Mobs** zählen über die Klassenhierarchie: `Entity` 0–7,
  `LivingEntity` 8–14 (Gesundheit 9), `Mob` 15, `AgeableMob` 16–17, danach die
  Art (Schafwolle 18). Nachzählen in den `defineSynchedData` der Klassen.
- **Mobs am Boden behalten −0,0784 als Fallgeschwindigkeit** (Schwerkraft mal
  Luftwiderstand). Wer sie auf 0 setzt, lässt `on_ground` jeden Tick wechseln,
  und der Tracker schickt dann Teleports statt Bewegungen.
- **Entity-Größen, Reichweiten und Attribute** liest `scripts/EntityInfo.java`
  aus dem Jar (`entities.json`); Loot-Tabellen heißen in `loot_tables.json`
  wie bei Vanilla `minecraft:blocks/…` und `minecraft:entities/…`.
