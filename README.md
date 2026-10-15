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
| Blöcke | Setzen und abbauen (Türen, Betten, Treppen, Doppelkisten), Türen/Falltüren/Zauntore/Hebel/Knöpfe benutzen, Licht wird neu berechnet; Nachbar- und Form-Updates wie Vanillas `setBlock` (Pflanzen und Fackeln ohne Halt fallen ab), geplante und zufällige Ticks, die einen Neustart überstehen | ✅ |
| Flüssigkeiten | Wasser und Lava fließen wie bei Vanilla (Stufen, Fallen, Suche nach dem nächsten Loch, neue Quellen zwischen zwei Wasserquellen), Lava und Wasser werden zu Obsidian, Bruchstein und Stein; Eimer schöpfen und gießen aus, Blöcke mit Wasser (`waterlogged`); Sand, Kies, Betonpulver und Ambosse fallen, Betonpulver härtet im Wasser | ✅ |
| Wachsen | Hacke macht Ackerboden (Schaufel Trampelpfade, Axt entrindet, alles aus Vanillas `block_transformer`-Daten), Weizen, Karotten, Kartoffeln, Rote Bete, Kürbis- und Melonenstiele wachsen nach Licht und Bewässerung, Ackerboden trocknet aus und wird zertrampelt; Setzlinge werden zu Bäumen nach Vanillas Baum-Features (Eiche, große Eiche, Birke, Fichte, Tropenbaum, Akazie, Kirsche, Schwarzeiche und Blasseiche 2×2), Laub ohne Stamm zerfällt, Gras breitet sich aus, Zuckerrohr und Kakteen wachsen; Knochenmehl | ✅ |
| Redstone | Hebel, Knöpfe, Druckplatten (auch Wägeplatten), Redstone-Fackeln (mit Ausbrennen), Redstone-Blöcke, Beobachter und Redstone-Truhen geben Signal; Leitungen mit Vanillas Auswertung (eine Stufe weniger pro Block, verbinden sich, Kreuz und Punkt per Rechtsklick), Verstärker mit Verzögerung und Sperre, Komparatoren (Vergleichen, Subtrahieren, lesen Behälter, Kuchen, Komposter, Kessel); Kolben und klebrige Kolben (bis zwölf Blöcke, Schleim und Honig nehmen Nachbarn mit, Quasi-Konnektivität); Lampen, Türen (auch aus Eisen), Falltüren, Zauntore und TNT reagieren | ✅ |
| Automatik | Trichter (aus Behältern darüber und liegende Items, in Behälter und Öfen nach Seite, Redstone hält sie an), Werfer (in Behälter oder hinaus), Spender (Pfeile, Eimer füllen und gießen, Feuerzeug, TNT, Knochenmehl) | ✅ |
| Feuer und Wetter | Feuer breitet sich nach Vanillas Brennbarkeit aus und frisst Blöcke, entsteht an Lava, Feuerzeug und Feuerkugel zünden Feuer, Lagerfeuer, Kerzen und TNT, brennende Pfeile (durch Feuer oder Lava geflogen) TNT; Regen und Gewitter im Wechsel wie bei Vanilla (`/weather`), Regen löscht Feuer, bewässert Ackerboden und füllt Kessel, Blitze zünden und laden Creeper auf | ✅ |
| Umweltschaden | Feuer, Lava, Brennen, Ertrinken (Luftvorrat), Kakteen, Magma und Blitze schaden Spielern und Mobs; Wasser und Regen löschen; Items verbrennen | ✅ |
| Block-Entities | Kisten, Fässer, Spender, Trichter, Shulker-Kisten, Endertruhe, Werkbank (Rezepte von Vanilla), Schilder beschreiben | ✅ |
| Öfen | Ofen, Räucherofen, Hochofen: Brennstoffe und Rezepte aus Vanillas Daten, Feuer und Fortschritt im Menü, der Block glüht, Erfahrung beim Herausnehmen | ✅ |
| Tiere | Kühe, Schweine, Schafe, Hühner: spawnen auf Gras, laufen mit Wegfindung umher, schauen Spieler an, folgen Futter, flüchten nach Treffern, Loot nach Vanillas Tabellen; züchten mit ihrem Futter, Babys wachsen heran und folgen den Eltern (Lämmer in gemischter Farbe); Schafe scheren, Kühe melken, Hühner legen Eier; werden gespeichert | ✅ |
| Monster | Zombies, Skelette (schießen Pfeile), Creeper (explodieren), Spinnen (klettern, springen): spawnen im Dunkeln wie bei Vanilla, jagen Spieler über Wegfindung, Zombies und Skelette brennen in der Sonne; Schwierigkeitsgrade | ✅ |
| Kampf | Nahkampf gegen Mobs und Spieler mit Aufladung, kritischen Treffern, Rückstoß, Waffenschaden aus Vanillas Daten; Bogen mit Spannen, kritischen Pfeilen und Aufsammeln; Rüstung schützt und nutzt sich ab | ✅ |
| Explosionen | Creeper und TNT (mit Feuerzeug oder Feuerkugel zünden, Kettenreaktionen): Blöcke nach Explosionsfestigkeit, Schaden und Rückstoß nach Deckung | ✅ |
| Erfahrung | Kugeln von Mobs und Öfen fliegen zum Spieler, Level wie bei Vanilla, beim Tod fällt ein Teil heraus | ✅ |
| Befehle | `/list`, `/gamemode`, `/time`, `/tp`, `/give`, `/kill`, `/summon`, `/difficulty`, `/experience` (`/xp`), `/setblock`, `/fill`, `/gamerule` (alle Regeln von Vanilla), `/weather` (alle außer `/list` für Operatoren); Server-Konsole mit `say`, `save-all` und `stop` | ✅ |
| Schutz | Bewegungsprüfung wie Vanilla: zu weite Sprünge und Bewegungen in Blöcke werden zurückgesetzt, Schweben im Überlebensmodus trennt die Verbindung (`allow_flight`) | ✅ |
| Welt | Speichern im Vanilla-Format samt `level.dat`, Spielregeln, Wetter und Entities (auch Pfeile, gezündetes TNT, fallende Blöcke), Vanilla-Welten laden | ✅ |
| Fehlt | Braustände, Verzauberungen, Armbrust, Schienen, Nether und Ende; alles in [`OFFENE-PUNKTE.md`](OFFENE-PUNKTE.md), auch die geplante Schicht für Fabric- und Forge-Mods | ⏳ |

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
operators = ["Chaosnico9000"] # dürfen alle Befehle außer /list nutzen und F3+F4
animals = true               # Kühe, Schweine, Schafe und Hühner spawnen bei Spielern
monsters = true              # Zombies, Skelette, Creeper und Spinnen spawnen im Dunkeln
difficulty = "easy"          # peaceful, easy, normal, hard (wie Vanilla)
allow_flight = false         # Schweben im Überlebensmodus erlauben (Flug-Mods)
```

Die Server-Konsole nimmt Befehle wie Vanillas: jede Zeile auf der
Standardeingabe ist ein Befehl ohne `/`, ausgeführt mit Operator-Rechten am
Weltspawn; die Antwort steht auf Englisch im Log (aus Vanillas `en_us.json`).
`say <Text>` schreibt allen, `save-all` speichert, `stop` beendet den Server.
Ohne Eingabe (`< /dev/null`) läuft der Server einfach weiter.

## Die Welt

Gespeichert wird im Format und Ordner-Layout von Vanilla 26.3: Chunks samt
Kisteninhalten, Öfen und Schildtexten in
`world/dimensions/minecraft/overworld/region/r.X.Z.mca`, Tiere, Monster,
liegende Items und Erfahrungskugeln in
`world/dimensions/minecraft/overworld/entities/r.X.Z.mca`, Spieler (Position,
Blickrichtung, Inventar, Endertruhe, Leben, Erfahrung, Spielmodus, Brennen,
Luft) in `world/players/data/<uuid>.dat`, Spawn und Weltzeit in `level.dat`
und `data/minecraft/world_clocks.dat`, Spielregeln in
`data/minecraft/game_rules.dat`, das Wetter in `data/minecraft/weather.dat`. Ferrocraft schreibt
nur Chunks, in denen sich etwas geändert hat, alle 5 Minuten, beim Verlassen
eines Spielers und beim Beenden (Strg+C oder SIGTERM). Entities in Chunks,
denen kein Spieler näher als 10 Chunks ist, ruhen wie bei Vanilla und kommen
zurück, wenn jemand auf 8 Chunks herankommt.

Kolben in Bewegung, Komparatoren und Trichter behalten ihren Zustand als
Block-Entity wie bei Vanilla. Geplante Ticks (fließendes Wasser, Verstärker,
Knöpfe, Feuer) stehen wie bei
Vanilla als `block_ticks` und `fluid_ticks` im Chunk und laufen nach dem
Laden weiter, bei Ferrocraft wie bei Vanilla.

Vanilla 26.3 öffnet so eine Welt ohne Fehler (geprüft mit `data get block`
für Kisten, Schilder und Öfen und `data get entity` für alle Mob-Arten), und
Ferrocraft lädt von Vanilla geschriebene Welten samt Biomen, gespeichertem
Licht und Entities. Was Ferrocraft nicht kennt (andere Entities wie Pferde,
fremde Block-Entities, Strukturen), bleibt beim Speichern erhalten. Chunks,
die Vanilla nur angefangen hat zu erzeugen, lässt Ferrocraft aus und erzeugt
sie selbst.

Log-Level über `RUST_LOG`, z. B. `RUST_LOG=debug cargo run`. Tick-Zeit und
die Zahl der Mobs erscheinen alle 30 s mit `RUST_LOG=info,ferrocraft::game=debug`.

## Werkzeuge

```sh
# Mitschnitt: jedes Paket zwischen Client und Server mit Zustand und Namen,
# komprimierte Pakete entpackt. Der Server dahinter muss offline laufen.
cargo run -p ferrocraft-devtools --bin proxy -- 127.0.0.1:25570 127.0.0.1:25565 --log pakete.jsonl

# Bots (nur Offline-Server, zum Bauen im Kreativmodus). Rollen reihum:
# walk (läuft, springt, schleicht), build (baut Türme und reißt sie ab),
# house (Kiste, Tür, Hebel), throw (wirft Items und sammelt sie ein),
# idle (steht still)
cargo run -p ferrocraft-devtools --bin bot -- 127.0.0.1:25565 --count 8 --radius 4
cargo run -p ferrocraft-devtools --bin bot -- 127.0.0.1:25565 --roles build,walk
# Befehle ausführen (der Bot muss Operator sein), die Antworten erscheinen:
cargo run -p ferrocraft-devtools --bin bot -- 127.0.0.1:25565 --name Admin \
  --roles idle --seconds 3 --commands "time set midnight;summon zombie ~5 ~ ~"

# Zwei Welten Block für Block vergleichen, z. B. eine Vanilla-Welt und eine
# Ferrocraft-Welt nach denselben /setblock- und /fill-Befehlen:
cargo run --release --example compare_worlds -- vanilla/world ferro/world 0 -62 0 47 -55 47

# Daten für eine Version neu erzeugen (Paket-IDs, Registries, Tags, Blöcke,
# Items, Brennstoffe, Rezepte, Loot-Tabellen, Baum-Features, Werkzeug-
# Umwandlungen). Braucht Java: Mojangs Datengenerator und
# scripts/BlockPhysics.java (Härte, Explosionsfestigkeit, Licht, Kollisions-
# und Umrissformen, Spawnflächen, Redstone-Leiter, feste Seiten, Kolben-
# Verhalten, Komparator-Lesbarkeit, Brennbarkeit, Klassen je Block)
# scripts/EntityInfo.java (Größe, Reichweite, Attribute je Entity-Typ) laufen
# gegen den Vanilla-Jar. Der Server selbst bleibt reines Rust.
scripts/gen-reports.sh 26.3
```

## Aufbau

```
crates/protocol   Wire-Typen (VarInt, Strings, UUID, BlockPos), NBT, Chunk-Format,
                  Framing mit Kompression, Verschlüsselung, Pakete je Zustand
crates/server     Netzwerk (tokio, ein Task pro Verbindung), Status, Login und
                  Mojang-Auth (RSA über aws-lc-rs), Configuration, Play,
                  game/ (Tick-Thread: Spieler, Menüs, Überleben, Items,
                  Mobs mit Wegfindung, Pfeile, Explosionen, Erfahrung, Öfen,
                  Befehle, Konsole; blocks: Block-Updates, Ticks und
                  Block-Events, fluid, falling, growth, redstone, piston,
                  hopper, dispenser, fire, weather, hazards, movement),
                  Spielregeln, Welt, Licht, Block-Entities, Rezepte, Loot,
                  Bäume (tree.rs), Speichern
crates/devtools   Test-Client, gen-data (Daten aus dem Datengenerator),
                  proxy (Mitschnitt), bot
data/26.3         Aus Mojangs Datengenerator: packets.json (Paket-IDs),
                  registries.json, tags.json, blocks.json (mit Physik),
                  items.json, entities.json, recipes.json, loot_tables.json,
                  features.json (Bäume, Werkzeug-Umwandlungen), ids.json,
                  en_us.json (englische Texte für die Konsole)
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
