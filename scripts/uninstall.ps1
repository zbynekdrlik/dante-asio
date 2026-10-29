# dante-asio: remove one instance (plan Task 9). Run as administrator:
#   .\uninstall.ps1 -Instance 1 -Ip <its address>
# Stops and removes its logon task, unregisters its driver entry, removes its
# address from the interface. The files in C:\Program Files\dante-asio stay
# (other instances use them); its data folder stays (the device's names).
param(
    [Parameter(Mandatory)][ValidateRange(1, 16)][int]$Instance,
    [Parameter(Mandatory)][string]$Ip
)
$ErrorActionPreference = "Stop"

$task = "dante-asio-$Instance"
$exe = Join-Path $env:ProgramFiles "dante-asio\dante-asio.exe"
if (Get-ScheduledTask -TaskName $task -ErrorAction SilentlyContinue) {
    if (Test-Path $exe) {
        # Asked to stop (never killed).
        & $exe stop --instance $Instance
        if ($LASTEXITCODE -ne 0) { throw "stop failed ($LASTEXITCODE)" }
        $deadline = (Get-Date).AddSeconds(15)
        while ((Get-ScheduledTask -TaskName $task).State -eq "Running") {
            if ((Get-Date) -gt $deadline) { throw "instance $Instance did not stop within 15 s" }
            Start-Sleep -Milliseconds 200
        }
    }
    Unregister-ScheduledTask -TaskName $task -Confirm:$false
}
if (Test-Path $exe) {
    & $exe unregister --instance $Instance
    if ($LASTEXITCODE -ne 0) { throw "unregister failed ($LASTEXITCODE)" }
}
if (Get-NetIPAddress -IPAddress $Ip -ErrorAction SilentlyContinue) {
    Remove-NetIPAddress -IPAddress $Ip -Confirm:$false
}
Write-Output "dante-asio instance $Instance removed"
