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
- **Umbenennen allein ist nicht absturzsicher**: ohne `fsync` vor dem
  `rename` kann nach einem harten Neustart (WSL!) eine leere Datei unter dem
  richtigen Namen liegen. `write_atomically` synchronisiert und behält
  `…_old`.
- **Entities liegen getrennt von den Chunks**: in
  `dimensions/minecraft/overworld/entities/r.X.Z.mca`, je Chunk
  `{DataVersion, Position: [I; x, z], Entities: [...]}` (`EntityStorage`).
  Vanilla speichert dort auch Entities, die Ferrocraft nicht kennt; sie
  müssen beim Speichern erhalten bleiben.
- **Vanilla speichert halbfertige Chunks**: um den erkundeten Bereich liegen
  viele Chunks mit `Status` wie `minecraft:structure_starts`, ohne Blöcke. Nur
  `minecraft:full` ist ein fertiger Chunk.
- **Brennstoffe sind Daten, kein Code**: die Item-Komponente `cooking_fuel`
  verweist auf `context_int_provider/cooking/*` (Brennzeit) und
  `context_float_provider/cooking/*` (Tempo); die Bedingung
  `block/fast_cooking` macht Räucher- und Hochöfen doppelt so schnell und
  halbiert die Brennzeit. `gen-data` rechnet beides aus.
- **Tageslicht kommt aus `timeline/day.json`**: `gameplay/sky_light_level`
  (15 mal Faktor, nachts 4) und `gameplay/monsters_burn`; `skyDarken` ist
  `(int)(15 - Wert)`. Monster spawnen bei Himmelslicht über `random(32)` nie.
- **Entity-Daten der Monster**: `Mob` 15 (0x04 Arme hoch), Zombie 16 (Baby),
  Creeper 16 (Zündrichtung, Int) und 18 (gezündet), Spinne 16 (klettert),
  Pfeil 10 (steckt), Erfahrungskugel 8 (Wert). Das Extra-Int von
  `add_entity` ist beim Pfeil der Schütze.
- **`explode`** trägt Partikel als Registry-ID ohne Optionen, den Sound als
  `id + 1` und eine gewichtete Liste von Block-Partikeln; den Rückstoß bekommt
  nur der Spieler, den es betrifft.
- **Schaden skaliert nur manchmal mit dem Schwierigkeitsgrad**: Explosionen
  immer, sonst nur, wenn ein Mob schuld ist (`scaling` der `damage_type`s).
- **Tests mit mehreren Entities** dürfen sich nicht auf die Reihenfolge der
  `add_entity` verlassen, und Werte, die im selben Tick kommen
  (`container_set_data`), kommen in Index-Reihenfolge.
- **Der Test-Client überspringt, was er nicht dekodieren kann**: Items mit
  Komponenten wie `damage` scheitern beim Dekodieren, und `read_until`
  wartet dann still weiter, bis der Keep-Alive die Verbindung beendet (nach
  30 s). Solche Pakete roh lesen und nur die vorderen Felder prüfen.
- **Bogen**: Der Client meldet das Loslassen als `player_action`
  RELEASE_USE (6); die Spannzeit zählt der Server selbst. Gezündetes TNT
  trägt die Lunte als Entity-Daten 8 (Int), der Client zählt sie herunter.
- **Block-Updates laufen wie Vanillas `setBlock`**: erst `onPlace`, dann
  `neighborChanged` an die sechs Nachbarn (über einen Stapel wie
  `CollectingNeighborUpdater`, nicht rekursiv), dann `updateShape` mit Tiefe
  höchstens 512. `onPlace` kommt bei *jeder* Zustandsänderung, nicht nur bei
  neuem Block; Verstärker melden darüber ihre Ausgabe. Flags wie bei
  Vanilla (`update::NEIGHBORS` 1, `CLIENTS` 2, `INVISIBLE` 4, `KNOWN_SHAPE`
  16, `SUPPRESS_DROPS` 32, `MOVE_BY_PISTON` 64,
  `SKIP_BLOCK_ENTITY_SIDEEFFECTS` 256). Ohne `CLIENTS` bekommen die Clients
  die Änderung **nicht**: Kolben setzen ihre `moving_piston`-Blöcke so, und
  der Client bewegt die Blöcke selbst aus dem Block-Event. Schickt man ihm
  die Zwischenzustände doch, findet er beim Block-Event schon
  `moving_piston` vor und die Animation fällt aus.
- **Block-Events laufen am Ende des Block-Teils eines Ticks**
  (`runBlockEvents`, nach Zufalls-Ticks und Regen), doppelte werden
  verworfen; nur wenn das Event etwas tat, geht `block_event` an die
  Clients. `handling_tick` entspricht `ServerLevel.isHandlingTick` (Kolben
  entscheiden damit zwischen Einfahren und Fallenlassen).
- **Landende Blöcke kommen in Positionsreihenfolge**: Block-Entities ticken
  nach Position sortiert, ein Kolbenkopf kann also vor dem geschobenen Block
  ankommen. Tests auf mehrere Blöcke gemeinsam warten lassen
  (`all_become` in `tests/pistons.rs`).
- **Schleim am Boden klebt am Boden**: Gras und Erde sind schiebbar, ein
  Schleimblock auf der Grasschicht will die ganze Reihe mitnehmen und der
  Kolben fährt nicht aus (bei Vanilla genauso). Testaufbauten mit Schleim
  in die Luft setzen.
- **Komparatoren hören über `container_changed`**: Wer Behälterinhalte
  ändert (Menü-Klicks, Trichter, Werfer, Öfen), ruft es auf
  (`updateNeighbourForOutputSignal`); `set_block` mit `NEIGHBORS` tut es für
  Blöcke mit `flags::ANALOG` selbst.
- **Verhalten nach Vanillas Klasse**: `blocks.json` trägt je Block die
  Klassenkette (`CropBlock`, `FallingBlock`, …), `Kind::of` wählt daraus.
  Die Flags je Zustand (fest, Zufalls-Ticks, Redstone-Leiter, ersetzbar,
  feste Seiten, Signalquelle) und die Umrissform liest
  `scripts/BlockPhysics.java`.
- **Richtungen im Redstone**: `getSignal(pos, dir)` bekommt die Richtung *vom
  Fragenden zum Sender*. Eine Fackel gibt dem Block unter sich nichts
  (`dir == UP`), stark nur dem Block darüber (`getDirectSignal`, `DOWN`). Ein
  Verstärker nimmt sein Signal von `pos + facing` und gibt es nach
  `pos - facing` ab.
- **Lava prüft beim Erstarren oben und die vier Seiten, nicht unten**
  (`POSSIBLE_FLOW_DIRECTIONS` gegenläufig); Lava, die auf Wasser fließt,
  macht das Wasser zu Stein.
- **`/setblock` und `/fill` formen den Block nach seinen Nachbarn**
  (`Block.updateFromNeighbourShapes`) und lassen nur die genannten
  Eigenschaften stehen; Leitungen bekommen so ihre Verbindungen. Danach
  `updateNeighboursOnBlockSet`.
- **Beute vor dem Block-Update**: `destroyBlock` lässt erst fallen, dann
  setzt es Luft. Tests, die beides lesen, dürfen die Reihenfolge nicht
  umdrehen. Wanduhr-Zeiten in Tests sind unzuverlässig, weil der Tick-Thread
  versäumte Ticks am Stück nachholt; lieber die Reihenfolge prüfen.
- **Gegenprobe Block für Block**: dieselben Befehle auf einem Vanilla- und
  einem Ferrocraft-Server, Vanilla mit `save-all flush`, Ferrocraft per
  SIGTERM speichern, dann `cargo run --release --example compare_worlds`.
  Unterschiede bei Gras/Erde sind Zufalls-Ticks. Beide Konsolen lesen
  Befehle von der Standardeingabe (`tail -F console.in | ferrocraft`);
  Zeilen, die zusammen kommen, laufen wie bei Vanilla im selben Tick.
- **Die Konsole liest auf einem eigenen Thread**: ein blockierendes
  `stdin`-Lesen in tokios Pool hält beim Beenden die Laufzeit und damit den
  Prozess fest (SIGTERM endete nie). Antworten werden mit Vanillas
  `en_us.json` (aus dem Server-Jar, `data/<version>/en_us.json`) gerendert.
- **Die Bewegungsprüfung gilt auch in Tests**: mehr als zehn Blöcke pro
  Tick werden zurückgesetzt, Bewegungen in Blöcke auch im Kreativmodus.
  Weite Wege mit `walk_steps` gehen (acht Blöcke pro Tick) oder per
  `/gamerule player_movement_check false`.
- **Spielregeln und Wetter liegen nicht in `level.dat`**: 26.3 speichert
  sie als `data/minecraft/game_rules.dat` (Schlüssel `minecraft:<regel>`,
  Byte oder Int) und `data/minecraft/weather.dat`.
- **Blockdaten für Kolben, Komparatoren und Feuer**: `scripts/BlockPhysics.java`
  schreibt die `PushReaction` (3 Bit ab Bit 12, Reihenfolge der Enum:
  push_pull, push, popped, immovable, ignore), `hasAnalogOutputSignal`
  (Bit 15), `ignitedByLava` (Bit 16) und je Block `fire` (Ignite- und
  Burn-Odds aus den privaten Maps von `FireBlock`). Der Regen hört an
  `#blocks_motion_in_heightmap` auf, das ist ein Tag, kein Flag.
