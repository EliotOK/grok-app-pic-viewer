$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '../src-tauri')
try {
    cargo test --lib --no-run --message-format=json | Set-Content test-binaries.jsonl
    if ($LASTEXITCODE -ne 0) { throw 'Rust test compilation failed' }
    $binaries = Get-Content test-binaries.jsonl | ForEach-Object {
        $entry = $_ | ConvertFrom-Json
        if ($entry.reason -eq 'compiler-artifact' -and $entry.profile.test -and $entry.executable) {
            $entry.executable
        }
    }
    if (-not $binaries) { throw 'No Rust test binary was produced' }
    $sdkBin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/bin'
    $mt = Get-ChildItem -LiteralPath $sdkBin -Recurse -Filter mt.exe |
        Where-Object { $_.FullName -match '\\x64\\mt\.exe$' } |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $mt) { throw 'Windows SDK manifest tool not found' }
    # Java distributions can shadow Windows API-set DLLs needed by the test host.
    $env:Path = (($env:Path -split ';') | Where-Object { $_ -notmatch '(?i)java|jdk|temurin' }) -join ';'
    foreach ($binary in $binaries) {
        & $mt.FullName -nologo -manifest windows-test-manifest.xml "-outputresource:$binary;#1"
        if ($LASTEXITCODE -ne 0) { throw 'Test manifest embedding failed' }
        & $binary 'mirror::' --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw 'Mirror tests failed' }
        & $binary 'app_update::' --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw 'Update tests failed' }
        & $binary 'acp_client::classify_rpc_error_tests' --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw 'Provider error classification tests failed' }
        & $binary 'error::tests' --test-threads=1
        if ($LASTEXITCODE -ne 0) { throw 'Error taxonomy tests failed' }
    }
} finally {
    Pop-Location
}
