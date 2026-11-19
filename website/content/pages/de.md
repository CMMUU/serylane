<!-- docs -->
# Einstieg und Plattformen

Wähle das passende offizielle Paket und verwende ein eigenes Abonnement oder eine Mihomo-Konfiguration. Hier findest du Funktionen, Grenzen und frühere Versionen.

## Erste Verbindung {#start}

Füge ein Abonnement hinzu und wähle es aus, oder importiere eine YAML-Datei in Profile. Aktiviere den Systemproxy und prüfe eine Verbindung. Die Schritte stehen im [Benutzerhandbuch](site:guide).

## Funktionsumfang {#current-capabilities}

Verwalte Abonnements und Knoten, prüfe Verbindungen, Protokolle und Diagnosen. AI-Proxy und manuelle Knotenwahl sind Alternativen. [Domain-Routing](site:routing) ist standardmäßig aus; neue Codex-Anbindungen und App-Proxys sind deaktiviert.

## TUN nach Plattform {#tun-compatibility}

macOS erfordert den vollständigen Developer-ID-signierten und von Apple notarisierten Installer und die Freigabe des Helfers. Windows TUN ist experimentell und benötigt Administratorrechte. Unter Linux gibt es kein TUN; der Systemproxy nutzt GNOME gsettings.

## Download und Architektur {#download-center}

Wähle auf der [Downloadseite](site:home#download) das System und x64 oder ARM64. Für Macs der M-Serie nutze das Paket für Apple-Chips. Maßgeblich ist die stabile Version im [Downloadcenter](https://downloads.cmmuu.com/projects/serylane).

## Ältere Veröffentlichungen {#history}

Frühere Nachweise stehen im [englischen Archiv](/docs/#history) oder [chinesischen Archiv (vereinfacht)](/zh/docs/#history). Historische Einträge sind keine Auskunft über aktuelle Downloads.

<!-- routing -->
# Ausgänge nach Domain festlegen

Sende passende Verbindungen, die bereits im Proxy-Kern ankommen, an eine gewählte Richtliniengruppe. Domain-Routing ist standardmäßig aus.

## Werkzeuge und eigene Domains {#node-metadata}

Nutze Vorgaben für OpenAI, Claude und Gemini oder füge Domains hinzu. `example.com` trifft die Hauptdomain; `*.example.com` trifft nur Subdomains. Das Abwählen eines Werkzeugs löscht eigene Einträge nicht.

## Speichern und Anwenden {#save-apply}

Wähle eine Gruppe des aktuellen Abonnements, speichere den Entwurf und bestätige Aktivieren oder Anwenden. Nicht passende Verbindungen behalten ihre Regeln. Fehlt die Gruppe nach einem Wechsel, wähle einen neuen Ausgang; es wird nicht automatisch direkt verbunden.

## Gemeinsame Bedienung {#global-controls}

Die Schaltfläche oben und die Routing-Seite benutzen denselben Zustand. Du kannst den Status prüfen, ohne den Proxy zu starten oder zwischen Systemproxy und TUN umzuschalten.

## Wirksamkeit prüfen {#enablement}

Prüfe Regelmodus, geladene Regeln und tatsächlichen Eingang. Das Speichern beweist keine Wirksamkeit. Beobachte eine neue Verbindung und ihren Ausgang.

## Anhalten und fortsetzen {#proxy-routing-state}

Ein Stopp des Kerns pausiert aktiviertes Routing, behält aber den Entwurf. Beim nächsten ausdrücklichen Start wird erneut geprüft; Routing allein startet den Proxy nicht.

## Knoten und Verlässlichkeit {#stability}

AI-Proxy mit 1–10 geeigneten Knoten und manuelle Knotenwahl sind Alternativen. Ein Knoten hat keinen Ersatz. Ein Wechsel betrifft nur neue Verbindungen.

## Grenzen {#validation-boundaries}

Die Regeln entschlüsseln kein TLS, unterscheiden keine verschlüsselten Pfade und ändern keine Codex-Konfiguration. Neue Codex-Anbindungen, App-Proxys und die frühere erweiterte Stabilitätsstrategie sind deaktiviert. [AI-Werkzeuge im Handbuch](site:guide#ai-tools).

<!-- verification -->
# Downloads und Updates prüfen

Prüfe die stabile Veröffentlichung, den genauen Dateinamen, die Architektur, den SHA-256-Wert und die Signatur. Die Berechtigungen des Betriebssystems sind davon getrennt.

## Downloadquelle {#download-origin}

Nutze die [Downloadseite](site:home#download) oder das [Downloadcenter](https://downloads.cmmuu.com/projects/serylane). GitHub-Quelllinks sind keine Ausweichquelle für Installationspakete.

## SHA-256 vergleichen {#checksums}

Vergleiche den Digest für den genauen Dateinamen mit `SHA256SUMS.txt` derselben Version. Bei Abweichung nicht installieren. Ersetze die Beispielnamen durch die echten Namen.

- Windows: `Get-FileHash -LiteralPath ./installer.exe -Algorithm SHA256`
- macOS: `shasum -a 256 ./installer.dmg`
- Linux: `sha256sum ./installer.AppImage`

## Signatur des Updates {#update-signatures}

Der Updater prüft die Projektsignatur. Für eine manuelle Prüfung verwende Signatur, Manifest und öffentlichen Schlüssel der passenden Veröffentlichung.

## Betriebssystemsignatur {#system-signing}

Die Projektsignatur ersetzt weder Windows Authenticode noch Apple Developer ID oder Notarisierung. Maßgeblich sind die ausdrücklichen Angaben zur jeweiligen Version.

## macOS TUN {#macos-tun}

Nutze den vollständigen signierten und notarisierten Installer. Meldet der alte Helfer `EX_CONFIG`, ist die alte Registrierung zu entfernen und der neue Helfer zu bestätigen. Erlaube ihn bei Bedarf in den Anmeldeobjekten und prüfe anschließend den tatsächlichen Datenverkehr.

## Unterbrochener Download {#update-downloads}

Die Update-Wege sind GitHub direkt, der Beschleuniger, GitHub über den laufenden Serylane-Proxy und das Downloadcenter. Ein Ersatzweg muss dieselbe Version, Architektur, Größe, Prüfsumme und Signatur liefern.

## Installation fehlgeschlagen {#update-installation}

Prüfe Phase, Speicherplatz und Berechtigungen. Falls ein alter Updater wiederholt scheitert, verwende den passenden vollständigen Installer am gleichen Speicherort und behalte die Konfiguration.

## Frühere Prüfungen {#verification-history}

Siehe das [englische Archiv](/docs/verification#verification-history) oder [chinesische Archiv (vereinfacht)](/zh/docs/verification#verification-history).

<!-- privacy -->
# Datenschutz und Datenverarbeitung

Website, Downloadcenter und Desktop-Client sind getrennte Systeme. Das Lesen der Website startet keinen Proxy und verbindet Codex nicht.

## Verhalten der Website {#website-data}

Die Website liefert statische Seiten und öffentliche Versionsinformationen. Sie hat keine Konten, Analyse oder Werbetracker, fordert keine Abonnement-URLs oder Geheimnisse an und kontaktiert keinen lokalen Proxy.

## Lokale Einstellung {#theme-preference}

Der Browser speichert nur `serylane-theme`. Das Design wird nicht an Server gesendet. Die Sprache steht in der URL; es gibt keine gespeicherte Sprachwahl oder Tracking-Cookies.

## Hosting {#hosting}

Für Auslieferung und Sicherheit werden grundlegende Zugriffsdaten wie IP-Adresse, Zeit, Pfad und Browser verarbeitet.

## Downloads und externe Links {#downloads}

Der Download prüft öffentliche Versionsdaten und die passende Datei. Cookies oder Autorisierungsdaten werden nicht weitergeleitet. Externe Dienste und der Dateiserver verarbeiten Anfragen separat. [Prüfhinweise](site:verification).

## Desktop-Client {#desktop-client}

Abo-Konfigurationen und Domain-Regeln sind Funktionen des Clients. Domain-Routing ist standardmäßig aus und entschlüsselt kein TLS. Entferne Zugangsdaten aus Abonnement-URLs vor dem Teilen.

## Probleme melden {#reporting}

Melde Fehler im [GitHub-Projekt](https://github.com/CMMUU/serylane/issues), ohne Passwörter, Tokens, Links oder Rohkonfiguration zu veröffentlichen. Prüfe bei Sicherheitslücken zuerst den privaten Meldeweg.
