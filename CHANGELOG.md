# Changelog

Alle nennenswerten Änderungen an Ferrocraft. Format nach
[Keep a Changelog](https://keepachangelog.com/de/1.1.0/).

## [Unreleased]

### Neu

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

### Geändert

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
