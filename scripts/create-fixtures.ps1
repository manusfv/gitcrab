param([string]$OutputRoot = (Join-Path (Split-Path $PSScriptRoot -Parent) 'fixtures'))
$ErrorActionPreference = 'Stop'
$OutputRoot = [IO.Path]::GetFullPath($OutputRoot)
if (Test-Path -LiteralPath $OutputRoot) { throw "Fixture destination already exists; choose a new -OutputRoot. Nothing was overwritten: $OutputRoot" }
New-Item -ItemType Directory -Path $OutputRoot | Out-Null
$hooks = Join-Path $OutputRoot 'empty-hooks'
New-Item -ItemType Directory -Path $hooks | Out-Null

function Invoke-FixtureGit([string]$Repo, [string[]]$Arguments) {
    $result = & git -C $Repo -c commit.gpgsign=false -c "core.hooksPath=$hooks" @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($Arguments -join ' ') failed in ${Repo}: $result" }
    return $result
}
function Write-File([string]$Repo, [string]$Name, [string]$Text) {
    [IO.File]::WriteAllText((Join-Path $Repo $Name), $Text, [Text.UTF8Encoding]::new($false))
}
function New-Repo([string]$Name) {
    $repo = Join-Path $OutputRoot $Name
    New-Item -ItemType Directory -Path $repo | Out-Null
    Invoke-FixtureGit $repo @('init', '--quiet', '--initial-branch=main') | Out-Null
    Invoke-FixtureGit $repo @('config', 'user.name', 'GitCrab Fixture') | Out-Null
    Invoke-FixtureGit $repo @('config', 'user.email', 'fixture@example.invalid') | Out-Null
    Invoke-FixtureGit $repo @('config', 'core.autocrlf', 'false') | Out-Null
    return $repo
}
function Commit-All([string]$Repo, [string]$Message) {
    Invoke-FixtureGit $Repo @('add', '--all') | Out-Null
    Invoke-FixtureGit $Repo @('commit', '--quiet', '-m', $Message) | Out-Null
}

$repo = New-Repo 'showcase'
Write-File $repo 'README.md' "# Papertrail`nA disposable repository for exploring GitCrab.`n"
Write-File $repo 'app.rs' ((0..45 | ForEach-Object { "// Application line $_" }) -join "`n")
Write-File $repo 'legacy.txt' "Retired component`n"
[IO.File]::WriteAllBytes((Join-Path $repo 'image.bin'), [byte[]](0,1,2,3,0,255))
Commit-All $repo 'Initialize the Papertrail workspace'
$root = (Invoke-FixtureGit $repo @('rev-parse', 'HEAD')).ToString().Trim()
for ($i = 1; $i -le 8; $i++) {
    Write-File $repo 'iteration.txt' "Foundation iteration $i`n"
    Commit-All $repo "Refine workspace foundation $i"
}
$base = (Invoke-FixtureGit $repo @('rev-parse', 'HEAD')).ToString().Trim()
foreach ($feature in @('graph', 'search', 'staging')) {
    Invoke-FixtureGit $repo @('switch', '--quiet', '-c', "feature/$feature", $base) | Out-Null
    for ($i = 1; $i -le 6; $i++) {
        Write-File $repo "$feature.txt" "$feature implementation pass $i`n"
        Commit-All $repo "Improve $feature interaction $i"
    }
}
Invoke-FixtureGit $repo @('switch', '--quiet', 'main') | Out-Null
Write-File $repo 'release.txt' "Prepare release`n"
Commit-All $repo 'Prepare the next release'
Invoke-FixtureGit $repo @('merge', '--no-edit', 'feature/graph', 'feature/search', 'feature/staging') | Out-Null
Invoke-FixtureGit $repo @('tag', '-a', 'v0.2.0', '-m', 'Interaction preview') | Out-Null
foreach ($label in @('release/preview', 'release/candidate', 'review/graph-polish', 'backup/interaction')) {
    Invoke-FixtureGit $repo @('branch', $label) | Out-Null
}
Invoke-FixtureGit $repo @('switch', '--quiet', '-c', 'feature/diff') | Out-Null
for ($i = 1; $i -le 7; $i++) {
    Write-File $repo 'diff.txt' "Diff iteration $i`n"
    Commit-All $repo "Tune diff rendering $i"
}
Invoke-FixtureGit $repo @('switch', '--quiet', 'main') | Out-Null
for ($i = 1; $i -le 4; $i++) {
    Write-File $repo 'release.txt' "Release quality pass $i`n"
    Commit-All $repo "Address release feedback $i"
}
Invoke-FixtureGit $repo @('merge', '--no-edit', 'feature/diff') | Out-Null
Invoke-FixtureGit $repo @('mv', 'legacy.txt', 'archive.txt') | Out-Null
Commit-All $repo 'Rename the archived component'
Invoke-FixtureGit $repo @('switch', '--quiet', '--orphan', 'archive/unrelated') | Out-Null
Write-File $repo 'archive.md' "# Independent history`n"
Commit-All $repo 'Archive an unrelated project root'
Invoke-FixtureGit $repo @('switch', '--quiet', 'main') | Out-Null

# Separate repositories preserve detached, unborn and conflicted states for inspection.
$detached = New-Repo 'detached'
Invoke-FixtureGit $detached @('fetch', '--quiet', $repo, '+refs/heads/*:refs/remotes/fixture/*', '+refs/tags/*:refs/tags/*') | Out-Null
Invoke-FixtureGit $detached @('switch', '--quiet', '--detach', $root) | Out-Null
$unborn = New-Repo 'unborn'
Write-File $unborn 'first.txt' "Ready for an initial commit`n"
$conflict = New-Repo 'conflict'
Write-File $conflict 'shared.txt' "Original line`n"
Commit-All $conflict 'Start conflict fixture'
Invoke-FixtureGit $conflict @('switch', '--quiet', '-c', 'feature/change') | Out-Null
Write-File $conflict 'shared.txt' "Feature version`n"
Commit-All $conflict 'Change shared line on feature'
Invoke-FixtureGit $conflict @('switch', '--quiet', 'main') | Out-Null
Write-File $conflict 'shared.txt' "Main version`n"
Commit-All $conflict 'Change shared line on main'
$mergeResult = & git -C $conflict -c commit.gpgsign=false -c "core.hooksPath=$hooks" merge --no-edit feature/change 2>&1
if ($LASTEXITCODE -eq 0) { throw 'Expected conflict was not produced' }

# Leave simultaneous index and worktree changes in showcase.
$app = [IO.File]::ReadAllText((Join-Path $repo 'app.rs'))
Write-File $repo 'app.rs' ($app.Replace('// Application line 2', '// Staged interaction improvement'))
Invoke-FixtureGit $repo @('add', '--', 'app.rs') | Out-Null
$app = [IO.File]::ReadAllText((Join-Path $repo 'app.rs'))
Write-File $repo 'app.rs' ($app.Replace('// Application line 32', '// Unstaged rendering improvement'))
Invoke-FixtureGit $repo @('mv', 'archive.txt', 'history.txt') | Out-Null
Remove-Item -LiteralPath (Join-Path $repo 'iteration.txt')
[IO.File]::WriteAllBytes((Join-Path $repo 'image.bin'), [byte[]](0,4,5,6,0,254))
Write-File $repo 'notes untracked.txt' "Review the graph layout`n"

$summary = foreach ($name in @('showcase', 'detached', 'unborn', 'conflict')) {
    $path = Join-Path $OutputRoot $name
    $count = & git -C $path rev-list --all --count 2>$null
    [ordered]@{ name = $name; path = $path; commits = [int]($count | Select-Object -First 1); status = @(& git -C $path status --short) }
}
$summary | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputRoot 'manifest.json') -Encoding utf8
Write-Output "Created disposable fixtures in $OutputRoot"
$summary | ConvertTo-Json -Depth 5
