# Bomberman L4D

Bomberman für das Netzwerk: ein Server auf Ubuntu, ein Spielprogramm für jeden
Windows-Rechner. Entstanden aus den drei Teams des BTC-Hackathons 2026:

| Hackathon-Teil | Hier |
|---|---|
| Basisspiel (Rust-Server) | `crates/bomber-{domain,protocol,application,server}` |
| Bomberman-Client mit Bot (Python/pygame) | nach Rust portiert: `crates/bomber-client` |
| Visualisierung und Moderation (Java/JavaFX) | in den Client integriert |

```
 Windows-PC  ┐
 Windows-PC  ├── UDP 47800 ──►  Ubuntu-Server  ◄── ws 8080 (nur localhost) ── Moderationskonsole
 Windows-PC  ┘                  bomber-server
 Bomberman.exe: Lobby, Start, Spiel, Bot, Ergebnis
```

Jeder verbundene Spieler kann das Match starten, pausieren und abbrechen. Das
geht über denselben UDP-Kanal, über den er spielt. Der Server glaubt einen
Befehl nur von der Adresse, der der Platz gehört. Der unauthentifizierte
Web-Port muss dafür nicht ins Netz.

---

## Server auf Ubuntu

Einmalig Rust installieren, dann das Installationsskript aus dem Repository
ausführen:

```bash
sudo apt install -y build-essential curl git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"

git clone <repo-url> bomberman-l4d && cd bomberman-l4d
sudo ./deploy/ubuntu/install.sh
```

Das Skript baut den Server, legt den Systembenutzer `bomberman` an, installiert
`/opt/bomberman-l4d/bin/bomber-server`, die Konfiguration `/opt/bomberman-l4d/server.toml`
und den systemd-Dienst `bomberman`. Bei aktiver `ufw` gibt es UDP-Port 47800
frei. Ein erneuter Aufruf nach `git pull` aktualisiert Binary und Dienst und
lässt eine angepasste Konfiguration stehen.

| Aufgabe | Befehl |
|---|---|
| Protokoll ansehen | `journalctl -u bomberman -f` |
| Nach Konfigurationsänderung | `sudo systemctl restart bomberman` |
| Stoppen | `sudo systemctl stop bomberman` |

Nach außen offen sein muss nur **UDP 47800**. Die Moderationskonsole auf Port
8080 hat keine Authentifizierung und lauscht deshalb nur auf `127.0.0.1`. Wer
sie braucht, holt sie per SSH-Tunnel:

```bash
ssh -L 8080:127.0.0.1:8080 <server>
# dann http://127.0.0.1:8080 oder den Java-Viewer mit --host=127.0.0.1
```

Die wichtigsten Einstellungen in `server.toml`:

| Schlüssel | Wirkung |
|---|---|
| `lobby.min_players` / `max_players` | ab wie vielen Spielern gestartet werden kann, höchstens 4 |
| `lobby.player_control` | `false` erlaubt Start und Pause nur noch der Moderation, etwa für ein Turnier |
| `lobby.server_bots` | `true` lässt freie Plätze auf Wunsch mit einem Bot des Servers besetzen |
| `lobby.bot_seats` | Plätze (ab 0), die ein Server-Bot besetzt, sobald ein Spieler da ist, z. B. `[2, 3]` |
| `lobby.drop_after_secs` | wer so lange nichts sendet, verliert seinen Platz (Standard 30, `0` schaltet es ab) |
| `rules.round_time_secs`, `rules.sudden_death_secs` | Rundenlänge und Beginn des Sudden Death |
| `map.width`, `map.height`, `map.seed` | Spielfeld; `seed = 0` würfelt jedes Match neu |

---

## Client auf Windows

`Bomberman.exe` ist eine einzelne Datei, rund 3,5 MB. Sie braucht keine
Installation, kein Python und kein Java. Voraussetzung ist Windows 10 oder 11
mit einem OpenGL-fähigen Grafiktreiber, also praktisch jeder Rechner.

1. `Bomberman.exe` starten.
2. Namen und Serveradresse eintragen, **Verbinden**.
3. In der Lobby startet jeder Spieler das Match mit **Enter** oder dem Knopf.

Wer während eines laufenden Matches dazukommt, wartet auf dem
Verbindungsbildschirm. Er wird aufgenommen, sobald das Match endet, und spielt
im nächsten mit.

Beim ersten Start kann Windows SmartScreen warnen, weil die Datei nicht
signiert ist: „Weitere Informationen“, dann „Trotzdem ausführen“.

| Taste | Wirkung |
|---|---|
| Pfeiltasten oder WASD | laufen |
| Leertaste | Bombe legen, mit Richtung: legen und weglaufen |
| Enter | Match starten, nach dem Match: nächstes Match |
| B | eingebauten Bot ein- oder ausschalten |
| P | Pause für alle ein und aus |
| Esc | Menü: Pause, Match abbrechen, Bot, Ton, Trennen |
| M | Ton ein und aus |
| F11 | Vollbild |
| F3 | technische Anzeige: Bildrate, Tick, Phase |

Name, Server, Bot und Ton merkt sich der Client in
`%APPDATA%\Bomberman-L4D\client.cfg`. Stürzt er ab, steht der Grund in
`crash.log` daneben.

Für Verknüpfungen oder Bot-Runden gibt es Startparameter:

```
Bomberman.exe --host 10.0.0.5 --name Anna --connect
Bomberman.exe --host 10.0.0.5 --name Bot1 --bot --connect --autostart
```

`--autostart` fordert den Start an, sobald er möglich ist, und lässt Ergebnisse
sechs Sekunden stehen. Damit laufen Bot-Turniere ohne Aufsicht.

### Die .exe bauen

Unter Linux per Cross-Compiler, das Ergebnis landet in `dist/Bomberman.exe`:

```bash
sudo apt install gcc-mingw-w64-x86-64
./scripts/build-windows.sh
```

Oder direkt unter Windows mit Rust und den Visual Studio Build Tools:
`scripts\build-windows.ps1`.

---

## Was der Client zeigt

- **Lobby** mit festen Plätzen, Figur und Name je Spieler. Ein Spieler, der
  nicht mehr antwortet, ist markiert. Der Start-Knopf richtet sich nach der
  Antwort des Servers. Freie Plätze lassen sich mit **+ Server-Bot** durch
  einen Bot des Servers besetzen, das **X** auf der Karte schickt ihn wieder
  weg. Kommt ein neuer Spieler an einen vollen Tisch, übernimmt er den Platz
  eines Bots. Verlässt der letzte Spieler die Lobby, gehen auch alle Bots.
  Wer 30 Sekunden lang nichts sendet, wird aus der Lobby entfernt.
- **Countdown** 3, 2, 1 mit Ton.
- **Spielfeld** mit flüssiger Bewegung zwischen den 60 Server-Ticks.
  Feuerbalken werden aus Mitte, Armen und Spitzen zusammengesetzt. Kisten
  zerfallen, Bomben pulsieren schneller, je kürzer die Zündschnur ist.
  Explosionen lassen den Bildschirm wackeln. Zellen, die Sudden Death als
  Nächstes zumauert, blinken rot.
- **Seitenleiste** mit Bomben, Reichweite, Tempo und Punkten je Spieler.
- **Ergebnis** mit Sieger, Rangliste und Konfetti. Enter startet das nächste
  Match.

Alle Grafiken, Schriften und Töne stecken in der `.exe`. Die Töne werden beim
Start berechnet, es gibt keine Audiodateien.

---

## Entwicklung

```bash
just test        # alle Tests
just lint        # clippy, Warnungen verboten
just arena       # Server und zwei Bot-Clients, die selbst starten
just server      # nur der Server
just client      # der Client unter Linux
just windows     # dist/Bomberman.exe
```

Unter Linux braucht der Client zum Linken `libasound2-dev`. X11 und OpenGL lädt
er erst zur Laufzeit.

Die verbindliche Protokollbeschreibung steht in `BOT_GUIDE.md`, die
Moderations-Schnittstelle in `MODERATION_API.md`, der WebSocket-Vertrag in
`VISUALIZER_GUIDE.md`.

### Änderungen am Protokoll gegenüber dem Hackathon

Alle Änderungen sind abwärtskompatibel. Bestehende Bots laufen unverändert.

- **Uplink-Codes 10 bis 13** sind jetzt Lobby-Befehle: Start, Pause,
  Fortsetzen, Abbrechen. **Code 14** (vier Bytes: zusätzlich Platz und
  an/aus) besetzt einen Platz mit einem Server-Bot oder gibt ihn frei.
- **LOBBY_STATUS** hat hinter den bisherigen 11 Bytes einen Anhang mit
  Pause-, Start-, Steuerungs- und Server-Bot-Flag, Mindestspielerzahl sowie
  Name und Status jedes Platzes (auch ob ein Server-Bot ihn hält). Er wird zusätzlich sofort bei jeder Änderung gesendet
  und während einer Pause weiter, damit Clients die Verbindung nicht für tot
  halten.
- **MATCH_END** kommt jetzt auch bei einem Abbruch, nicht nur bei einem
  regulären Ende.
- **Neue Spieler** werden auch auf dem Ergebnisbildschirm aufgenommen, nicht nur
  in der offenen Lobby. Sonst fände ein Nachzügler keine Lücke, wenn die
  anderen direkt „Nächstes Match“ drücken.

### Der Java-Viewer

Der JavaFX-Viewer aus dem Hackathon liegt weiter in seinem eigenen Repository.
Er spricht nur den WebSocket, dessen JSON-Format hier unverändert ist. Er lässt
sich also weiter als Beamer-Ansicht oder Moderationskonsole über den SSH-Tunnel
nutzen. Zum Spielen wird er nicht mehr gebraucht.
