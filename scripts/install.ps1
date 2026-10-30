# dante-asio: install or update one instance (plan Task 9). Run as administrator
# from the unpacked bundle:
#   .\install.ps1 -Instance 1 -InterfaceAlias Ethernet -Ip <free address> -PrefixLength 23 -Config <device.toml>
# - copies dante-asio.exe and dante_asio_driver.dll to C:\Program Files\dante-asio
#   (a running instance is stopped first: its service, or the logon task an
#   older install made, which is removed),
# - adds -Ip to the interface (SkipAsSource: the PC's own traffic keeps its address),
# - registers the driver for the instance (HKLM: its COM class and its ASIO entry),
# - puts the device config at C:\ProgramData\dante-asio\instance-<n>\device.toml,
# - makes the service "dante-asio-<n>" (LocalSystem, started at boot before any
#   logon; the SCM restarts it after 2 s, 5 s, then 30 s, also after an error
#   exit, the count reset after a day) and starts it.
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
$service = "dante-asio-$Instance"
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
if (Get-Service -Name $service -ErrorAction SilentlyContinue) {
    # The SCM's Stop: the device ends within its stop's 7 s.
    Stop-Service -Name $service
    (Get-Service -Name $service).WaitForStatus("Stopped", [TimeSpan]::FromSeconds(15))
}
if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) {
    # An older install's logon task: asked to stop (never killed), then removed.
    if (Test-Path $installed) {
        & $installed stop --instance $Instance
        if ($LASTEXITCODE -ne 0) { throw "stop failed ($LASTEXITCODE)" }
    }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-ScheduledTask -TaskName $task).State -eq "Running") {
        if ((Get-Date) -gt $deadline) { throw "instance $Instance did not stop within 15 s; nothing changed" }
        Start-Sleep -Milliseconds 200
    }
    Unregister-ScheduledTask -TaskName $task -Confirm:$false
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

$log = Join-Path $data "dante-asio.log"
$bin_path = "`"$exe`" service --instance $Instance --config `"$cfg`" --ip $Ip --log `"$log`""
$existing = Get-CimInstance Win32_Service -Filter "Name='$service'"
if ($existing) {
    # Through CIM: the path's quotes reach the SCM unchanged (sc.exe's
    # argument parsing would split them).
    $r = $existing | Invoke-CimMethod -MethodName Change -Arguments @{ PathName = $bin_path; StartMode = "Automatic" }
    if ($r.ReturnValue -ne 0) { throw "changing $service failed ($($r.ReturnValue))" }
} else {
    New-Service -Name $service -BinaryPathName $bin_path -DisplayName "dante-asio instance $Instance" -StartupType Automatic | Out-Null
}
& sc.exe failure $service reset= 86400 actions= restart/2000/restart/5000/restart/30000 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc failure $service failed ($LASTEXITCODE)" }
# Restart after an error exit too, not only after a crash.
& sc.exe failureflag $service 1 | Out-Null
if ($LASTEXITCODE -ne 0) { throw "sc failureflag $service failed ($LASTEXITCODE)" }
Start-Service -Name $service
(Get-Service -Name $service).WaitForStatus("Running", [TimeSpan]::FromSeconds(10))
Write-Output "dante-asio instance $Instance installed and started"
