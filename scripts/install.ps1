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
    [Parameter(Mandatory)][string]$Config,
    # The name DAWs list the driver under (HKLM\SOFTWARE\ASIO), e.g. master-asio-dante.
    [string]$Name = ""
)
$ErrorActionPreference = "Stop"

$bin = Join-Path $env:ProgramFiles "dante-asio"
$data = Join-Path $env:ProgramData "dante-asio\instance-$Instance"
$task = "dante-asio-$Instance"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# The bundle's checksums first.
Get-Content (Join-Path $here "SHA256SUMS") | ForEach-Object {
    # Not $name: PowerShell names are case-blind, so it would overwrite -Name.
    $hash, $file = $_ -split "  ", 2
    $got = (Get-FileHash (Join-Path $here $file) -Algorithm SHA256).Hash.ToLower()
    if ($got -ne $hash) { throw "$file does not match SHA256SUMS" }
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
$named = @()
if ($Name) { $named = @("--name", $Name) }
& $exe register --instance $Instance --dll (Join-Path $bin "dante_asio_driver.dll") @named
if ($LASTEXITCODE -ne 0) { throw "register failed ($LASTEXITCODE)" }

# The logged-on user, machine-qualified (a bare name is refused); the task's
# process at normal priority (a task's default is below normal); its log in
# the instance's folder.
$user = "$env:COMPUTERNAME\$env:USERNAME"
$run = "run --instance $Instance --config `"$cfg`" --ip $Ip --log `"$(Join-Path $data 'dante-asio.log')`""
$action = New-ScheduledTaskAction -Execute $exe -Argument $run -WorkingDirectory $data
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -Priority 4 -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
$registered = Register-ScheduledTask -TaskName $task -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force
if (-not $registered) { throw "the task $task was not registered" }
Start-ScheduledTask -TaskName $task
$deadline = (Get-Date).AddSeconds(10)
while ((Get-ScheduledTask -TaskName $task).State -ne "Running") {
    if ((Get-Date) -gt $deadline) { throw "the task $task did not start" }
    Start-Sleep -Milliseconds 200
}
Write-Output "dante-asio instance $Instance installed and started"
