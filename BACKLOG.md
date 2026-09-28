# Backlog

Stand: 28.09.2026. Einzige Arbeitsliste für offene Fehler, Wartung und Ideen —
keine weiteren Review-/Findings-Dateien anlegen, neue Punkte hier eintragen.
Erledigte Punkte entfernen; Details stehen in den Fix-Commits.
[README](README.md) beschreibt Betrieb und Tests, [RULES](RULES.md) das Sollverhalten.

## Offene Fehler

F-IDs stammen aus dem unabhängigen Review, A-IDs aus dem anschließenden Abgleich.
P2 = normale, P3 = geringe Priorität.

### Bot / Worker / Betrieb

| ID | Prio | Problem und nächster Schritt |
|---|---|---|
| F07 | P2 | `main.rs`: Updates werden vor erfolgreicher Verarbeitung als gesehen gespeichert. Handlerfehler können Wiederholungen dauerhaft unterdrücken. Persistente Verarbeitung mit Retry und idempotenten Seiteneffekten entwerfen. |
| F08 | P2 | `handlers::list`: bis zu 50 × 500 Zeichen passen nicht in eine Telegram-Nachricht. Ausgabe auf mehrere vollständige, gültige Nachrichten verteilen. |
| A01 | P2 | `handle_unhandled`: normale DM-Texte umgehen den Benutzer-Limiter. Gemeinsames Budget vor DB-Zugriff und Antwort prüfen. |
| A02 | P2 | `claim_due_alerts`: ungültige Zeitzone/Enum führt nach dem Claim zu einem Mappingfehler. Mit Batchgröße 1 werden keine Nachbarzeilen mehr mitverworfen; defekte Zeile wird aber weiter erneut geclaimt. Isolieren und sichtbar als fehlerhaft behandeln. |
| A03 | P2 | `Schedule::from_db` akzeptiert ungültige RRULE; `next_after` liefert dann `None`, der Worker beendet die Serie. Fehler vom regulären Ende unterscheiden und diagnostizieren. |
| A04 | P2 | Deployment prüft `is-active` unmittelbar nach Restart; spätere Startfehler lassen den Job grün. Betriebsbereitschaft prüfen und vorheriges Binary für Rollback behalten. |
| A05 | P3 | Gruppenbestätigung escapet Anzeigenamen, sendet aber Plaintext (`&amp;`). Escaping und Versandmodus aufeinander abstimmen. |

### Parser / Zeitberechnung

| ID | Prio | Problem und nächster Schritt |
|---|---|---|
| F09 | P2 | Datum ohne Jahr: am 8.5. nach 09:00 wird `8.5 09` abgelehnt; `29.2` findet aus manchen Nicht-Schaltjahren kein nächstes Schaltjahr. Vollständigen zukünftigen Zeitpunkt suchen. |
| F10 | P2 | Einmaliges `so 02:30` am 28.03.2026 (Berlin) landet in der DST-Lücke fälschlich am Samstag, 04.04. Zielsonntag erhalten und Lücke auflösen. |
| F11 | P2 | Relative Serien `*1M` am 31.01. bzw. `*1Y` am 29.02. überspringen Monate/Jahre ohne dieses Datum. Monatsend-Fallback ergänzen; `*31.` und `*29.2` haben ihn bereits. |
| F12 | P2 | `*1h` kann beim Herbst-DST-Wechsel zwei reale Stunden Abstand haben. Exakte Intervalle unabhängig von lokaler Wanduhr auswerten. |
| F13 | P2 | `5min` scheitert nach Teilmatch `5m`. Kompakte Suffixe nur bei vollständigem Match akzeptieren und Langformen erreichbar lassen. |
| F14 | P2 | Gemischtes `1Y1d` addiert den Tag lokal und kann über DST um eine Stunde abweichen. Nach Kalenderkomponenten exakte Dauer addieren. |

## Wartung

Verbesserungen oder Punkte, die weitere Prüfung brauchen — keine nachgewiesenen
Produktionsausfälle.

- **CI/SSH:** Actions, Toolchain und cloudflared pinnen/verifizieren; explizite
  minimale Token-Rechte, Secret-Debug-Ausgaben entfernen, feste Hostkeys und
  Job-Timeouts; Lint-/Format-/Dependency-Prüfungen einführen. Clippy meldet vier
  `criterion::black_box`-Deprecation-Warnungen im Benchmark.
- **Betrieb:** Backup- und Retention-Fehler melden, Restore regelmäßig testen;
  Fehlerquote bei Zustellungen überwachen. `Requires=postgresql.service` bei echten
  PostgreSQL-Restarts auf dem Zielsystem prüfen. systemd-Härtung und
  Bereitschaft/Watchdog ergänzen; Stats nur auf bewusst gewählter interner Adresse.
- **Wachstum:** Alert-Retention/Reaper-Index und Limiter-Speicherbereinigung prüfen.
  Fehlerhafte Schedule-Daten mit passendem Storage-Fehlertyp melden (siehe A03).
- **UX:** `cancelled_inline` lokalisieren; ungültiges `/cancel` auch in Gruppen
  erklären; doppelten Benutzer-Read in `set_lang` vermeiden; veralteten
  Rate-Limit-Testkommentar (21. statt 7. Anfrage) korrigieren. Bestehendes Limit:
  6 Befehle/Callbacks pro Minute.
- **Tests:** weitere DST-/Kalendergrenzen und Fuzzing; es gibt noch kein
  eingechecktes Fuzz-Target.
- **Doku:** README-Runbook bei Bedarf kürzen.

## Ideen / Roadmap

- **iCal-Feed** (nächstes größeres Feature): ICS-Endpoint, den Kalender-Apps
  abonnieren. Als zusätzliche Route auf dem bestehenden `stats.rs`-axum-Server
  (`STATS_LISTEN`) — keinen dritten Server bauen. `Schedule` ist bereits
  RFC-5545 (Anchor + RRULE), für einen Read-only-Feed ist keine Schemaänderung
  nötig. Braucht aber ein **per-User-Secret-Token** in der URL, weil Kalender-Apps
  unauthentifiziert abrufen. Bei wachsender HTTP-Fläche `stats.rs` → `http.rs`.
- Mehrere Uhrzeiten, Wochentagsbereiche, Zeitfenster mit Intervallen;
  Cron-Syntax oder Parser-Umbau nur bei konkretem Bedarf.
- `/help`-Überarbeitung, `/edit`, geplante Wartungsfenster, Gruppen-Ratelimit.
- Admin-Banliste und Verschlüsselung ruhender Texte bei konkretem Bedarf.

## Bereits geprüft — nicht erneut aufmachen

- **Behoben:** F01–F06 (Zahlenüberläufe im Parser, Snooze-Offsets,
  Backup-Veröffentlichung, Claim-Generation/Migration 0005, persistenter Shutdown,
  Worker-Überwachung). Früher: Retry-Reset pro Serienauftreten, Callback-Chat-Guard,
  Intervall-Casts, Recurring-Overrides, Backup-Retention-Regex, Lizenzdateien,
  Agent-Skills aus Git entfernt.
- **Verworfen:** „Ohne `WEBHOOK_SECRET` keine Authentifizierung“ — die verwendete
  Teloxide-Version erzeugt und prüft selbst ein Token. Für die behauptete
  `.all(4)`-Fehlfunktion gab es keinen Nachweis.
