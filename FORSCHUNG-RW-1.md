# FORSCHUNGSWELLE-RW-1 — Zustandsabhängiges Lernen & Überleben neuer Erfahrungen
Angelegt: 06.11.2026 · Auftraggeber: Raphael (Auftragsentwurf: ChatGPT-Gegenanalyse,
von Raphael übernommen) · Ricks bindende Regel gilt für jede Phase: **Bevor eine
neue Lernmechanik eingebaut wird, muss mindestens ein Experiment zeigen, welches
konkrete Defizit der bisherigen Architektur sie behebt.**
Arbeitsanweisung: Zuerst messen, dann Ursachen isolieren, dann gezielt optimieren.
Keine neuen Mechanismen allein aufgrund plausibler Hypothesen.

## VORRANG & ORT
- V01/F03 haben Vorrang und sind KOMPLETT (06.11.). Diese Welle startet nach
  Raphaels Publishing-GO für 2.6.1.
- Eigenständige Welle in der FIXPLAN-Struktur. Keine Änderung der Lernarchitektur
  vor Messbefund. Reproduzierbar, negative Ergebnisse werden dokumentiert.

## ZENTRALE FORSCHUNGSFRAGE
Wirkt eine neue Erfahrung vom Zustand des Graphen bei ihrem Eintreffen ab statt
von der Wiederholungszahl? Beobachtungsbasis: B (hallo jinx! blieb dauerhaft,
danach Nichts formbar), D (Lumen-Treffer entstand und verfloß), A (Spiegel-Rate).
Drei vermutete Wirkgrößen: Plastizität, Konkurrenz, Retention. UNBEWIESEN.

## ZWIE SPALTUNG — LERNBLOCKADE vs. ABRUFBLOCKADE (Punkt 2 des Auftrags)
Fall A Lernblockade: Zuordnung kommt nicht in den Graph.
Fall B Abrufblockade: Zuordnung IST im Graph, wird aber nicht ausgewählt.
Experimente müssen beide Trennen. Wenn B: Keine Lernmechanik-Änderung nötig —
Antwortpfad-Auswahl ist der Hebel. MESSUNG: Nach Einführung der Zuordnung
Graph-Inhalt lesen (ist sie da?) UND Inferenz lesen (wird sie gesagt?).

## KENNZAHLEN (Punkt 3) — zwei statt ein Überlebenskoeffizient
- Erwerbsrate A = (unmittelbar korrekt abrufbar) / (eingeführte Zuordnungen)
- Bedingte Überlebensrate S(N) = (bei N weiteren Nachrichten noch korrekt) /
  (unmittelbar korrekt). N = 0, 50, 200, 500, 1000.
- Unbedingte Erfolgsrate nach N (erfasst spät-korrigierte Fälle).
- Zusätzlich je Kennzahl: Zeitpunkt des ersten Verlusts, Wiederauftreten
  verlorener Antworten, Änderungen konkurrierender Pfade,
  Zusammenhang Anfangszustand → Stabilität.
- Ein einzelner richtiger Treffer gilt NICHT als dauerhaftes Lernen.

## EXPERIMENT 1 — Überleben einer einzelnen Neuerfahrung (ohne Architekturänderung)
1. Definierter Ausgangs-Graph (Kopie), Prüfferage+erwartete Antwort NOCH NIE
   trainiert. Wort-Kategorien separat auswerten: völlig neue Wörter / bekannte
   Wörter, neue Kombination / bekannter Fragekontext + neue Antwort.
2. Zustandsaufnahme VOR Einführung (nur lesend): Knoten, Kantengewichte,
   Anzahl konkurrierender Fortsetzungen, Gewicht des stärksten Konkurrenten,
   vorhandene Bewertungen, Nutzungshäufigkeiten, Sättigungsgrad, alternative
   Pfade, aktuelle Antwort auf die Prüfferage.
3. EINE kontrollierte Lernerfahrung einführen. Dokumentieren: was wurde
   tatsächlich gelernt (Knoten/Kanten/Bewertungen-Delta) + unmittelbare Antwort.
4. Retention bei N = 0/50/200/500/1000 weiteren Nachrichten (fester,
   reproduzierbarer Ablenkungskorpus; Prüferage nur read-only; Zuordnung
   dazwischen nicht erneut trainieren).
5. Auswerten: Erwerbsrate, S(N), unbedingte Rate, erster Verlust,
   Wiederauf tauchen, Pfad-Änderungen, Zustands-Zusammenhang.

## EXPERIMENT 2 — Isolierter Sättigungsversuch (ohne Architekturänderung)
Identische Ausgangszustände; NUR der Sättigungszustand relevanter Konkurrenz-
verbindungen variiert (schwach / mittel / nahezu gesättigt). Neuerfahrung und
Ablauf identisch. Getrennt prüfen: gespeichert? unmittelbar ausgewählt? bleibt
erhalten? bleibt gespeichert obwohl nicht mehr ausgewählt?
Ziel: den TATSÄCHLICHEN Einfluss der Kantensättigung bestimmen — nicht
bestätigen wollen, dass sie blockiert.

## EXPERIMENT 3 — Isolierter Retentionsversuch episodische Schicht (NACH E1+E2)
Minimalvariante: Bestehender Wortgraph UND Lernmechanik unverändert; nur eine
sparsame Schicht, die beobachtete Frage-Antwort-Zuordnungen + Evidenz speichert
(keine eingebaute Antworten, keine externen Modelle, keine Embeddings).
Variante A (ohne) vs. B (mit), identische Erfahrungen. Prüfen: Erwerb, Retention,
Korrekturfähigkeit, neue Formulierungen, bloße Wiedererkennung.
EHRLICHKEIT: Besseres Erinnern exakter Fragen = gültiger Fortschritt, aber
KEIN Beweis für Generalisierung. Generalisierung braucht eigenen Nachweis
(s. 8: Verwechslung exakte Erinnerung / Generalisierung).

## EXPERIMENT 4 — Bewertung als eigener Einflussfaktor (Punkt 7)
Getrennt vergleichen: Einmal-Beobachtung ohne Bewertung / wiederholte Beobachtung
ohne / einmal mit plus / Minus auf Falsche + Korrektur / Kombination.
Bewertung und Korrektur getrennt protokollieren; Negativ ohne Alternative kennt
das Modell keine richtige Antwort. "Bewertung wirkt stärker als tausend
Wiederholungen" ist HYPOTHESE, prüfen, nicht behaupten.

## EXPERIMENT 5 — Vorhersage des Lernerfolgs (Punkt 8, wichtigstes Langfristziel)
Merkmale VOR Einführung einer Erfahrung: Sättigungsgrad, Anzahl konkurrierender
Pfade, Gewichtsunterschiede, Feedback-Evidenz, Nutzungshäufigkeit,
Kontextüberlappung, ungenutzte Fortsetzungen. Einfaches interpretierbares
Verfahren (kein ML-Modell, kein Bestandteil der Lernarchitektur).
Prüfung auf ungesehene Fälle + getrennte Ausgangszustände; KEINE Datenleckage
(spätere Trainings-/Prüfinfo darf nicht in die Merkmale). Bei Erfolg: Grundlage
fürn adaptive Trainingssteuerung (Punkt 9) — aber erst NACH Validierung.

## SPÄTER (punkt 9): Zustandsabhängige Trainingssteuerung
Wann lohnt Einführung/Wiederholung/Bewertung/Konkurrenzprüfung/Konsolidierung/
Stopp — nur mit empirischer Basis aus den Experimenten, nicht als Heuristik.

## REPORT-PFlicht (Punkt 11) — Fragen A–G mit Messdaten, Bedingungen,
Unsicherheiten:
A) Welcher Graphzustand → unmittelbarer Erwerb?
B) Welche Bedingungen → langfristige Abrufbarkeit?
C) Wann gespeichert, aber verdrängt (Abrufblockade)?
D) Wie stark ist Kantensättigung?
E) Welchen Mehrwert liefert Bewertung gegen Wiederholung?
F) Verbessert episodische Schicht die Retention wirklich?
G) Vorhersage aus dem Vorher-Zustand zuverlässig?

## UMSETZUNGS-VERANKERUNG (konkret für uns)
- Messinfrastruktur aus den 10k-Läufen wiederverwenden: Runner (training_run.rs
  10k-fähig), read-only-Inferenz mit Assert, fester Prüffragekatalog, Seed-
  Reproduzierbarkeit. Neigt: Experiment-Runner in examples/ (experiment_runner)
  mit Konfig (Ausgangs-Vault kopieren, Einführungsschritt definierbar, N-Raster
  0/50/200/500/1000, Zustandsaufnahme als JSON).
- Sättigungs-/Kontex-Spannen als Konfig (kein Anti-Hardcode-Verstoß: Werte in
  fixtures-Experiment-Konfig, nicht im Code).
- Werte aus Phase-2 (paar_basis 0.2/zuwachs 0.1/deckel 0.95) sind KANDIDATEN
  für E2, keine Annahme: erst im Code gegenprüfen (Paar-Parameter leben in
  fixtures/start.json — anti-hardcode-Konform).
- Achtung ChatGPT-Anmerkungen, die wir ABGEHAKT haben: implizit_positiv ist
  bereits in V01 ENTFERNT (Variante B aus seinem Ursprungsauftrag ist damit
  faktisch umgesetzt); trim()-Bug (erster Map-Schlüssel) ist UNBEWIESEN und muss
  in E1-Zustandsaufnahme berücksichtigt werden. Der Runner ist heute 10k-fähig,
  nicht 1k (ChatGPT-Kritik überholt).
- Report-Ablage: /home/lumen/projekt-jinx-netz/jinx-mesh/forschung/RW-1/

## STATUS
- Angelegt 06.11.2026. Start nach Publishing-GO 2.6.1. Phase-Planung (E1 zuerst)
  in eigener Session mit GO von Raphael.
