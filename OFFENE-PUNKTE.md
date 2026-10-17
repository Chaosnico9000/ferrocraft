# Offene Punkte

Was Ferrocraft noch fehlt, grob nach Reihenfolge. Was erledigt ist, steht im
`CHANGELOG.md`.

## Spielwelt

- Weitere Mobs: Endermen, Hexen, Schleime, Zombiedorfbewohner, Pferde,
  Dorfbewohner (ihre Spawn-Anteile sind schon eingeplant, es spawnt nur nichts)
- Tiere: Gras fressen und Wolle nachwachsen lassen, Leinen, Reiten, Varianten
  (warme und kalte Kühe, Schweine, Hühner), Küken aus geworfenen Eiern
- Monster: Zombies brechen keine Türen auf und rufen keine Verstärkung, Skelette
  suchen keinen Schatten und laufen beim Schießen nicht seitwärts, keine
  Spinnenreiter, Mobs greifen sich nicht gegenseitig an, keine Rüstung und
  Waffen beim Spawnen
- Mobs schwimmen nicht (sie meiden Wasser nur), Leitern nutzen nur Spinnen;
  Strömung schiebt weder Mobs noch Items, Items treiben nicht auf
- Trichter lassen Komposter aus, Trichterloren fehlen
- Redstone: Tageslichtsensoren, Zielblöcke, Stolperdrähte, Schienen,
  Notenblöcke, Kupferbirnen schalten, Crafter; Komparatoren lesen keine
  Rahmen, Jukeboxen, Lesepulte, Bienenstöcke und Bücherregale (geben 0)
- Spender: Feuerkugeln, Schneebälle, Eier, Wurftränke und Raketen fliegen
  nicht (sie fallen als Item heraus), keine Rüstung anlegen, Scheren, Boote,
  Loren, Kürbisse, Glasflaschen; Kolben schieben Spieler nur auf deren
  Client (Server-seitig Mobs, Items, TNT), Schleim lässt nichts springen
- Blitzableiter, Skelettpferde-Fallen, verwandelte Schweine und
  Dorfbewohner bei Blitzschlag
- Wachsen: Riesenbäume aus vier Setzlingen (Fichte, Tropenbaum), Mangroven,
  Azaleen, Bienennester an Bäumen; Bambus, Ranken, Kelp, Seetang,
  Pilze, Beerensträucher, Kakao; Doppelpflanzen ohne Boden fallen nicht ab
- Wasser: Fischeimer setzen keine Fische aus, Blasensäulen, Eis schmilzt
  nicht, Schnee fällt nicht
- Wetter: Schnee und Eis in kalten Biomen (Ferrocraft kennt nur die
  Ebene); Schlafen überspringt die Nacht und das Gewitter nicht
- Nether und Ende, Portale
- Andere Weltgeneratoren als Superflat

## Spielmechanik

- Verzauberungen, die an Fehlendem hängen: Frostläufer (`location_changed`
  ersetzt Wasser durch Eis), Seelenläufer (`tick`), Dreizack (Treue,
  Sog, Kanalisierung, Harpune), Armbrust (Mehrfachschuss, Durchschuss,
  Schnellladen), Angel (Glück des Meeres, Köder), Streitkolben (Dichte,
  Bresche, Windstoß); Verzauberungen auf Ausrüstung von Mobs, verzauberte
  Beute (`enchant_randomly`, `enchant_with_levels`)
- Effekte mit eigenem Verhalten: Böses Omen, Überfall-Omen,
  Prüfungs-Omen, Held des Dorfes, Befall, Schleimen, Weben, Windgeladen
  (Wirkung beim Tod); Leuchtfeuer und Aquisitor geben keine Effekte
- Ersticken in Blöcken, Erfrieren im Pulverschnee, Beerensträucher;
  Schild, Armbrust, Dreizack
- Totem der Unsterblichkeit (`death_protection`)
- Spielregeln, denen Ferrocraft noch nicht folgt (sie werden gesetzt und
  gespeichert): `entity_drops`, `log_admin_commands`, `max_entity_cramming`,
  `players_sleeping_percentage`, `spawner_blocks_work`, `limited_crafting`
  und die zu Dingen, die es noch nicht gibt (Portale, Patrouillen,
  Phantome, Wächter, Raids)

## Lücken im Vorhandenen

- Shulker-Kisten: der Inhalt fällt beim Abbauen lose heraus, statt in der Kiste
  zu bleiben (`copy_components` wird noch nicht ausgewertet)
- Andere Spieler hören keine Geräusche beim Blocksetzen
- Shift-Klick legt Rüstung im Inventar nicht in den Rüstungsslot
- Komponenten ohne Beschreibung in `component.rs` (Bündel- und
  Behälterinhalt, Banner, Feuerwerk, Bücher, Rüstungsbesatz, Köpfe,
  Ausrüstbarkeit, Werkzeug- und Essensdaten, Attribut-Modifikatoren am
  Stack, …) werden nur durchgereicht und in Ferrocrafts eigener Form
  gespeichert; aus Vanilla-Dateien gehen sie beim Laden verloren
- Weitere Befehle: `/data`, `/clone`, `/item`, …;
  `/summon` ohne NBT, Ziele nur `@s`, `@a`, `@p`, `@r`, `@e` und Namen
- Die Konsole meldet Befehle von Operatoren nicht wie Vanilla an andere
  Operatoren weiter (`log_admin_commands`); kein `op`/`deop`, `whitelist`,
  `ban` (Operatoren stehen in der Konfiguration)
- Pfeile stecken nicht sichtbar in Spielern; Spielerbewegung geht nicht in
  den Schwung der Pfeile und Wurftränke ein
- Mobs nutzen Unsichtbarkeit nicht (sie sehen unsichtbare Spieler wie
  sichtbare)
- Die Bewegungsprüfung kennt keine Fahrzeuge, Elytren und Schwimmen; dass
  ein Client mehr als eine Position pro Client-Tick schickt, wird nicht
  geprüft (`ClientTickEnd`)

## Noch mit einem echten Client zu prüfen

Die Tests laufen mit dem eigenen Test-Client und gegen Vanilla-Dateien; ein
echter Minecraft-Client ließ sich in der Entwicklungsumgebung nicht starten.

- Respawn-Bildschirm und Rückkehr in die Welt
- Geräusche von Kisten, Türen, Knöpfen und Hebeln
- Essen (Animation, Ende nach 1,6 s)
- Tiere: Laufen, Kopfdrehen und Geräusche, wie sie im Client aussehen
- Monster: erhobene Arme der Zombies, Bogenspannen der Skelette, Anschwellen
  und Explosion der Creeper, klettern der Spinnen; Feuer auf brennenden
  Zombies; Pfeile im Flug
- Ofenmenü: Flamme und Pfeil laufen mit, Erfahrungskugeln fliegen zum Spieler
- Bogen: Spannen und Loslassen, Pfeile im Flug und beim Aufsammeln; TNT
  blinkt und explodiert; Herzen beim Füttern, Jungtiere in halber Größe
- Wasser und Lava fließen sichtbar, Eimer füllen und leeren (Geräusch,
  Animation); fallender Sand; Knochenmehl-Partikel; Bäume wachsen;
  Redstone-Leitungen leuchten nach Stärke, Verstärker und Lampen schalten
- Effekte: Symbole und Zeiten im Inventar, Partikel um Spieler und Mobs,
  goldene Herzen, schnelleres Laufen mit Tempo, Leuchten durch Wände;
  Tränke trinken, Wurftrank-Spritzer und Verweilwolke; Braustand mit
  laufendem Fortschritt; Verzauberungstisch mit Hinweisen und
  Galaktisch-Schrift, Amboss mit Namensfeld und „Zu teuer!“, Schleifstein;
  Glitzern verzauberter Items, Funken bei Schärfe-Treffern
- Kolben fahren flüssig aus und ein (Block-Event), Schleim nimmt Blöcke mit;
  Beobachter blinken, Komparatoren leuchten; Trichter und Spender (Pfeile,
  Wasser); Feuer flackert und breitet sich aus, Rauch; Regen und Gewitter,
  Blitze mit Donner; Flammen auf brennenden Spielern, Luftblasen unter
  Wasser; Zurücksetzen bei zu weiten Sprüngen

## Nach Fertigstellung: Mod-Schicht für Fabric und Forge

Ziel: Mods für Fabric und Forge/NeoForge sollen auf Ferrocraft laufen.

**Das Problem dabei:** Diese Mods sind Java und für Mojangs eigenen Server
geschrieben. Sie rufen dessen Klassen direkt auf (`net.minecraft.*`), und viele
verändern sie zur Laufzeit: Fabric mit Mixins, die den Bytecode einzelner
Vanilla-Methoden umschreiben, Forge/NeoForge mit gepatchten Klassen und einem
Event-Bus. Ferrocraft hat diese Klassen nicht, weil es in Rust geschrieben ist.
Eine Schicht muss also so tun, als gäbe es sie. Das geht nur stufenweise, und
nicht jede Mod wird laufen.

Stufen, vom Machbaren zum Schweren:

1. **Datapacks laden.** Rezepte, Loot-Tabellen, Tags, Weltgenerierung,
   Funktionen. Ferrocraft liest diese Formate für Vanilla schon; viele
   „Mods“ sind in Wahrheit Datapacks. Das ist die Grundlage für alles Weitere.
2. **Modded Clients hereinlassen.** Die Handshakes der Loader im
   Configuration-Zustand nachbauen: Fabric API (Kanalregistrierung,
   Registry-Sync) und NeoForge/Forge (Netzwerk-Aushandlung, Registry-Sync).
   Dann können Spieler mit Mods beitreten, und Mods, die nur auf dem Client
   etwas tun, funktionieren.
3. **Eigene Inhalte über die Registries.** Neue Blöcke, Items und Entities, die
   eine Mod anmeldet, in Ferrocrafts Daten aufnehmen und an Clients mit der
   Mod schicken (Registry-Sync statt fester Vanilla-IDs).
4. **Java-Mods ausführen.** Eine JVM in den Server einbetten (JNI, z. B. mit
   dem `jni`-Crate) und darin Fabric Loader bzw. den NeoForge-Loader laufen
   lassen, mit einer Fassade der `net.minecraft`-Klassen, die auf Ferrocrafts
   Rust-Zustand zeigt (ähnlich wie Glowstone die Bukkit-API umgesetzt hat).
   Die Fassade muss Mojangs Klassen und Methoden mit gleichen Namen und
   gleichem Verhalten nachbilden, damit Aufrufe und Mixins ihr Ziel finden.
   Seit 26.1 ist Vanilla nicht mehr obfuskiert, die Namen sind also bekannt.
   Der Tick-Thread bliebe in Rust; Mods würden über Events und die Fassade
   eingreifen.
5. **Nach Beliebtheit abdecken.** Die verbreitetsten Mods einzeln testen
   (Fabric API, Architectury, Mods mit vielen Mixins) und die Fassade dort
   ausbauen, wo sie scheitern. Mixins in Klassen, die es in Ferrocraft gar nicht
   gibt (Rendering, Weltgeneratoren von Vanilla), bleiben die Grenze.

Unabhängig davon wäre eine eigene Plugin-Schnittstelle in Rust oder WASM
denkbar, für Erweiterungen, die nicht aus der Java-Welt kommen.
