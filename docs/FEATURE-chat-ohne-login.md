# FEATURE: Chat ohne Anmeldung — "Immer antwortfähig" (Öffentlicher Bot-Pfad)
Auftrag: Raphael, 06.11.2026. Kontext: Jinx-Mesh soll künftig unseren Serverbot
im Frontend ersetzen. Der Bot MUSS immer antworten können — auch ohne dass ein
Nutzer (oder der Bot-Dienst) im Vault angemeldet ist. Die heutige /freeze-Funktion
setzt Login voraus und löst das nicht.

## ANFORDERUNG (bindungswirksam aus dem Auftrag)
1. Lesende Inferenz (Antwort geben) OHNE aktive Nutzer-Anmeldung.
2. Dauerhaft verfügbar im Dienstbetrieb (Frontend-Bot ersetzt Serverbot).
3. Antwortfähigkeit = immer; keine Anmelde-Hürde im Antwortpfad.
4. (Implizit aus Kap. 4 Sicherheits-Frame, prüfen bei Design:) Kunden-Eingaben
   lehren den Vault NICHT automatisch — Lernen bleibt Eigentümer-Sitzung oder
   explizit geschalteter Pfad.

## DESIGN-OPTIONEN (Jinx-Einschätzung, 06.11.2026)

### Option A — Wächter-Daemon mit In-Memory-Lesestand (EMPFEHLEN)
Der Eigentümer meldet sich EINMAL an (Dienststart / boot). Der Prozess hält die
Sitzung im Speicher und betreibt einen NUR-LESE-Pfad (freeze-äquivalent) für
alle externen Anfragen: Inferenz ohne Lernen, ohne Schreiben, ohne Freigabe des
Vaults an den Aufrufer. Schnittstelle wie beim bestehenden Qt-Graphfenster über
private Pipe (keine offenen Ports, keine Klartext-Graphdateien).
- Vault bleibt verschlüsselt auf Disk (Argon2id/XChaCha20 bleibt unangetastet).
- Antwor pfad ohne Anmelde-Check: der Daemon IST die Anmeldung.
- Crash-Verhalten ehrlich dokumentieren: nach Restart meldet der Eigentümer neu;
  bis dahin antwortet der Bot nicht (oder mit Status-Durchsage statt Stille).
- Passt zu Kap. 16 (Baby-Route/Dienstbetrieb) und dem privaten-Pipe-Muster.

### Option B — Unverschlüsselter öffentlicher Snapshot beim Export (NICHT empfehlen)
Beim Login wird eine rein lesende Modellkopie ohne Verschlüsselung exportiert,
die der Dienst ohne Anmeldegrenze lädt.
- NACHTEIL: Gelernte Inhalte liegen dann Klartext auf Disk — bricht das
  Vault-Versprechen (Kap. 4, Vertraulichkeit der Lerninhalte).
- Nur akzeptabel, wenn Raphael für den Bot-Kontext bewusst anders entscheidet.

### Option C — Separater öffentlicher Leseschlüssel im Vault (VERWERFEN)
Ein "public read key" ermöglicht /freeze ohne voll Anmeldung.
- NACHTEIL: Wer Datei+Schlüssel besitzt, liest ALLES (Archiv inklusive). Der
  Lese-Schutz wäre formal da, praktisch aber eine Tür mit Schlüssel im Schloss.
- Widerspricht dem bestehenden Vertraulichkeitsversprechen; kein Vorteil gegen-
  über Option A.

## EMPFEHLUNG
Option A. Zusätzlich klären (Raphael-Entscheidung NACH Fix-Welle):
- DARF der öffentliche Bot aus Kunden-Eingaben lernen? (Standard-Entwurf:
  NEIN — lernen nur im Eigentümer-DEV-Modus, Kap. 4/15 Erstquellen-Isolation.)
- Antwort-Tempo/Priorität vs. laufende Lern-Sitzungen (ein Store, ein Owner-
  serialization wie SlotDB — Lese- und Schreib-Sitzungen trennen).
- Beziehung zur geplanten SlotDB-Integration: Wenn ein Daemon eh resident ist,
  ist der Speicherpfad-Tausch (M2) ein nachgelagerter Schritt auf derselben
  Achse.

## Status
- ENTSCHEIDUNG RAPHAEL (06.11.2026): Option A GENEHMIGT — "Der Server läuft ja
  sowieso." Wächter-Daemon als Design-Richtung bestätigt.
- Begründung passt zur Realität: der VPS-Dienst läuft durgehend; die eine
  Eigentümer-Anmeldung passiert beim Dienststart (bzw. nach Restart), der
  Kundenpfad ist rein lesend.
- Umsetzung NACH Fix-Welle 2.6.1 + F03-Release (eigener Block, dann eigener GO).
- Offene Detailfragen für den Umsetzungsblock:
  1. Lernfähigkeit des Kundenpfads: Standard bleibt NEIN (nur lesend);
     Lernen nur Eigentümer-DEV.
  2. Antworten bei Daemon-Neustart vor Neuanmeldung: Status-Durchsage statt
     Stille (ehrlich, Kap-19-Stil).
  3. Schnittstelle: private Pipe analog Qt-Graphfenster; keine offenen Ports.
- Kein Code bis dahin. Entwurf dokumentiert.
