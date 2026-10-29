param(
    [Parameter(Mandatory = $true)][string]$BuildDir,
    [ValidateSet('Debug', 'Release')][string]$Configuration = 'Release'
)
$ErrorActionPreference = 'Stop'
cmake -S $PSScriptRoot -B $BuildDir -A x64
if ($LASTEXITCODE -ne 0) { throw 'CMake configuration failed.' }
cmake --build $BuildDir --config $Configuration
if ($LASTEXITCODE -ne 0) { throw 'Native build failed.' }
ctest --test-dir $BuildDir -C $Configuration --output-on-failure
if ($LASTEXITCODE -ne 0) { throw 'Native tests failed.' }
