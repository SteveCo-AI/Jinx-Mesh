# FIXPLAN 2.6.1 — Jinx-Mesh (v2.6.0-Befunde)
Stand: 06.11.2026 · Quelle: Ausführlicher Prüfbereicht (Lumen-Übergabe)
Checkout: /home/lumen/projekt-jinx-netz/jinx-mesh-rust (main, f89696e)
Regressionscode: /home/lumen/.hermes/cache/documents/doc_03abee06e55d_Jinx-Mesh-v2.6.0-Regressionstests.rs

## BEFUNDE (alle vier live auf diesem Checkout verifiziert bzw. bestätigt)
- F01 (P1): journal_append-Fehler lässt mutierte Sitzung aktiv → Vault unladbar.
  Ort: src/app.rs:445-451 + gleiche Muster in consolidate/train_file/Heilbefehle.
- F02 (P1): Undo löscht unveränderte Kanten kalter Knoten (alpha→charlie verlor).
- F03 (P1): STARTER-LIEFERUNG ALTER BINARY. LIVE BEWIESEN:
  bin/saeugling-linux-x86_64 --help → "Jinx 2.5.0"; sha256 = ee4ef006... (Alt-Binary);
  start.sh BIND auf genau diese Datei; SHA256SUMS-2.6.0.txt nennt 3812166b... unter
  Pfad 'saeugling', der im Checkout NICHT existiert. = Jinx' Release-Fehler aus der
  GO-Welle (Binary gebaut, Startpfad nie geprüft). ABNAHME: Neubau, Startpfad korrekt,
  Prüfsummen NACH dem finalen Kopieren erzeugen, Normalstart-Test frischer Checkout.
- F04 (P2): undo(35) → active_nodes=175 > Budget 128.
- V01 (ENTSCHEIDUNG RAPHAEL): v2.6.0-Defaults brechen alte Neutralität-Vorgabe
  (anker 'super'/'nein', implizit_positiv=0.1, Wiederholung=-0.25, Sprachgerüste).
  Rapfaels alte Vorgabe: ohne Präfix KEINE Bewertung, keine vorgegebenen Inhalte.
  Empfehlung: Opt-in-Schalter, alle DEFAULT AUS. → Rapfaels GO nötig.

## REIHENFOLGE (aus Bericht übernommen)
1. F01 Journal-Fehlerpfade (fail-closed; Abnahme-Kriterien 1-5 im Bericht)
2. F02 kalte Records mit vollständigen Kanten (Abnahme 1-6; nicht durch Map-Vereinigung
   kaschieren — Undo echter Neuanlagen muss weiter funktionieren)
3. F04 Rücknahme unter 128-Knoten-Budget (Batches; Peak-Speicher prüfen; F02 nicht
   durch Cachekürzung "reparieren")
4. V01 — RAPHAELS ENTSCHEIDUNG (06.11.2026, bindend):
   - Anker-Wörter ("super"/"nein"): MECHANIK ENTFERNEN. Begründung: Ankerwörter
     können zufällig in beliebigen Sätzen vorkommen und verfälschen Gewichte.
     + und - Präfixe bleiben das EINZIGE explizite Signal (reines DEV-Tool,
     Vorzeichen am Nachrichten-Anfang kommt statistisch kaum zufällig vor).
     Anker-Reifung entfällt mit (braucht Anker). Konfig-Keys anker_* raus.
   - Implizites Feedback: NUR Wiederholungs-Kanal bleibt (doppelter Input =
     negatives Signal). implizit_positiv auf 0.0 / Fortsetzungs-Bewertung raus.
     Signalquellen final: +/- Präfix UND Wiederholung.
   - Sprachgerüste (Kap-19-Fallback): Mechanik bleibt im Code, aber DEFAULT AUS
     (Schalter analog mutation_aktivier). Keine vorgefertigten Texte ohne GO.
   - Tests, die entfernte Anker-Verhaltens-Prüfen, entfallen MIT der Funktion
     (Produktentscheidung des Owners); neue Tests: Ankerwörter sind gewöhnliche
     Wörter (kein Bewertungs-Trigger).
5. Testphase: review_v260.rs als echte Tests ins Repo (JETZT noch absichtlich ROT —
   erst nach den Fixes hinzufügen, sonst scheitert jede CI), plus Vollläufe:
   cargo test --locked, fmt --check, clippy -D warnings
6. F03 Release-Neubau: saubere Binary, start.sh-Pfad, SHA256SUMS final, frischer
   Download-Test via ./start.sh; erst dann neuer Tag/Push

6. Forschungs-Welle RW-1 (FORSCHUNG-RW-1.md im Repo): Zustandsabhängiges Lernen &
   Überleben neuer Erfahrungen. Ricks Regel gilt: Experiment beweist Defizit
   VOR Mechanik-Einbau. Erst messen, dann Ursachen, dann optimieren.
   Experimente: E1 Einzel-Neuerfahrung (Erwerbsrate + S(N)), E2 Sättigung isoliert,
   E3 minimale epis. Schicht, E4 Bewertung isoliert, E5 Vorhersage aus Zustand.
   Start nach Publishing-GO 2.6.1. Ablage: jinx-mesh/forschung/RW-1/.

## REGELN
- Regressionsdatei von Lumen liegt bereit (Pfad oben); beim Fix-Block je Test
  aktivieren (grün = Heiler-Nachweis, nicht vorher).
- Vor jedem Block: Backup (Timestamp), nur Mechanik, keine Inhalts-Strings im Code.
- Jede Abnahme-Kriterien-Nummer des Prüfbreichs einzeln abhaken — nicht "sollte gehen".
