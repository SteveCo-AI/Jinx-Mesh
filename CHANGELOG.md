# Changelog

## 2.6.1 — 06.11.2026

Mechanik- und Release-Korrekturen auf Basis des v2.6.0-Prüfberichts. Keine
neuen Fähigkeiten; die Suite (86 Lib-, 4 Acceptance-, 17 Security-,
4 Review-Tests) ist grün.

- **F01 — Journal fail-closed:** Schlägt das Anhängen eines
  Op-Journal-Eintrags fehl (Nachricht, Konsolidierung, Training,
  Heilbefehle), wird die veränderte Sitzung jetzt deaktiviert und der Fehler
  gemeldet, statt sie aktiv zu lassen. Vorher konnte genau das einen Vault
  hinterlassen, der nicht mehr ladbar war.
- **F02 — Undo kalter Knoten:** `/undo` lädt kalte Records jetzt mit ihren
  vollständigen Kanten, bevor es zurückrollt; unveränderte Kanten kalter
  Knoten wurden vorher gelöscht. Undo echter Neuanlagen funktioniert
  weiterhin.
- **F04 — Undo-Budget:** Die Rücknahme läuft jetzt in Batches und hält das
  aktive Knotenset unter dem 128-Knoten-Budget. Vorher konnte etwa
  `/undo 35` den Arbeits-Cache über das Budget treiben.
- **V01 — Entscheidung des Owners (bindend):** Die 2.6.0-Ankerwörter
  (`anker_*`) sind entfernt — Ankerwörter können zufällig in beliebigen
  Sätzen vorkommen und Gewichte verfälschen. Bewertungssignale sind nur noch
  die expliziten `+`/`-`-Präfixe (DEV-Werkzeug) und der Wiederholungs-Kanal
  (doppelte Eingabe = negatives Signal). Die implizit positive Bewertung
  entfällt; der Kap.-19-Sprach-Fallback bleibt als Mechanik im Code, ist
  aber über `fallback_aktivier` standardmäßig AUS (analog
  `mutation_aktivier`). Tests auf entfernte Anker-Funktionen sind mit der
  Funktion entfallen; neue Tests sichern, dass frühere Ankerwörter als
  gewöhnliche Wörter lernen, ohne Bewertung auszulösen.
- **F03 — Release-Lieferung:** `bin/saeugling-linux-x86_64` — genau der
  Pfad, den `start.sh` startet — enthält jetzt die frische 2.6.1-Binary;
  das 2.6.0-Release lieferte dort noch das 2.5.0-Binary.
  `SHA256SUMS-2.6.1.txt` ist NACH dem Kopieren über die tatsächlich
  gelieferten Dateien erzeugt (inklusive des `bin/`-Startpfads) und ist für
  dieses Release maßgeblich. Die alte `SHA256SUMS-2.6.0.txt` bleibt aus
  Dokumentationsgründen liegen, ist aber nicht maßgeblich — sie nannte
  einen Pfad, der im Checkout nicht existiert.
- **Tests:** Die Regressionstests des Prüfberichts laufen als echte Tests
  mit (`tests/review_v260.rs`), damit die Fixes nicht wieder einbrechen.
