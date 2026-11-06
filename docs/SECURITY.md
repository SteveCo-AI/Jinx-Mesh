# Sicherheitsmodell

## Implementiert

- Ein lokaler Developer-Account je Vault; Nutzername plus Passwort, optional TOTP.
- Argon2id v19, 64 MiB, drei Durchläufe, ein Lane, 32-Byte-Schlüssel,
  zufälliger 16-Byte-Salt. Passwort wird nicht gespeichert.
- XChaCha20-Poly1305 mit zufälligem 24-Byte-Nonce je Speicherobjekt und
  zweck-/kontoabhängigen authentisierten Zusatzdaten. Audit zusätzlich mit Sequenz.
- Kompression vor Verschlüsselung. Begrenzte Entschlüsselung/Dekompression.
- TOTP: SHA1, sechs Stellen, 30 Sekunden, ±1 Schritt; bestätigte Einrichtung,
  persistierter letzter akzeptierter Zähler verhindert Wiederverwendung.
- Fehlversuche erzeugen persistierte exponentielle Pausen von zwei bis 60 Sekunden.
  Ein fehlgeschlagener erneuter Login beendet eine bisher aktive Sitzung.
- 30 Minuten Inaktivität, acht Stunden maximale Sitzung, explizites Logout.
- `/freeze` sperrt Schreibzugriffe und behält den Entschlüsselungsschlüssel für begrenztes Nachladen bei Inferenz im RAM. Die Lesebeschränkung wird durch die Anwendung durchgesetzt; es handelt sich nicht um einen kryptografisch getrennten Schlüssel. Zur Rückkehr zu Lernen braucht es neue Anmeldung.
- Anmeldedaten werden nicht der Engine übergeben, nicht in Deltas protokolliert
  und in der CLI nicht angezeigt. Rust-Schlüssel und Passwortpuffer werden mit
  zeroize verwaltet. Das Modell selbst wird beim Abmelden freigegeben, jedoch
  nicht garantiert aus jedem Speicherrest gelöscht.
- SQLite-Transaktionen plus exklusiver Anwendungs-Lock. Neue Vault-/Lock-Dateien
  bekommen unter Unix Modus 0600; privaten Elternordner verwenden.
- Bei Prüfung der Historie müssen alle Sequenzen lückenlos vorliegen, AEAD gültig
  sein und Replay exakt dem aktuellen Modell entsprechen.
- Format 3 hält Graphknoten in einzeln verschlüsselten Datensätzen. SQLite-Indizes
  enthalten HMAC-abgeleitete IDs statt Klartext-Wort-IDs. Der authentisierte
  Snapshot enthält Graph-/Archivzähler und Manifestwerte, keine Vollkopie des
  Graphen.
- Beim Login werden Manifeste und der Archiv-Sequenzbereich geprüft. Ein
  ausgewählter Graph- oder Archivdatensatz wird beim Lesen per AEAD geprüft.
  `/verify-graph` und `/verify-archive` scannen jeweils den gesamten Bestand
  streaming und mit begrenztem RAM; ihre I/O-Kosten wachsen mit der Datenmenge.
- Die Hintergrund-Vorladung lädt nur eine begrenzte Graphseite je Schritt und
  läuft nur mit aktiver Developer-Sitzung. `/freeze` stoppt die Hintergrundvorladung, erlaubt aber lesendes Nachladen bei Nachrichten. Logout und Sitzungsablauf
  verwerfen sämtliche Sitzungsschlüssel; Vorladung wird während verdeckter
  Passwort-/Authenticator-Eingaben pausiert.

## Präzise Grenzen

Die Verschlüsselung schützt Daten **auf dem Datenträger**. Während Lernen oder
Inferenz liegen Modellbestandteile entschlüsselt im RAM. Root-Zugriff,
Prozessinjektion, manipulierte Programmdateien, Keylogger, Debugger, Core Dumps
und ungeschützter Swap liegen außerhalb dieses Schutzes. OS-Berechtigungen und
Geräteschutz bleiben erforderlich. Es gibt keine Garantie vollständiger
Speicherbereinigung aller Graph-/Serde-/Kompressionsallokationen.

Bei rein lokalem TOTP liegt das 2FA-Geheimnis verschlüsselt im selben Vault. Wer
Vault **und Passwort** besitzt und eigene Software ausführt, kann den Inhalt
entschlüsseln und die lokale 2FA-Prüfung umgehen. Diese 2FA schützt den regulären
Anmeldeweg zusätzlich; für einen unabhängigen zweiten Entschlüsselungsfaktor
wären Hardware-Schlüssel oder ein separat vertrauenswürdiger Dienst erforderlich.

Das Zurückspielen einer vollständigen, älteren und authentischen Vault-Datei
kann lokal nicht zuverlässig erkannt werden. Dazu wäre ein extern verankerter
monotoner Zähler nötig. Das betrifft auch den gespeicherten TOTP-Replayschutz.
Das Ändern der unverschlüsselten Anmeldebremse setzt lokale Dateischreibrechte
voraus und wird nicht als eigenständige Sicherheitsgrenze betrachtet.

Metadaten wie Nutzername, Salt, Datenbankgröße, Anzahl/Größe der Änderungen und
Fehlversuchszähler sind sichtbar. Inhalte, Modelle und Geheimnisse sind verschlüsselt.
Diese Implementierung ist kein vollständig SQLCipher-verschlüsseltes Dateisystem.
Der lazy Login liest nicht alle verschlüsselten Datensätze. Vollständige Prüfung
steht explizit über `/verify-graph` und `/verify-archive` zur Verfügung. Ohne
solchen Scan werden Fehler in noch nicht ausgewählten Datensätzen erst beim
Zugriff auf den jeweiligen Datensatz erkannt.

Es existieren weder Netzwerklistener noch HTTP-Login noch Mehrbenutzerverwaltung.
`App` ist eine lokale Sitzung; ein späterer Server muss pro Nutzer isolierte
Sitzungen und getrennte öffentliche Inferenzpfade besitzen. Ein globaler
Developer-Schalter für alle Chatnutzer wäre unsicher. Keine Admin-API aus
unvertrauenswürdigen Texten aufrufen.

Passwortwechsel, nachträgliche 2FA-Verwaltung, Wiederherstellungsschlüssel,
Hardware-2FA und Accountverwaltung sind noch nicht implementiert. Ohne Passwort
ist der Vault nicht regulär entschlüsselbar. Backups nur bei geschlossenem
Programm als vollständige Kopie erstellen; Originalimporte geschützt behalten.
Kein Standardkonto und keine fest eingebauten Produktionszugangsdaten.

## Quellen für die verwendeten Bibliotheken

- https://docs.rs/argon2/0.5.3/argon2/ – Passwortbasierte Schlüsselableitung
- https://docs.rs/chacha20poly1305/0.10.1/chacha20poly1305/ – AEAD und Nonces
- https://docs.rs/totp-rs/5.7.2/totp_rs/ – Authenticator-Codes
- https://www.sqlite.org/atomiccommit.html – Transaktionen

Die eingesetzten Primitive stammen aus Bibliotheken. Diese Anwendung selbst hat
keinen unabhängigen Kryptografie-/Penetrationstest erhalten. Die mitgelieferten
Tests prüfen konkrete Invarianten und Fehlerpfade, keine vollständige Angriffsfläche.
