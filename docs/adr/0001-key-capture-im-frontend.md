# Key-Capture lebt im Frontend (Webview), nicht im Rust-Hook

Der Hotkey-Key-Capture („Taste drücken …" in den Einstellungen) ist bewusst als Webview-Keydown-Zustandsautomat in `src/settings.tsx` implementiert; der frühere Rust-Pfad (Zustandsautomat im Low-Level-Hook + Commands + `pw://hotkey-captured`-Event) wurde am 22.11.2026 gelöscht, weil das Frontend ihn nie aufrief und die Logik dadurch doppelt gepflegt wurde (AltGr-Korrektur musste zweimal nachgezogen werden).

Entscheidung: Der beim App-Start erstellte Hook bleibt ausschließlich für die PTT-Kanten zuständig. Für den Capture ist er nicht zuverlässig erreichbar — die Hook-Installation ist in manchen Prozess-Kontexten eine Lotterie (Bug 25.10., Log-Stichwort „Hook-Selbsttest"), und echte Fenster-Tastenereignisse in der WebView sind dort trivial verfügbar, wo der Nutzer ohnehin hinzeigt (das Einstellungsfenster). Unerwünschter Nebeneffekt des alten Entwurfs war zudem, dass während des Captures die F24-Hook-Selbsttests aussetzen mussten.

Abgelehnte Alternative: Capture im Hook (Rust) — bei jeder Wartung (z. B. seitenspezifische Modifier, AltGr-Phantom-Strg links) müssten zwei Implementierungen synchron gehalten werden; ein Adapter auf jeder Seite war nie geplant.
