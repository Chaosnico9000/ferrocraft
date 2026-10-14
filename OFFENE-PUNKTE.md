# Offene Punkte

Was Ferrocraft noch fehlt, grob nach Reihenfolge. Was erledigt ist, steht im
`CHANGELOG.md`.

## Spielwelt

- Feindliche Mobs (Zombies, Skelette, Creeper, Spinnen) und weitere Tiere
- Tiere: speichern (sie verschwinden noch beim Neustart und weit weg von allen
  Spielern), Babys und Züchten, echte Wegfindung um Hindernisse, Gras fressen
  und Wolle nachwachsen lassen, Leinen, Reiten
- Erfahrungskugeln beim Töten
- Öfen, Räucher- und Schmelzöfen, Braustände (eigene Menüs mit Fortschritt)
- Redstone: Strom, Kolben, Beobachter, Verstärker, Komparatoren
- Wasser und Lava fließen, Schwerkraft für Sand und Kies
- Explosionen (TNT, Creeper)
- Wachsen: Pflanzen, Bäume, Gras und Blätter, die zerfallen
- Wetter: Regen, Gewitter
- Nether und Ende, Portale
- Andere Weltgeneratoren als Superflat

## Spielmechanik

- Verzauberungen, Amboss, Schleifstein; dadurch auch Behutsamkeit und Glück
  beim Abbauen
- Rüstung schützt (Rüstungswerte, Verzauberungen)
- Erfahrung: Kugeln, Level
- Schaden durch Feuer, Lava, Ertrinken, Kakteen; Schild, Bogen, Dreizack
- Statuseffekte, Tränke
- Schwierigkeitsgrade und Spielregeln (`gamerule`)

## Lücken im Vorhandenen

- Shulker-Kisten: der Inhalt fällt beim Abbauen lose heraus, statt in der Kiste
  zu bleiben (`copy_components` wird noch nicht ausgewertet)
- Andere Spieler hören keine Geräusche beim Blocksetzen
- Shift-Klick legt Rüstung im Inventar nicht in den Rüstungsslot
- Item-Komponenten werden nur durchgereicht, nicht verstanden (Namen,
  Verzauberungen, Inhalt)
- Komponenten werden in Spielerdateien in Netzwerkform gespeichert, nicht wie
  bei Vanilla als NBT
- Weitere Befehle: `/setblock`, `/fill`, `/summon`, `/effect`, `/weather`, …
- Bewegungen werden nicht geprüft (Fliegen, Durch-Wände-Laufen im
  Überlebensmodus)

## Noch mit einem echten Client zu prüfen

Die Tests laufen mit dem eigenen Test-Client und gegen Vanilla-Dateien; ein
echter Minecraft-Client ließ sich in der Entwicklungsumgebung nicht starten.

- Login im Online-Modus mit dem 2048-Bit-Schlüssel
- Respawn-Bildschirm und Rückkehr in die Welt
- Geräusche von Kisten, Türen, Knöpfen und Hebeln
- Essen (Animation, Ende nach 1,6 s)
- Tiere: Laufen, Kopfdrehen und Geräusche, wie sie im Client aussehen

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
