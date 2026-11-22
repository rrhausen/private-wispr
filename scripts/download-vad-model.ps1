# Lädt das Silero-VAD-Modell (v6.2, F32) für transcribe.cpp von HuggingFace
# und prüft SHA256 und Größe (gleiche Disziplin wie download-model.ps1).
# Standard-Ziel: %LOCALAPPDATA%\PrivateWispr\models\ — mit -OutDir z. B. auch
# für Paket-Builds (portable-zip.ps1 / NSIS-Resource).
#
# Benutzung:  powershell -ExecutionPolicy Bypass -File scripts/download-vad-model.ps1
#             powershell ... -OutDir <verzeichnis>

param(
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"

$Repo    = "handy-computer/silero-vad-v6.2-gguf"
$Rev     = "a9b2d61c20ae4ddf3644edb34969e8185a485448"
$File    = "silero-vad-v6.2-F32.gguf"
$Sha256  = "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6"
$Size    = 1241056

if (-not $OutDir) {
    $OutDir = Join-Path $env:LOCALAPPDATA "PrivateWispr\models"
}
$Dest = Join-Path $OutDir $File
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

if ((Test-Path $Dest) -and (Get-Item $Dest).Length -eq $Size) {
    Write-Host "VAD-Modell existiert bereits: $Dest"
} else {
    $Url = "https://huggingface.co/$Repo/resolve/$Rev/$File"
    Write-Host "Lade $Url ..."
    $Tmp = "$Dest.part"
    Invoke-WebRequest -Uri $Url -OutFile $Tmp
    Move-Item -Force $Tmp $Dest
}

$Hash = (Get-FileHash -Algorithm SHA256 $Dest).Hash.ToLower()
if ($Hash -ne $Sha256) {
    throw "SHA256-Mismatch! Erwartet $Sha256, bekommen $Hash — Datei löschen und erneut versuchen."
}
Write-Host "OK: $Dest ($([math]::Round($Size/1MB, 1)) MB, SHA256 verifiziert)"
