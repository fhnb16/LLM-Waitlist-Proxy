<#
.SYNOPSIS
  Подпись Authenticode одного файла (exe, dll, установщик, деинсталлятор). Не зависит от
  поставщика сертификата.

.DESCRIPTION
  Способ подписи — переменные окружения (в CI их заполняют секреты репозитория):

  LMQ_SIGN_COMMAND       Своя команда поставщика в синтаксисе PowerShell: облачный HSM, Azure,
                         SSL.com, Certum и т. п. {file} заменяется путём к файлу в кавычках.
                         Команда — секрет: в журнал не выводится.
  LMQ_SIGN_PFX           Иначе — сертификат .pfx в base64 и пароль к нему
  LMQ_SIGN_PFX_PASSWORD  (LMQ_SIGN_PFX_PASSWORD). Подписывает signtool из Windows SDK.
  LMQ_SIGN_TIMESTAMP     Сервер меток времени RFC 3161. По умолчанию http://timestamp.digicert.com.
  SIGNTOOL               Путь к signtool.exe, если он не в Windows SDK.

  Скрипт вызывают Tauri (bundle.windows.signCommand: LMQ.exe, плагины NSIS, деинсталлятор,
  установщик) и CI (lmq-server.exe). Без настроек — ошибка: сюда приходят, только когда подпись
  включена. После подписи файл проверяется: нет подписи или она не сходится — ошибка.

.EXAMPLE
  pwsh -NoProfile -File scripts/sign.ps1 target/release/lmq-server.exe
#>
param([Parameter(Mandatory = $true)][string]$File)

$ErrorActionPreference = 'Stop'
$File = (Resolve-Path -LiteralPath $File).Path
$name = Split-Path $File -Leaf
$timestamp = if ($env:LMQ_SIGN_TIMESTAMP) { $env:LMQ_SIGN_TIMESTAMP } else { 'http://timestamp.digicert.com' }

function Find-SignTool {
  if ($env:SIGNTOOL -and (Test-Path -LiteralPath $env:SIGNTOOL)) { return $env:SIGNTOOL }
  $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $found = Get-ChildItem -Path $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.Directory.Name -eq 'x64' } |
    Sort-Object { [version]($_.Directory.Parent.Name -replace '[^\d.]', '') } -Descending |
    Select-Object -First 1
  if (-not $found) { throw 'signtool.exe не найден: установите Windows SDK или задайте SIGNTOOL' }
  return $found.FullName
}

# Сервер меток времени иногда не отвечает: до трёх попыток с паузой.
function Invoke-WithRetry([scriptblock]$Action) {
  for ($i = 1; $i -le 3; $i++) {
    $global:LASTEXITCODE = 0
    try {
      & $Action
      if ($LASTEXITCODE -eq 0) { return }
      $err = "код выхода $LASTEXITCODE"
    } catch {
      $err = $_.Exception.Message
    }
    if ($i -lt 3) {
      Write-Warning "подпись $name не удалась ($err), попытка $($i + 1) через $(5 * $i) с"
      Start-Sleep -Seconds (5 * $i)
    }
  }
  throw "подпись $name не удалась: $err"
}

if ($env:LMQ_SIGN_COMMAND) {
  $quoted = "'" + $File.Replace("'", "''") + "'"
  $command = $env:LMQ_SIGN_COMMAND.Replace('{file}', $quoted)
  Write-Output "Подпись $name командой поставщика"
  Invoke-WithRetry { Invoke-Expression $command }
} elseif ($env:LMQ_SIGN_PFX) {
  $signtool = Find-SignTool
  $pfx = Join-Path ([IO.Path]::GetTempPath()) ("lmq-sign-" + [guid]::NewGuid().ToString('N') + '.pfx')
  try {
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String(($env:LMQ_SIGN_PFX -replace '\s', '')))
    $stArgs = @('sign', '/fd', 'SHA256', '/f', $pfx, '/tr', $timestamp, '/td', 'SHA256', '/d', 'LMQ')
    if ($env:LMQ_SIGN_PFX_PASSWORD) { $stArgs += @('/p', $env:LMQ_SIGN_PFX_PASSWORD) }
    Write-Output "Подпись $name сертификатом .pfx"
    Invoke-WithRetry { & $signtool @stArgs $File | Out-Host }
  } finally {
    Remove-Item -LiteralPath $pfx -Force -ErrorAction SilentlyContinue
  }
} else {
  throw 'подпись включена, но не задан ни LMQ_SIGN_COMMAND, ни LMQ_SIGN_PFX'
}

$sig = Get-AuthenticodeSignature -LiteralPath $File
switch ($sig.Status) {
  'Valid' { Write-Output "$name подписан: $($sig.SignerCertificate.Subject)" }
  { $_ -in 'NotSigned', 'HashMismatch', 'Incompatible' } { throw "$name после подписи: $($sig.Status) — $($sig.StatusMessage)" }
  default { Write-Warning "$name подписан, но Windows не доверяет сертификату ($($sig.Status)): $($sig.StatusMessage)" }
}
