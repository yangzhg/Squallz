param(
    [string]$Archive = "",
    [string]$Password = "squallz-credential-validation-secret"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -Scope Global -ErrorAction SilentlyContinue) {
    $Global:PSNativeCommandUseErrorActionPreference = $false
}

$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Report = Join-Path $Root "benches/WINDOWS_CREDENTIAL_MANAGER_SMOKE.md"
$Work = Join-Path $Root "target/squallz-windows-credential-validation"
$TestLog = Join-Path $Work "test.log"
$TestErrLog = Join-Path $Work "test.stderr.log"
$Service = "com.squallz.archive-password"

New-Item -ItemType Directory -Force -Path (Join-Path $Root "benches") | Out-Null

function Write-SmokeReport {
    param(
        [string]$Status,
        [string]$Result,
        [string]$ArchivePath = "",
        [string]$TargetName = ""
    )

    $Generated = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    @"
# Squallz Windows Credential Manager Smoke

Generated: $Generated

## Scope

This smoke check runs Squallz's real Windows `SecretStore` backend against
Windows Credential Manager, using a throwaway archive path and a non-user
password.

## Inputs

- Archive account path: $ArchivePath
- Credential target: $TargetName
- Test log: $TestLog

## Checks

- Existing test credential for the archive target is deleted before the run.
- `WindowsCredentialManagerSecretStore::set_archive_password` writes a generic credential.
- `has_archive_password` reports the item as saved.
- `get_archive_password` reads the saved password back through Squallz's `Password` wrapper.
- `delete_archive_password` removes the item.
- A direct `cmdkey /list` check confirms no test credential remains.

## Result

Status: $Status

$Result
"@ | Set-Content -Encoding UTF8 -Path $Report
}

function Stop-Blocked {
    param([string]$Message)
    Write-SmokeReport -Status "blocked" -Result $Message
    Write-Error "windows_credential_manager_smoke: blocked: $Message"
    exit 2
}

function Stop-Failed {
    param(
        [string]$Message,
        [string]$ArchivePath,
        [string]$TargetName
    )
    Write-SmokeReport -Status "failed" -Result $Message -ArchivePath $ArchivePath -TargetName $TargetName
    Write-Error "windows_credential_manager_smoke: $Message"
    exit 1
}

$IsWindowsHost = [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
    [System.Runtime.InteropServices.OSPlatform]::Windows
)
if (-not $IsWindowsHost) {
    Stop-Blocked "this smoke check only runs on Windows"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Stop-Blocked "missing cargo on PATH"
}
if (-not (Get-Command cmdkey.exe -ErrorAction SilentlyContinue)) {
    Stop-Blocked "missing cmdkey.exe on PATH"
}

New-Item -ItemType Directory -Force -Path $Work | Out-Null
if ([string]::IsNullOrWhiteSpace($Archive)) {
    $Archive = Join-Path $Work "credential smoke #1.7z"
}
$ArchivePath = [System.IO.Path]::GetFullPath($Archive)
Set-Content -Encoding UTF8 -Path $ArchivePath -Value "credential manager smoke placeholder"
$TargetName = "${Service}:archive:${ArchivePath}"

function Remove-TestCredential {
    & cmdkey.exe "/delete:$TargetName" *> $null
}

try {
    Remove-TestCredential

    Push-Location $Root
    try {
        $env:SQUALLZ_CREDENTIAL_VALIDATION = "1"
        $env:SQUALLZ_CREDENTIAL_VALIDATION_ARCHIVE = $ArchivePath
        $env:SQUALLZ_CREDENTIAL_VALIDATION_PASSWORD = $Password
        Remove-Item -LiteralPath $TestLog, $TestErrLog -ErrorAction SilentlyContinue
        $CargoArgs = @(
            "test",
            "-p",
            "squallz-gui",
            "secrets::tests::windows_credential_manager_write_read_delete_validation",
            "--",
            "--ignored",
            "--exact",
            "--nocapture"
        )
        $CargoProcess = Start-Process `
            -FilePath "cargo" `
            -ArgumentList $CargoArgs `
            -WorkingDirectory $Root `
            -NoNewWindow `
            -Wait `
            -PassThru `
            -RedirectStandardOutput $TestLog `
            -RedirectStandardError $TestErrLog
        $TestStatus = $CargoProcess.ExitCode
        if (Test-Path -LiteralPath $TestLog) {
            Get-Content -Path $TestLog
        }
        if (Test-Path -LiteralPath $TestErrLog) {
            Get-Content -Path $TestErrLog | Add-Content -Path $TestLog
            Get-Content -Path $TestErrLog
        }
    } finally {
        Pop-Location
        Remove-Item Env:SQUALLZ_CREDENTIAL_VALIDATION -ErrorAction SilentlyContinue
        Remove-Item Env:SQUALLZ_CREDENTIAL_VALIDATION_ARCHIVE -ErrorAction SilentlyContinue
        Remove-Item Env:SQUALLZ_CREDENTIAL_VALIDATION_PASSWORD -ErrorAction SilentlyContinue
    }

    if ($TestStatus -ne 0) {
        Stop-Failed "Windows Credential Manager ignored test failed; see $TestLog" $ArchivePath $TargetName
    }

    $List = & cmdkey.exe /list 2>&1
    if ($List | Select-String -SimpleMatch $TargetName) {
        Stop-Failed "test credential was not deleted" $ArchivePath $TargetName
    }

    Write-SmokeReport -Status "pass" -Result "Passed." -ArchivePath $ArchivePath -TargetName $TargetName
    Write-Output "report=$Report"
    Write-Output "log=$TestLog"
} finally {
    Remove-TestCredential
}
