# dante-asio: install or update one instance (plan Task 9). Run as administrator
# from the unpacked bundle:
#   .\install.ps1 -Instance 1 -InterfaceAlias Ethernet -Ip <free address> -PrefixLength 23 -Config <device.toml>
# - copies dante-asio.exe and dante_asio_driver.dll to C:\Program Files\dante-asio
#   (a running instance is stopped first through its scheduled task),
# - adds -Ip to the interface (SkipAsSource: the PC's own traffic keeps its address),
# - registers the driver for the instance (HKLM: its COM class and its ASIO entry),
# - puts the device config at C:\ProgramData\dante-asio\instance-<n>\device.toml,
# - makes the logon task "dante-asio-<n>" (the logged-on user, highest privileges)
#   and starts it.
param(
    [Parameter(Mandatory)][ValidateRange(1, 16)][int]$Instance,
    [Parameter(Mandatory)][string]$InterfaceAlias,
    [Parameter(Mandatory)][string]$Ip,
    [Parameter(Mandatory)][ValidateRange(8, 30)][int]$PrefixLength,
    [Parameter(Mandatory)][string]$Config
)
$ErrorActionPreference = "Stop"

$bin = Join-Path $env:ProgramFiles "dante-asio"
$data = Join-Path $env:ProgramData "dante-asio\instance-$Instance"
$task = "dante-asio-$Instance"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# The bundle's checksums first.
Get-Content (Join-Path $here "SHA256SUMS") | ForEach-Object {
    $hash, $name = $_ -split "  ", 2
    $got = (Get-FileHash (Join-Path $here $name) -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $hash) { throw "$name does not match SHA256SUMS" }
}

$installed = Join-Path $env:ProgramFiles "dante-asio\dante-asio.exe"
if ((Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) -and (Test-Path $installed)) {
    # Asked to stop (never killed): the device ends within its stop's 7 s.
    & $installed stop --instance $Instance
    if ($LASTEXITCODE -ne 0) { throw "stop failed ($LASTEXITCODE)" }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-ScheduledTask -TaskName $task).State -eq "Running") {
        if ((Get-Date) -gt $deadline) { throw "instance $Instance did not stop within 15 s; nothing changed" }
        Start-Sleep -Milliseconds 200
    }
}

New-Item -ItemType Directory -Force $bin, $data | Out-Null
Copy-Item (Join-Path $here "dante-asio.exe"), (Join-Path $here "dante_asio_driver.dll") $bin -Force
$cfg = Join-Path $data "device.toml"
Copy-Item $Config $cfg -Force

if (-not (Get-NetIPAddress -IPAddress $Ip -ErrorAction SilentlyContinue)) {
    New-NetIPAddress -InterfaceAlias $InterfaceAlias -IPAddress $Ip -PrefixLength $PrefixLength -SkipAsSource $true | Out-Null
}

$exe = Join-Path $bin "dante-asio.exe"
& $exe register --instance $Instance --dll (Join-Path $bin "dante_asio_driver.dll")
if ($LASTEXITCODE -ne 0) { throw "register failed ($LASTEXITCODE)" }

$args = "run --instance $Instance --config `"$cfg`" --ip $Ip"
$action = New-ScheduledTaskAction -Execute $exe -Argument $args -WorkingDirectory $data
$trigger = New-ScheduledTaskTrigger -AtLogOn
$principal = New-ScheduledTaskPrincipal -UserId "$env:USERDOMAIN\$env:USERNAME" -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName $task
Write-Output "dante-asio instance $Instance installed and started"
