# Changelog

Alle nennenswerten Änderungen an Ferrocraft. Format nach
[Keep a Changelog](https://keepachangelog.com/de/1.1.0/).

## [Unreleased]

### Neu

- Item-Komponenten: Ferrocraft beschreibt Vanillas Komponenten
  (`DataComponents`) einmal als Form, aus der Netzwerkform und
  Speicherform folgen: Haltbarkeit, Verzauberungen und gespeicherte
  Verzauberungen, Namen, Beschreibungen, Seltenheit, Reparaturkosten,
  Tränke mit eigenen Effekten und Farben, Schmorgericht-Effekte,
  Färbung, Karten-ID, eigene Daten (`custom_data`) und mehr. Items werden
  wie bei Vanilla gespeichert (`components: {"minecraft:damage": 10, …}`,
  entfernte als `"!name": {}`), Vanilla-Dateien werden gelesen. Gegen einen
  Vanilla-Server geprüft: acht Items mit vielen Komponenten werden aus
  Vanillas Netzwerkform gelesen und genau so gespeichert, wie Vanilla sie
  speichert (`cargo run --example vanilla_items`).
- `/give` mit Komponenten (`diamond_sword[enchantments={sharpness:5},damage=10]`,
  `!komponente` zum Entfernen) über einen SNBT-Parser.
- Statuseffekte nach `MobEffectInstance` und `LivingEntity`: alle 40
  Effekte von Vanilla mit Kategorie, Farbe und Attribut-Modifikatoren
  (ausgelesen mit `scripts/Alchemy.java`), Ablaufen, versteckte schwächere
  Effekte, die nach einem stärkeren weiterlaufen, Regeneration, Gift (tötet
  nicht), Verdorrung, Hunger, Sättigung, Sofortheilung und -schaden
  (Untote umgekehrt), Absorption (goldene Herzen), Gesundheitsschub,
  Feuerresistenz, Wasseratmung, Resistenz, Eile und Abbaulähmung beim
  Abbauen, Sprungkraft und sanfter Fall beim Fallschaden, Schweben ohne
  Rauswurf, Unsichtbarkeit und Leuchten als Flags, Partikel für alle.
  Effekte gelten für Spieler und Mobs, werden gespeichert, der Client hört
  `update_mob_effect` und `remove_mob_effect`. `/effect give|clear` mit
  Vanillas Antworten.
- Attribute nach `AttributeInstance`: Grundwerte (Spieler aus dem Jar,
  Mobs aus ihren Typen), Modifikatoren aus Ausrüstung
  (`attribute_modifiers` nach Slot), Verzauberungen und Effekten, Vanillas
  Formel; Clients bekommen `update_attributes` für alles, was sie selbst
  rechnen (Tempo, Abbau, Sprung, Fallhöhe …). Angriffsschaden,
  Angriffstempo, Rüstung, Härte, Rückstoßresistenz, Maximalgesundheit und
  Mob-Tempo laufen jetzt darüber.
- Essen und Trinken über die `consumable`-Komponente: Tränke, Milch (heilt
  alles), Honigflasche (heilt Gift), goldene und verzauberte goldene Äpfel,
  verdorbenes Fleisch, Spinnenaugen, Kugelfisch, Chorusfrüchte
  (teleportieren), Schmorgerichte und unheilvolle Flaschen, mit ihrem
  Rest (Flasche, Eimer, Schüssel) aus `use_remainder`.
- Wurf- und Verweiltränke: fliegen wie Schneebälle, Wurftränke wirken im
  Umkreis von vier Blöcken nach Nähe, Verweiltränke hinterlassen eine Wolke
  (`AreaEffectCloud`), die schrumpft und ein Viertel der Wirkung gibt.
  Wasser löscht Feuer, Kerzen, Lagerfeuer und Brennende. Beide werden
  gespeichert.
- Getränkte Pfeile (ein Achtel der Wirkung) und Spektralpfeile (Leuchten);
  Pfeile behalten ihr Item und geben es beim Aufsammeln zurück.
- Braustand nach `BrewingStandBlockEntity`: Lohenstaub für zwanzig Brauvorgänge,
  20 Sekunden je Brauvorgang, alle 279 Brau-Rezepte aus Vanillas Daten
  (26.3 hat sie als `recipe/brewing`), Menü mit Fortschritt und
  Brennstoff, der Block zeigt die Flaschen, Trichter füllen Reagenz von
  oben, Flaschen und Brennstoff von der Seite und nehmen unten Flaschen.
- Verzauberungen aus ihren Daten (`enchantment/*.json`) ausgewertet:
  Effekt-Komponenten wie `minecraft:damage`, `damage_protection`,
  `post_attack`, `attributes`, `item_damage`, `knockback`, `ammo_use`,
  `block_experience`, `repair_with_xp`, `projectile_spawned`,
  `damage_immunity`, `prevent_equipment_drop` und
  `prevent_armor_change`, mit ihren Bedingungen (Ziel-Typ, Schadensart,
  Werkzeug, Zufall nach Stufe) und stufenabhängigen Werten. Damit wirken
  Schärfe, Bann, Nemesis der Gliederfüßer, alle Schutz-Arten, Haltbarkeit,
  Effizienz, Wasseraffinität, Atmung, Schwungkraft, Verbrennung,
  Rückstoß, Stärke, Schlag, Flamme, Unendlichkeit, Reparatur, Dornen,
  Fluch der Bindung und des Verschwindens. In Loot-Tabellen: Behutsamkeit,
  Glück (`apply_bonus` mit allen Formeln, `table_bonus`) und Plünderung
  (`enchanted_count_increase`, `random_chance_with_enchanted_bonus`).
- Verzauberungstisch nach `EnchantmentMenu`: Bücherregale mit Luft
  dazwischen, Kosten, Hinweise, Lapis und Stufen, mit Javas
  Zufallsgenerator (`LegacyRandomSource`) aus dem Verzauberungs-Seed des
  Spielers. Gegen Vanilla geprüft: Für Schwert, Buch, Bogen, Goldhelm,
  Spitzhacke und Angel bietet der Tisch bei gleichem Seed genau dasselbe an.
- Amboss nach `AnvilMenu`: Kombinieren (Stufen erhöhen, unverträgliche
  Verzauberungen kosten), Reparieren mit Material und zweitem Gegenstand,
  Umbenennen (`rename_item`), steigende Reparaturkosten, „Zu teuer!“ ab 40,
  der Amboss nutzt sich ab und zerbricht.
- Schleifstein nach `GrindstoneMenu`: nimmt Verzauberungen außer Flüchen,
  verbindet zwei gleiche Gegenstände, gibt Erfahrung.
- `/enchant` mit Vanillas Prüfungen und Antworten.
- Erfahrung aus Erzen und anderen Blöcken (`DropExperienceBlock`, Redstone-
  Erz, Sculk, Spawner); Behutsamkeit verhindert sie.
- Spielregeln, die bisher nur gespeichert wurden, wirken jetzt:
  `keep_inventory`, `natural_health_regeneration`, `show_death_messages`,
  `immediate_respawn` und `reduced_debug_info` (an die Clients),
  `pvp`, `tnt_explodes` (mit Vanillas Hinweis), `block_drops`,
  `mob_drops`, `spawn_mobs`, `spawn_monsters`, `send_command_feedback`,
  `mob_explosion_drop_decay`, `tnt_explosion_drop_decay`.

- Kolben und klebrige Kolben nach Vanillas `PistonBaseBlock` und
  `PistonStructureResolver`: Strom von jeder Seite außer vorn oder in den
  Block darüber (Quasi-Konnektivität), bis zu zwölf Blöcke, Schleim und Honig
  nehmen ihre Nachbarn mit (aber nicht einander), was zerbricht
  (Fackeln, Laub) fällt als Beute ab, Obsidian und Blöcke mit Block-Entity
  bleiben stehen. Die Bewegung läuft als Block-Event am Ende des Ticks; die
  bewegten Blöcke stecken zwei Ticks in `moving_piston`-Blöcken
  (`PistonMovingBlockEntity`) und landen danach. Clients sehen die Bewegung
  aus dem Block-Event, wie bei Vanilla bekommen sie die Zwischenzustände
  nicht. Kurze Pulse lassen klebrige Kolben ihren Block zurücklassen. Mobs,
  Items und gezündetes TNT werden mitgeschoben.
- Beobachter: eine Änderung vor ihnen gibt einen Puls von zwei Ticks nach
  hinten.
- Komparatoren: Vergleichen und Subtrahieren mit Seiteneingang, Rechtsklick
  wechselt; sie lesen Kisten (auch doppelte), Fässer, Spender, Werfer,
  Trichter, Öfen und Shulker-Kisten nach Füllstand, dazu Kuchen, Komposter,
  Kessel, Endportalrahmen, Seelenanker und Kupferbirnen, auch durch einen
  festen Block hindurch. Behälter melden Änderungen wie Vanillas
  `updateNeighbourForOutputSignal`.
- Redstone-Truhen geben so viel Signal, wie Spieler hineinschauen.
- Trichter (`HopperBlockEntity`): alle 8 Ticks ein Item in den Behälter, auf
  den sie zeigen, und eines aus dem Behälter darüber, sonst liegende Items;
  Öfen nehmen von oben Zutaten, von der Seite Brennstoff und geben unten das
  Ergebnis (und leere Eimer) ab. Ein Strom hält den Trichter an.
- Werfer und Spender: Strom löst sie aus, vier Ticks später nehmen sie einen
  zufälligen Platz. Werfer füllen den Behälter vor sich oder werfen das Item
  hinaus; Spender schießen Pfeile, gießen und füllen Eimer, legen Feuer mit
  dem Feuerzeug, zünden TNT und streuen Knochenmehl.
- Feuer nach Vanillas `FireBlock`: Brennbarkeit und Abbrennen je Block aus
  den Daten des Jars, Feuer altert, springt auf Luft neben brennbaren Blöcken
  über, frisst sie und zündet TNT; es geht aus ohne Halt oder im Regen,
  Netherrack brennt ewig, Seelenfeuer auf Seelensand. Lava zündet brennbare
  Blöcke in der Nähe. Feuerzeug und Feuerkugel zünden Feuer, Lagerfeuer,
  Kerzen und Kerzenkuchen. Pfeile, die durch Feuer oder Lava fliegen,
  brennen, zünden TNT, das sie treffen, und setzen Getroffene fünf Sekunden
  in Brand; Wasser und Regen löschen sie.
- Wetter nach `ServerLevel.advanceWeatherCycle`: klare Phasen, Regen und
  Gewitter wechseln zufällig, blenden über fünf Sekunden ein und aus, neue
  Spieler sehen den Regen gleich. Regen löscht Feuer und brennende Spieler
  und Mobs, hält Ackerboden nass und füllt Kessel. Bei Gewitter schlagen
  Blitze nahe Spielern ein, zünden Feuer, schaden und laden Creeper auf
  (doppelt so große Explosion). `/weather clear|rain|thunder [Dauer]`,
  `/summon lightning_bolt`. Gespeichert in `data/minecraft/weather.dat`.
- Umweltschaden wie bei Vanilla: Feuer (1, Seelenfeuer 2) und Lava (4)
  setzen in Brand, Brennen schadet einmal pro Sekunde, Wasser und Regen
  löschen; unter Wasser sinkt der Luftvorrat (Blasen im Client) bis zum
  Ertrinken (2 pro Sekunde); Kakteen, Magma und Blitze schaden. Gilt für
  Spieler und Mobs, Untote ertrinken nicht. Items verbrennen in Feuer und
  Lava (außer Netherit). Brennen und Luft werden gespeichert.
- Spielregeln: alle Regeln von Vanilla 26.3 mit ihren Grenzen, `/gamerule`
  für jede, gespeichert in `data/minecraft/game_rules.dat` wie bei Vanilla.
  Ferrocraft folgt `advance_time`, `advance_weather`, `random_tick_speed`,
  `water_source_conversion`, `lava_source_conversion`,
  `fire_spread_radius_around_player`, `fire_damage`, `drowning_damage`,
  `fall_damage`, `freeze_damage`, `mob_griefing` (Creeper) und
  `player_movement_check`.
- Server-Konsole: Befehle auf der Standardeingabe laufen mit
  Operator-Rechten am Weltspawn, die Antworten stehen auf Englisch im Log
  (Vanillas `en_us.json` aus dem Server-Jar); dazu `say`, `save-all` und
  `stop`.
- Bewegungsprüfung nach Vanillas `handlePlayerPositionChange`: mehr als zehn
  Blöcke in einem Tick und Bewegungen in Blöcke hinein werden mit einem
  Teleport zurückgenommen, im Überlebensmodus auch Bewegungen, die die Blöcke
  verhindert hätten (mit Vanillas Hochsteigen auf Stufen bis 0,6 Blöcke).
  Wer im Überlebensmodus vier Sekunden ohne Halt schwebt, wird getrennt,
  außer mit `allow_flight = true`.
- Pfeile, gezündetes TNT und fallende Blöcke werden im Vanilla-Format
  gespeichert und fliegen, brennen und fallen nach dem Neustart weiter.

- Block-Updates wie Vanillas `Level.setBlock`: Ein geänderter Block sagt es
  seinen Nachbarn (`neighborChanged`, in Vanillas Reihenfolge und ohne
  Rekursion über den `CollectingNeighborUpdater`), die ihre Form anpassen
  (`updateShape`): Pflanzen ohne Boden, Fackeln, Hebel, Knöpfe und
  Druckplatten ohne Halt fallen mit Beute ab. Dazu geplante Ticks mit
  Prioritäten (`LevelTicks`) und Zufalls-Ticks in den Chunks um die Spieler.
  Geplante Ticks werden wie bei Vanilla im Chunk gespeichert und laufen nach
  dem Neustart weiter.
- Wasser und Lava fließen nach Vanillas `FlowingFluid`: eine Stufe weniger je
  Block (Lava zwei), nach unten zuerst, zum nächsten Loch in bis zu vier
  Blöcken (Lava zwei), alle 5 Ticks (Lava 30); zwei Wasserquellen machen eine
  dritte. Lava an Wasser wird zu Obsidian oder Bruchstein, Lava auf Wasser zu
  Stein. Wasser spült Pflanzen, Fackeln und Redstone weg und lässt sie
  fallen. Eimer schöpfen Quellen und gießen sie aus, auch in Blöcke, die
  Wasser halten (`waterlogged`). Gegen Vanilla geprüft: dieselben Befehle auf
  beiden Servern ergeben Block für Block dieselbe Welt.
- Fallende Blöcke: Sand, Kies, Betonpulver, Ambosse und andere fallen als
  Entity und landen wieder als Block, wo kein Platz ist als Item;
  Betonpulver wird im Wasser zu Beton.
- Landwirtschaft: Hacke, Schaufel und Axt verwandeln Blöcke nach Vanillas
  `block_transformer`-Daten (Ackerboden, Trampelpfad, entrindete Stämme,
  Kupfer abschaben). Samen nur auf Ackerboden; Weizen, Karotten, Kartoffeln,
  Rote Bete wachsen nach Licht und Nachbarschaft, nasser Ackerboden
  beschleunigt; Kürbis- und Melonenstiele setzen Früchte. Ackerboden wird
  nass in der Nähe von Wasser, trocknet sonst aus und wird zu Erde, Fallen
  zertrampelt ihn.
- Bäume aus Setzlingen nach Vanillas Baum-Features im Datenpaket (Stamm- und
  Laubformen: gerade, gegabelt, Schwarzeiche, große Eiche, Kirsche; Kugel,
  Fichte, Akazie, Schwarzeiche, große Eiche, Kirsche); Schwarzeichen und
  Blasseichen aus vier Setzlingen. Laub ohne Stamm in Reichweite zerfällt und
  lässt Setzlinge, Stöcke und Äpfel fallen; gesetztes Laub bleibt.
- Gras breitet sich im Licht auf Erde aus und stirbt im Dunkeln, Zuckerrohr
  und Kakteen wachsen bis drei Blöcke hoch (Kakteen blühen), Knochenmehl
  lässt Pflanzen, Setzlinge und Gras wachsen.
- Redstone: Hebel, Knöpfe, Druckplatten (Stein, Holz, Wägeplatten) und
  Redstone-Fackeln und -Blöcke geben Signal, starke und schwache Ladung von
  Blöcken wie bei Vanilla. Leitungen mit Vanillas Standard-Auswertung, ihre
  Verbindungen (auch über Stufen) und Kreuz/Punkt per Rechtsklick;
  Redstone-Fackeln als Inverter (brennen bei zu schnellem Takt aus);
  Verstärker mit 1–4 Ticks Verzögerung, Sperre von der Seite, Rechtsklick
  stellt um. Lampen (aus nach 4 Ticks), Türen, Eisentüren, Falltüren,
  Zauntore und TNT reagieren auf Signale.
- Befehle `/setblock`, `/fill` (mit `destroy`, `keep`, `hollow`, `outline`,
  `replace`) und `/gamerule` für `random_tick_speed`,
  `water_source_conversion` und `lava_source_conversion`.
- `compare_worlds` (Beispielprogramm): zwei Welten Block für Block
  vergleichen.
- Bogen: Rechtsklick spannt, Loslassen schießt; die Kraft hängt von der
  Spannzeit ab, voll gespannt fliegen kritische Pfeile mit Zufallsbonus. Im
  Überlebensmodus braucht er Pfeile, verbraucht einen pro Schuss und nutzt
  sich ab. Steckende Pfeile lassen sich wieder aufsammeln, wer getroffen wird,
  weiß, von wem; der Schütze hört den Treffer.
- TNT: mit Feuerzeug oder Feuerkugel gezündet, hüpft es hoch und explodiert
  nach vier Sekunden mit Stärke 4. Die Blöcke lassen dabei alles fallen (kein
  Verfall wie bei Creepern), anderes TNT in der Nähe geht mit kurzer Lunte
  hoch, Mobs und TNT werden weggeschleudert. Wer es gezündet hat, gilt als
  Täter.
- Züchten: Tiere mit ihrem Futter füttern (Weizen, Karotten, Samen …) macht
  sie verliebt (Herzen); zwei verliebte finden sich und bekommen ein Junges
  in halber Größe, dazu Erfahrung. Die Eltern brauchen fünf Minuten Pause,
  Junge wachsen in zwanzig Minuten heran (Füttern beschleunigt), folgen den
  Großen und lassen beim Tod nichts fallen. Lämmer bekommen die
  Mischfarbe ihrer Eltern nach Vanillas Farbrezepten. Jede zwanzigste Kuh,
  jedes zwanzigste Schaf usw. einer neuen Herde ist ein Junges.
- Monster: Zombies, Skelette, Creeper und Spinnen spawnen nach Vanillas
  Regeln im Dunkeln (Licht, Obergrenze je Spieler, Gewichte der Ebene),
  verschwinden fern der Spieler und im Friedlich-Modus. Sie suchen sich
  Spieler im Überlebensmodus in Sichtweite, laufen mit Wegfindung zu ihnen
  und greifen an: Zombies und Spinnen schlagen zu, Spinnen springen und
  klettern Wände hoch und sind bei Tag friedlich, Skelette spannen ihren
  Bogen und schießen Pfeile, Creeper zischen und explodieren. Zombies und
  Skelette fangen in der Sonne Feuer. Beute nach Vanillas Tabellen,
  Geräusche, Babyzombies. `monsters = false` schaltet sie ab.
- Wegfindung (A*) für alle Mobs: um Wände herum, Stufen hoch, ein paar
  Blöcke hinunter, nie in Lava, Feuer oder Kakteen.
- Explosionen wie bei Vanilla: Strahlen durch die Blöcke nach deren
  Explosionsfestigkeit, Schaden und Rückstoß nach Abstand und Deckung; die
  Blöcke lassen ihre Beute mit Verfall fallen, Kisten ihren Inhalt.
- Pfeile mit Schwerkraft, Treffern an Blöcken und Wesen und Schaden nach
  Geschwindigkeit; stecken bleibende Pfeile verschwinden nach einer Minute.
- Erfahrung: Kugeln von getöteten Mobs und aus Öfen fliegen zum nächsten
  Spieler und verschmelzen; Level und Fortschritt wie bei Vanilla, Fanfare
  alle fünf Level, gespeichert in der Spielerdatei. Wer stirbt, verliert seine
  Erfahrung, ein Teil fällt als Kugeln heraus.
- Öfen, Räucheröfen und Hochöfen: Brennstoffe und Brennzeiten aus Vanillas
  `cooking_fuel`-Daten, Ofenrezepte aus dem Datenpaket, Räucher- und Hochöfen
  doppelt so schnell, Lava lässt den Eimer zurück. Das Menü zeigt Feuer und
  Fortschritt, Shift-Klick sortiert Erz nach oben und Brennstoff nach unten,
  der Block glüht beim Brennen. Herausnehmen gibt die Erfahrung der Rezepte.
- Rüstung schützt (Rüstungswerte und Härte aus Vanillas Daten) und nutzt sich
  bei Treffern ab.
- Schwierigkeitsgrad `difficulty` (wie Vanilla `easy` voreingestellt) und
  `/difficulty`: Schaden durch Mobs und Explosionen, Verhungern bis zur halben
  Leiste, einem halben Herz oder zum Tod, Heilen im Friedlich-Modus.
- Tiere, Monster, liegende Items und Erfahrungskugeln werden in Vanillas
  Entity-Speicher gesichert und kommen nach einem Neustart zurück; Entities
  aus Vanilla-Welten, die Ferrocraft nicht kennt, bleiben erhalten. Tiere
  spawnen jetzt wie bei Vanilla alle 20 Sekunden und verschwinden nicht mehr.
- `/summon` für Zombies, Skelette, Creeper und Spinnen, `/experience` (`/xp`)
  zum Vergeben und Abfragen von Erfahrung; Creeper lassen sich mit Feuerzeug
  anzünden.
- Bots: Rolle `idle` und `--commands`, um Befehle auszuführen.
- Tiere: Kühe, Schweine, Schafe und Hühner spawnen in Herden auf Gras um die
  Spieler, laufen umher, schauen Spieler an, folgen jemandem mit ihrem Futter
  in der Hand und rennen weg, wenn man sie schlägt. Sie geben, was Vanillas
  Loot-Tabellen sagen (Rindfleisch, Leder, Wolle in der Farbe des Schafs …).
  Schafe lassen sich scheren, Kühe melken, Hühner legen Eier, und alle machen
  ihre Geräusche. `animals = false` schaltet sie ab.
- Nahkampf gegen Tiere und andere Spieler (im Überlebensmodus): Schaden nach
  Waffe und Aufladung, kritische Treffer im Fallen, Rückstoß, Sprint-Schläge,
  Abnutzung der Waffe, Todesmeldung „von Spieler erschlagen“.
- `/summon` für Kühe, Schweine, Schafe und Hühner.
- Werkzeuge nutzen sich um ihren `damage_per_block` ab, zerbrechen mit Geräusch,
  und Schwerter bauen im Kreativmodus keine Blöcke ab.
- Überlebensmodus: Abbauen dauert so lange wie bei Vanilla (Härte, Werkzeug,
  unter Wasser, in der Luft), andere sehen die Risse. Blöcke lassen fallen,
  was Vanillas Loot-Tabellen sagen (Gras gibt Erde, Stein ohne Spitzhacke
  nichts); Werkzeuge nutzen sich ab. Gesetzte Blöcke verbrauchen Items.
- Leben, Hunger und Essen: Fallschaden (Heuballen, Betten, Schleim dämpfen),
  Hunger durch Sprinten, Springen und Abbauen, Heilen bei vollem Magen,
  Verhungern, Essen mit Wartezeit. Wer stirbt, verliert sein Inventar auf den
  Boden und kann am Spawn wiederbeleben.
- Spielmodi Überleben, Kreativ, Abenteuer und Zuschauer, `game_mode` für neue
  Spieler und `operators` in `ferrocraft.toml`; Befehle `/gamemode`, `/time`,
  `/tp`, `/give`, `/kill` und der F3+F4-Umschalter für Operatoren.
- Items fallen lassen (Q, Strg+Q, aus dem Inventar werfen), aufsammeln, und
  der Mittelklick sucht den Block aus (im Kreativmodus auch, wenn man ihn
  nicht hat).
- Inventar und Kisten wie bei Vanilla: Ziehen, Shift-Klick, Zahlentasten,
  Doppelklick, Werfen. Das 2×2-Feld im Inventar und die Werkbank craften mit
  Vanillas Rezepten.
- Kisten (auch doppelt), Fässer, Spender, Werfer, Trichter, Shulker-Kisten und
  die Endertruhe lassen sich öffnen; zwei Spieler sehen dieselbe Kiste, der
  Deckel geht auf. Beim Abbauen fällt der Inhalt heraus.
- Schilder lassen sich beschreiben; alle sehen den Text, er wird gespeichert.
- Türen, Falltüren, Zauntore und Hebel gehen per Rechtsklick, Knöpfe springen
  nach einer Weile zurück.
- Licht wird bei jeder Blockänderung neu berechnet, Fackeln leuchten, Dächer
  werfen Schatten.
- `level.dat` mit Spawn, Weltzeit und Seed; Tag und Nacht laufen weiter.
- Bots mit Rollen (`--roles`): Läufer springen, schleichen und wechseln ihre
  Items, Baumeister bauen Türme aus wechselnden Blöcken mit Fackel und reißen
  sie wieder ab, Hausmeister benutzen Kiste, Tür und Hebel, Werfer werfen Items
  und sammeln sie ein. Alle sagen ab und zu etwas im Chat.
- `OFFENE-PUNKTE.md`: was noch fehlt, und der Plan für Fabric- und Forge-Mods.

### Behoben

- Wer von einem Creeper in die Luft gesprengt wurde, bekam eine
  Todesmeldung ohne den Creeper: er verschwand vor seiner Explosion.
- Vanilla-Welten luden Chunks, die Vanilla nur angefangen hatte zu erzeugen,
  als fertig: Um den erkundeten Bereich herum fehlte dann der Boden, und der
  Start dauerte lange (900 Chunks, 16 s). Solche Chunks werden jetzt
  übersprungen und neu erzeugt.
- Wird die Maschine beim Speichern hart abgeschaltet, bleibt keine leere
  Spielerdatei mehr zurück, mit der der Spieler nicht mehr hereinkam: Dateien
  werden vor dem Umbenennen auf die Platte geschrieben (`fsync`), die vorige
  Fassung bleibt wie bei Vanilla als `…_old` liegen und wird gelesen, wenn die
  neue kaputt ist. Eine unlesbare Spielerdatei verhindert den Beitritt nicht
  mehr.
- Scheitert ein Beitritt mitten drin, zählt der Spieler nicht weiter als
  online.

### Geändert

- Der Test-Client liest jetzt Items mit Komponenten (vorher übersprang er
  sie still), über die Komponentenformen des Protokoll-Crates.
- Schaden für Spieler und Mobs läuft durch einen Weg wie Vanillas
  `actuallyHurt`: Rüstung (aus Attributen, mit Abnutzung, die
  Schadensresistenz wie bei Netherit beachtet), Resistenz-Effekt,
  Schutz-Verzauberungen, Absorption.
- Spieler-gegen-Spieler-Rückstoß kommt mit dem Extra-Rückstoß in einem
  Paket; Rückstoß beachtet Rückstoßresistenz.
- Neue Spieler haben wie bei Vanilla den Verzauberungs-Seed 0, gespeichert
  als `XpSeed`.
- Ein fehlender Schlüssel in Loot-Bedingungen ließ den Tick-Thread abstürzen;
  behoben.

- Block-Updates ohne `UPDATE_CLIENTS` erreichen die Clients nicht mehr, wie
  bei Vanilla: so sehen sie Kolbenbewegungen aus dem Block-Event statt aus
  Zwischenzuständen, und unsichtbare Änderungen (Alter von Zuckerrohr)
  bleiben beim Server.
- Ein Spieler, der einen Block mit Wasser abbaut, lässt das Wasser stehen.
- Die Bots gehen höchstens einen Block pro Tick, wie ein Client, damit die
  Bewegungsprüfung sie nicht zurücksetzt.
- Die Blockdaten (`scripts/BlockPhysics.java`) haben je Zustand das
  Kolbenverhalten, ob ein Komparator den Block liest und ob Lava ihn
  entzündet, je Block die Brennbarkeit.
- Pflanzen, Fackeln und Redstone-Bauteile lassen sich nur setzen, wo sie
  halten (Samen auf Ackerboden, Zuckerrohr neben Wasser …); Fackeln an
  Wänden werden zu Wandfackeln. Gesetztes Laub zerfällt nicht. Gesetzte
  Lampen und Türen sind gleich an, wenn sie Strom haben.
- Beim Benutzen eines Items dreht sich der Spieler wie bei Vanilla auf die
  Blickrichtung des Pakets `use_item` (Bogen, Eimer).
- Knöpfe springen über geplante Block-Ticks zurück statt über eine eigene
  Warteschlange; sie geben jetzt Redstone-Signal.

- RSA beim Login über aws-lc-rs (zeitkonstant) statt des `rsa`-Crates mit
  RUSTSEC-2023-0071; der Server-Schlüssel hat jetzt 2048 Bit.
- `rand` 0.9.

- Online-Modus wie bei Vanilla: Verschlüsselung und Prüfung bei Mojang, mit
  Skins. `online_mode = false` lässt wie bisher jeden unter jedem Namen herein.
- Pakete werden ab 256 Byte komprimiert; ein Join bei Sichtweite 10 braucht
  damit rund 80 KB statt 3,5 MB (`compression_threshold`).
- Mehrspieler: Man sieht die anderen laufen, schauen, schleichen, schlagen und
  was sie in der Hand halten, mit Ping in der Tab-Liste. Wer sich ein zweites
  Mal anmeldet, löst die erste Sitzung ab.
- Chat für alle, Beitritts- und Abschiedsmeldungen in der Sprache des Clients,
  `/list`.
- Bauen im Kreativmodus: Blöcke setzen und abbauen, alle sehen es sofort.
  Treppen, Stämme, Stufen, Knöpfe und Öfen zeigen in die richtige Richtung,
  Türen und Betten bekommen beide Hälften.
- Die Welt wird gespeichert, im Format von Vanilla 26.3: alle 5 Minuten, beim
  Verlassen und beim Beenden. Spieler behalten Position und Inventar.
  Vanilla-Welten lassen sich laden, ohne dass beim Speichern etwas verloren
  geht.
- Werkzeug `bot`: Bots laufen im Kreis um den Spawn. Der `proxy` entpackt
  jetzt auch komprimierte Pakete.

- Beitreten klappt: Nach Login und Configuration steht man im Kreativmodus in
  einer endlosen Superflat-Welt und kann herumlaufen und fliegen; Chunks werden
  beim Laufen nachgeladen und hinter einem vergessen.
- Configuration wie bei Vanilla: Registries und Tags kommen aus Mojangs
  Datengenerator, der Client nimmt die Inhalte aus seinem eigenen Vanilla-Paket.
- Ein Tick-Thread mit 20 TPS schickt alle 15 s ein Keep-Alive und wirft
  Clients raus, die nicht innerhalb von 15 s antworten.
- Ist der Server voll, bekommt man das beim Beitreten gesagt.
- `view_distance` in `ferrocraft.toml` legt fest, wie viele Chunks um den
  Spieler geschickt werden.
- Werkzeuge: `proxy` schneidet den Verkehr zwischen Client und Server mit,
  `gen-data` erzeugt die Spieldaten für eine Version.

- Der Server erscheint in der Mehrspieler-Liste von Minecraft 26.3 mit MOTD,
  Spielerzahl und Version, und der Ping wird angezeigt.
- Clients mit einer anderen Version bekommen gesagt, ob sie zu alt oder zu neu sind.
- Einstellungen in `ferrocraft.toml` (Adresse, MOTD, maximale Spielerzahl), beim
  ersten Start mit Standardwerten angelegt.
