# CI (bundle job, windows-2025, administrator): the bundle installed as the PC
# installs it, run as its service, killed and brought back by the SCM, then
# removed. The device runs on the runner's routed IPv4 (install.ps1 adds no
# address: the interface has it) with the stack's test clock (no dantesync on
# a runner).
#   .\ci-service-test.ps1 -Bundle <staged bundle> -DeviceConfig <test-dante-device.toml>
param(
    [Parameter(Mandatory)][string]$Bundle,
    [Parameter(Mandatory)][string]$DeviceConfig
)
$ErrorActionPreference = "Stop"

$instance = 14
$service = "dante-asio-$instance"
$exe = Join-Path $env:ProgramFiles "dante-asio\dante-asio.exe"

function Wait-Until([scriptblock]$Condition, [int]$Seconds, [string]$What) {
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not (& $Condition)) {
        if ((Get-Date) -gt $deadline) { throw "$What not within $Seconds s" }
        Start-Sleep -Milliseconds 200
    }
}

function Get-ServicePid {
    (Get-CimInstance Win32_Service -Filter "Name='$service'").ProcessId
}

$route = Find-NetRoute -RemoteIPAddress 192.0.2.1 | Where-Object { $_.IPAddress } | Select-Object -First 1
$ip = $route.IPAddress
$alias = (Get-NetIPAddress -IPAddress $ip).InterfaceAlias
$prefix = (Get-NetIPAddress -IPAddress $ip).PrefixLength
Write-Output "runner address $ip on $alias /$prefix"

# The test device at 10 ms (a shared runner's scheduling), its state file in
# a writable folder.
$work = Join-Path $env:RUNNER_TEMP "dante-asio-service"
New-Item -ItemType Directory -Force $work | Out-Null
$state = (Join-Path $work "state.toml") -replace '\\', '/'
$text = (Get-Content $DeviceConfig) | Where-Object { $_ -notmatch '^state_file' } | ForEach-Object {
    if ($_ -match '^latency_ns') { 'latency_ns = 10000000' } else { $_ }
}
$text += "state_file = `"$state`""
$cfg = Join-Path $work "device.toml"
Set-Content -Path $cfg -Value $text

& (Join-Path $Bundle "install.ps1") -Instance $instance -InterfaceAlias $alias -Ip $ip -PrefixLength $prefix -Config $cfg -Name "ci-asio-dante" -TestClock
$svc = Get-CimInstance Win32_Service -Filter "Name='$service'"
if ($svc.StartMode -ne "Auto") { throw "start mode $($svc.StartMode), not Auto" }
if ($svc.StartName -ne "LocalSystem") { throw "account $($svc.StartName), not LocalSystem" }
$failure = (& sc.exe qfailure $service) -join "`n"
foreach ($delay in 2000, 5000, 30000) {
    if ($failure -notmatch "RESTART -- Delay = $delay") { throw "no restart after $delay ms:`n$failure" }
}
$flag = (& sc.exe qfailureflag $service) -join "`n"
if ($flag -notmatch "TRUE") { throw "no restart after an error exit:`n$flag" }

# The device drives a host on the driver: the probe through the service.
Wait-Until { Test-Path (Join-Path $env:ProgramData "dante-asio\instance-$instance\dante-asio.log") } 20 "the device's log"
Start-Sleep -Seconds 2
$probe = (& $exe probe --instance $instance --seconds 3) -join "`n"
if ($LASTEXITCODE -ne 0) { throw "probe failed ($LASTEXITCODE):`n$probe" }
Write-Output $probe
if ($probe -notmatch "switches (\d+)") { throw "no switches in the probe's line" }
$switches = [int]$Matches[1]
# 3 s at the test device's block (at most 256 frames at 96 kHz): over 1000.
if ($switches -lt 1000) { throw "only $switches switches in 3 s" }

# A crash: the SCM's recovery starts it again within its 2 s delay.
$before = Get-ServicePid
Stop-Process -Id $before -Force
Wait-Until { (Get-Service $service).Status -eq "Running" -and (Get-ServicePid) -ne $before -and (Get-ServicePid) -ne 0 } 15 "a restart after the kill"
Write-Output "restarted: pid $before -> $(Get-ServicePid)"

& (Join-Path $Bundle "uninstall.ps1") -Instance $instance -Ip $ip -Name "ci-asio-dante"
Wait-Until { -not (Get-Service -Name $service -ErrorAction SilentlyContinue) } 15 "the service's removal"
if (-not (Get-NetIPAddress -IPAddress $ip -ErrorAction SilentlyContinue)) { throw "uninstall removed the runner's own address" }
if (Test-Path "HKLM:\SOFTWARE\ASIO\ci-asio-dante") { throw "the driver's ASIO entry is still there" }
Get-Content (Join-Path $env:ProgramData "dante-asio\instance-$instance\dante-asio.log") | Select-Object -Last 15
Write-Output "service test passed"
