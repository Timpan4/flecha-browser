# Chromium 158 requires SDK 10.0.28000; the VS2026 hosted image only includes 26100.
$ErrorActionPreference = 'Stop'
if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
    throw 'SDK installation requires a disposable GitHub-hosted runner.'
}
$sdk = 'C:\Program Files (x86)\Windows Kits\10'
if (!(Test-Path "$sdk\Include\10.0.28000.0\um\Windows.h")) {
    $installer = Join-Path $env:RUNNER_TEMP 'winsdksetup.exe'
    # Microsoft's SDK bootstrapper, also used by ungoogled-chromium-windows.
    Invoke-WebRequest 'https://download.microsoft.com/download/38ab8f6d-3676-4860-ae84-3361308b9d7f/KIT_BUNDLE_WINDOWSSDK_MEDIACREATION/winsdksetup.exe' -OutFile $installer
    $signature = Get-AuthenticodeSignature $installer
    if ($signature.Status -ne 'Valid' -or $signature.SignerCertificate.Subject -notmatch 'O=Microsoft Corporation,') {
        throw 'Windows SDK installer has no valid Microsoft signature.'
    }
    $install = Start-Process $installer -ArgumentList '/features OptionId.DesktopCPPx64 OptionId.WindowsDesktopDebuggers /q /norestart /ceip off' -PassThru -Wait
    if ($install.ExitCode -ne 0) { throw "Windows SDK installer failed: $($install.ExitCode)" }
}
if (!(Test-Path "$sdk\Include\10.0.28000.0\um\Windows.h") -or !(Test-Path "$sdk\Debuggers\x64\dbghelp.dll")) {
    throw 'Windows SDK headers or x64 debugger tools are missing.'
}
