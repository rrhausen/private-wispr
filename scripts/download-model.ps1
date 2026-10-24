# Lädt das M0-Default-Modell (Parakeet TDT 0.6B v3, Q8_0) von HuggingFace
# und prüft SHA256 und Größe. Ziel: %LOCALAPPDATA%\PrivateWispr\models\
#
# Benutzung:  powershell -ExecutionPolicy Bypass -File scripts/download-model.ps1

$ErrorActionPreference = "Stop"

$Repo    = "handy-computer/parakeet-tdt-0.6b-v3-gguf"
$Rev     = "85ac09ea12fc4b1112fa76810059364bc6adc9de"
$File    = "parakeet-tdt-0.6b-v3-Q8_0.gguf"
$Sha256  = "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7"
$Size    = 739508576

$Dir = Join-Path $env:LOCALAPPDATA "PrivateWispr\models"
$Dest = Join-Path $Dir $File
New-Item -ItemType Directory -Force -Path $Dir | Out-Null

if ((Test-Path $Dest) -and (Get-Item $Dest).Length -eq $Size) {
    Write-Host "Modell existiert bereits: $Dest"
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
Write-Host "OK: $Dest ($([math]::Round($Size/1MB)) MB, SHA256 verifiziert)"
