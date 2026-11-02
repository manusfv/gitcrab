param([string]$Destination = (Join-Path (Split-Path $PSScriptRoot -Parent) 'fixtures\large'))
$ErrorActionPreference = 'Stop'
$Destination = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $Destination) { throw "Destination already exists; nothing overwritten: $Destination" }
New-Item -ItemType Directory -Path $Destination | Out-Null
& git -C $Destination init --quiet --initial-branch=main
if ($LASTEXITCODE -ne 0) { throw 'Git initialization failed' }

# Generate objects directly through fast-import. No shell interprets the stream.
$start = [Diagnostics.ProcessStartInfo]::new('git')
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$start.RedirectStandardInput = $true
$start.RedirectStandardOutput = $true
$start.RedirectStandardError = $true
$start.StandardInputEncoding = [Text.ASCIIEncoding]::new()
foreach ($argument in @('-C', $Destination, 'fast-import', '--quiet', '--date-format=raw')) {
    $start.ArgumentList.Add($argument)
}
$process = [Diagnostics.Process]::new()
$process.StartInfo = $start
if (!$process.Start()) { throw 'Could not start Git fast-import' }
$stdout = $process.StandardOutput.ReadToEndAsync()
$stderr = $process.StandardError.ReadToEndAsync()
try {
    for ($i = 1; $i -le 10000; $i++) {
        $timestamp = 1704067200L + (60L * $i)
        $message = "Benchmark commit $i"
        $body = "Payload revision $i`n"
        $process.StandardInput.Write("commit refs/heads/main`nmark :$i`ncommitter GitCrab Benchmark <benchmark@example.invalid> $timestamp +0000`ndata $($message.Length)`n$message`n")
        if ($i -gt 1) { $process.StandardInput.Write("from :$($i - 1)`n") }
        $process.StandardInput.Write("M 100644 inline payload.txt`ndata $($body.Length)`n$body`n")
    }
    for ($i = 1; $i -le 20; $i++) {
        $name = 'refs/heads/review/{0:D2}' -f $i
        $mark = $i * 450
        $process.StandardInput.Write("reset $name`nfrom :$mark`n`n")
    }
    $process.StandardInput.Write("done`n")
} finally {
    $process.StandardInput.Close()
}
$process.WaitForExit()
$errorText = $stderr.GetAwaiter().GetResult()
$null = $stdout.GetAwaiter().GetResult()
if ($process.ExitCode -ne 0) { throw "Git fast-import failed: $errorText" }

# Materialize the solely generated fixture. Destination was created exclusively
# above and an existing destination is always rejected before any Git operation.
& git -C $Destination reset --hard --quiet HEAD
if ($LASTEXITCODE -ne 0) { throw 'Could not materialize fixture worktree' }
$count = & git -C $Destination rev-list --all --count
$branches = @(& git -C $Destination for-each-ref '--format=%(refname)' refs/heads)
if ([int]$count -ne 10000 -or $branches.Count -ne 21) { throw 'Fixture identity check failed' }
Write-Output "Created $Destination : 10000 linear commits, 21 branches, one tracked text file, clean worktree."
