# Baut das portable Verteil-ZIP aus dem Release-Build (Specs: „NSIS + ZIP").
# Vorher: bun run tauri build (CMake im PATH, s. AGENTS.md).
# Aufruf:  pwsh scripts/portable-zip.ps1 [-Version 0.1.1]
# Ergebnis: src-tauri\target\release\bundle\Private Wispr_<Version>_portable_x64.zip
param(
    [string]$Version = "0.1.1"
)
$ErrorActionPreference = "Stop"

$release = Join-Path $PSScriptRoot "..\src-tauri\target\release" | Resolve-Path
$exe = Join-Path $release "private-wispr.exe"
if (-not (Test-Path $exe)) {
    throw "Release-Build fehlt — zuerst 'bun run tauri build' ausführen."
}

$stageRoot = Join-Path $release "bundle\portable"
$stage = Join-Path $stageRoot "PrivateWispr"
if (Test-Path $stageRoot) { Remove-Item $stageRoot -Recurse -Force }
New-Item -ItemType Directory $stage | Out-Null

# exe + alle DLLs (Engine-Backends ggml-*.dll, transcribe.dll, …)
Copy-Item $exe $stage
Copy-Item (Join-Path $release "*.dll") $stage

$liesmich = @"
Private Wispr $Version — portabel

1) Diesen Ordner an einen festen Ort entpacken (z. B. C:\Tools\PrivateWispr).
   Der Ordner darf danach nicht mehr verschoben werden, wenn Autostart
   aktiviert wird.
2) private-wispr.exe starten.
3) Modell: entweder die Datei parakeet-tdt-0.6b-v3-Q8_0.gguf einfach
   in den entpackten Ordner legen (neben private-wispr.exe — wird
   automatisch gefunden) oder über Einstellungen -> Modell herunterladen
   (705 MB, landet dann unter %LOCALAPPDATA%\PrivateWispr\models\).

Bedienung (Kurzform):
- Strg+Alt+Leertaste halten, sprechen, loslassen -> Text am Cursor.
- Strg+Alt+H halten -> Diktat in die Hand-off-Tagesdatei.
- Escape bricht die Aufnahme ab. X minimiert in den Tray; Beenden im
  Tray-Menü.

Voraussetzungen: Windows 11 (WebView2 ab Werk), Vulkan-fähige GPU
(NVIDIA-Treiber bringt Vulkan mit). Ohne Vulkan läuft die Engine auf der
CPU — langsamer, aber funktional. Einstellungen/Modell/Historie liegen
pro Rechner unter %LOCALAPPDATA%\PrivateWispr\ bzw. %APPDATA%\PrivateWispr\.
Log (bei Problemen): %LOCALAPPDATA%\de.rrhausen.private-wispr\logs\.
"@
$liesmich | Out-File -FilePath (Join-Path $stage "LIESMICH.txt") -Encoding utf8

$zip = Join-Path $release "bundle\Private Wispr_${Version}_portable_x64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Host "ZIP fertig: $zip"
Get-ChildItem $stage | Format-Table Name, Length -AutoSize
