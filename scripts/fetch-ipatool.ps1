# Fetch pinned ipatool for the Windows sidecar. Idempotent. Usage: fetch-ipatool.ps1 [-DryRun]
# NOTE: not yet executed on Windows.
param([switch]$DryRun)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Lock = Join-Path $Root 'scripts\ipatool.lock'
$Out  = Join-Path $Root 'src-tauri\binaries'
$Base = 'https://github.com/majd/ipatool/releases/download'

$map = @{}
Get-Content -LiteralPath $Lock | Where-Object { $_ -match '^[A-Za-z0-9_]+=' } | ForEach-Object {
  $i = $_.IndexOf('='); $map[$_.Substring(0, $i)] = $_.Substring($i + 1).Trim()
}
$Version = $map['VERSION']
$Name    = $map['ARCHIVE_windows_amd64_NAME']
$ArcSha  = $map['ARCHIVE_windows_amd64_SHA256']
$BinPath = $map['BIN_windows_amd64_PATH']
$BinSha  = $map['BIN_windows_amd64_SHA256']
$Target  = Join-Path $Out 'ipatool-x86_64-pc-windows-msvc.exe'

function Get-Sha([string]$p) { (Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLower() }

if ((Test-Path -LiteralPath $Target) -and ((Get-Sha $Target) -eq $BinSha)) { Write-Output 'up to date'; exit 0 }
if ($DryRun) { Write-Output "dry-run: would download ipatool $Version into $Out"; exit 0 }

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $Tmp | Out-Null
try {
  $Arc = Join-Path $Tmp $Name
  Invoke-WebRequest -Uri "$Base/v$Version/$Name" -OutFile $Arc -UseBasicParsing
  if ((Get-Sha $Arc) -ne $ArcSha) { throw "sha256 mismatch: $Name" }
  & tar.exe -xzf $Arc -C $Tmp
  if ($LASTEXITCODE -ne 0) { throw 'tar extraction failed' }
  $Bin = Join-Path $Tmp ($BinPath -replace '/', '\')
  if ((Get-Sha $Bin) -ne $BinSha) { throw "sha256 mismatch: binary of $Name" }
  New-Item -ItemType Directory -Force -Path $Out | Out-Null
  Copy-Item -LiteralPath $Bin -Destination $Target -Force
  Write-Output "ipatool $Version installed into $Out"
} finally {
  Remove-Item -LiteralPath $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
