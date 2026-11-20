# Serylane-Benutzerhandbuch

Serylane installieren, ein eigenes Abonnement oder eine YAML-Datei verwenden und vom ersten Verbindungsaufbau bis zur Aktualisierung und Fehlersuche gelangen.

## Download und Installation
<!-- anchor: install -->

Lade die geprüfte stabile Version von der [Website](https://serylane.cmmuu.com/de/#download) oder aus dem [Downloadcenter](https://downloads.cmmuu.com/projects/serylane). Knoten, Abonnements und AI-Konten sind nicht enthalten.

- Windows: x64 für 64-Bit-PCs mit Intel oder AMD, ARM64 für Windows on Arm.
- macOS: Apple-Chip für die M-Serie, Intel für Intel-Macs. Ziehe die App aus dem DMG in Programme. Erforderlich ist macOS 13 oder neuer.
- Linux: Wähle das passende AppImage, mache es ausführbar und starte es.

## Schnellstart
<!-- anchor: quick-start -->

1. Füge in Abonnements einen Namen und eine Mihomo- oder Clash-Meta-Abonnement-URL hinzu. Öffne eine lokale YAML-Datei in den Profilen, prüfe sie und aktiviere das Profil.
2. Prüfe, dass das gewünschte Profil ausgewählt ist. Die Auswahl allein startet den Proxy nicht.
3. Aktiviere oben den Systemproxy. Öffne eine vertraute Website und prüfe in Verbindungen, ob die Anfrage den Kern erreicht und die erwartete Regel trifft.

## Proxy-Modus wählen
<!-- anchor: proxy-modes -->

- **Systemproxy**: für Apps, die die Proxy-Einstellungen des Betriebssystems lesen. Unter Linux wird GNOME gsettings verwendet.
- **TUN**: für Apps, die den Systemproxy ignorieren. Auf dem Mac braucht es den vollständigen signierten und von Apple notarisierten Installer und ggf. die Freigabe des Helfers in Anmeldeobjekte & Erweiterungen. Unter Windows ist TUN experimentell und erfordert Administratorrechte; unter Linux ist es nicht implementiert.
- **Lokaler Port**: Für das manuelle Proxy-Setup nutze `127.0.0.1` und den gemischten Port, per HTTP oder SOCKS5. Verwende nicht den Steuerport.

Ein bereiter Helfer belegt keine Verkehrsübernahme. Prüfe den Modus und eine echte Verbindung.

## Abonnements verwalten
<!-- anchor: subscriptions -->

Aktualisiere eine Karte oder alle Abonnements. Nutzung und Konfiguration werden separat aktualisiert; bei Fehlern bleibt die letzte nutzbare Konfiguration erhalten. Bearbeite eine Karte über Mehr oder per Rechtsklick und aktualisiere nach einem URL-Wechsel. Prüfe bei Fehlern den Downloadpfad und ob der Link eine Mihomo-Abonnement-URL statt einer Kontoseite ist.

## Knoten manuell wählen oder AI-Proxy verwenden
<!-- anchor: nodes -->

Beide Auswahlmodi schließen sich aus. Wähle manuell einen Knoten in einer Richtliniengruppe und miss die Latenz, oder nutze AI-Proxy mit 1–10 geeigneten Knoten. Ein Knoten hat keinen Ersatz. Wende die manuelle Auswahl an und warte auf ihre Bestätigung, oder stelle die automatische Auswahl wieder her. Änderungen betreffen neue Verbindungen.

## AI-Werkzeuge, Domain-Routing und Codex
<!-- anchor: ai-tools -->

Domain-Routing ist standardmäßig aus und betrifft nur Verbindungen im Regelmodus, die den Kern erreichen. Werkzeuge oder Domains und Ausgang wählen, Entwurf speichern, dann einschalten oder anwenden. `example.com` steht für die Hauptdomain, `*.example.com` nur für Subdomains. [Domain-Routing](https://serylane.cmmuu.com/de/docs/routing).

Unter macOS und Windows kann eine unterstützte Codex CLI mit `codex --enable respect_system_proxy` beim jeweiligen Start den vorhandenen Systemproxy lesen. Der Befehl aktiviert PAC nicht und ergänzt keinen automatischen Ersatzweg. Siehe [OpenAI-Dokumentation](https://learn.chatgpt.com/docs/enterprise/managed-configuration#use-the-system-proxy-for-native-client-requests) und prüfe eine tatsächlich abgeschlossene Anfrage.

## Updates prüfen und installieren
<!-- anchor: updates -->

Prüfe unter Einstellungen → Softwareupdate die Version und die tatsächliche Quelle. Der Updater versucht GitHub direkt, den GitHub-Beschleuniger, GitHub über den laufenden Serylane-Proxy und das Downloadcenter. Installiert wird erst nach Prüfung des Pakets für die gleiche Version und Architektur samt Signatur. Wenn der alte Updater scheitert, nutze den passenden vollständigen Installer und behalte die Konfiguration. [Prüfanleitung](https://serylane.cmmuu.com/de/docs/verification).

## Fehler beheben
<!-- anchor: troubleshooting -->

- App nicht über Proxy: Prüfe, ob Systemproxy oder TUN tatsächlich aktiv ist und ob die Anfrage in Verbindungen erscheint.
- macOS TUN wartet auf Freigabe: Prüfe die Anmeldeobjekte. Stoppe den Proxy vor einer angeforderten erneuten Zuordnung des Helfers.
- Port belegt: Sieh Port und Phase in den technischen Details nach. Warte nach Ende von TUN auf die Freigabe des Ports.
- HTTP 401 an einem AI-Endpunkt zeigt Erreichbarkeit, keine erfolgreiche Anmeldung oder Modellantwort.

Melde Version, OS, Architektur, Modus, Zeitpunkt und bereinigten Fehler, aber keine URLs mit Abonnementdaten oder Schlüssel.
