param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [Parameter(Mandatory=$true)][string]$OutputDirectory,
    [string]$Repository
)
$ErrorActionPreference='Stop'
$Binary=(Resolve-Path -LiteralPath $Binary).Path
$OutputDirectory=[IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
if (-not $Repository) {
    # A new, exclusively owned disposable directory; never reuse a user repo.
    $Repository=Join-Path ([IO.Path]::GetTempPath()) ('gitcrab-smoke-'+[guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $Repository | Out-Null
    $hooks=Join-Path $Repository 'empty-hooks'
    New-Item -ItemType Directory -Path $hooks | Out-Null
    function Invoke-SmokeGit([string[]]$GitArgs) {
        & git -C $Repository -c commit.gpgsign=false -c "core.hooksPath=$hooks" @GitArgs
        if ($LASTEXITCODE -ne 0) {throw "Fixture Git command failed: $($GitArgs -join ' ')"}
    }
    Invoke-SmokeGit @('init','--quiet','--initial-branch=main')
    Invoke-SmokeGit @('config','user.name','GitCrab Smoke')
    Invoke-SmokeGit @('config','user.email','smoke@example.invalid')
    Invoke-SmokeGit @('config','core.autocrlf','false')
    $text=((0..45 | ForEach-Object {"// Demo line $_"}) -join "`n")+"`n"
    [IO.File]::WriteAllText((Join-Path $Repository 'demo.rs'),$text)
    Invoke-SmokeGit @('add','--','demo.rs')
    Invoke-SmokeGit @('commit','--quiet','-m','Initialize smoke history')
    Invoke-SmokeGit @('switch','--quiet','-c','feature/graph')
    [IO.File]::WriteAllText((Join-Path $Repository 'feature.txt'),"Branch content`n")
    Invoke-SmokeGit @('add','--','feature.txt')
    Invoke-SmokeGit @('commit','--quiet','-m','Add graph feature')
    Invoke-SmokeGit @('switch','--quiet','main')
    [IO.File]::WriteAllText((Join-Path $Repository 'main.txt'),"Main content`n")
    Invoke-SmokeGit @('add','--','main.txt')
    Invoke-SmokeGit @('commit','--quiet','-m','Develop main branch')
    Invoke-SmokeGit @('merge','--no-edit','feature/graph')
    Invoke-SmokeGit @('tag','smoke-preview')
    $text=$text.Replace('// Demo line 2'+"`n",'// Staged edit'+"`n")
    [IO.File]::WriteAllText((Join-Path $Repository 'demo.rs'),$text)
    Invoke-SmokeGit @('add','--','demo.rs')
    [IO.File]::WriteAllText((Join-Path $Repository 'demo.rs'),$text.Replace('// Demo line 32'+"`n",'// Unstaged edit'+"`n"))
}
$Repository=(Resolve-Path -LiteralPath $Repository).Path
foreach ($view in @('history','staging','diff','split','commitdiff')) {
    $capture=Join-Path $OutputDirectory "$view.png"
    if (Test-Path -LiteralPath $capture) {throw "Capture already exists: $capture"}
    $start=[Diagnostics.ProcessStartInfo]::new($Binary)
    $start.UseShellExecute=$false
    $start.WindowStyle=[Diagnostics.ProcessWindowStyle]::Hidden
    $start.CreateNoWindow=$true
    $start.ArgumentList.Add($Repository)
    $start.Environment['GITCRAB_SCREENSHOT']=$capture
    $start.Environment['GITCRAB_CAPTURE_VIEW']=$view
    $start.Environment['GITCRAB_CAPTURE_EXIT']='1'
    $app=[Diagnostics.Process]::Start($start)
    if (-not $app.WaitForExit(30000)) {$app.Kill($true);throw "Native renderer timed out: $view"}
    if ($app.ExitCode -ne 0) {throw "Native renderer exited $($app.ExitCode): $view"}
    if (-not (Test-Path -LiteralPath $capture)) {throw "Native renderer did not save $capture"}
    $header=[IO.File]::ReadAllBytes($capture)
    if ($header.Length -lt 24 -or $header[0] -ne 137 -or [Text.Encoding]::ASCII.GetString($header,1,3) -ne 'PNG') {throw "Invalid capture: $capture"}
    Write-Output "Native $view capture passed: $($header.Length) bytes"
}
