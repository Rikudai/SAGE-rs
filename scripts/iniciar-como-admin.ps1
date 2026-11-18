# Faz o S.A.G.E. abrir com o Windows já como administrador, sem a janela do UAC a cada vez:
# cria a tarefa agendada "S.A.G.E." (ao fazer logon, privilégios mais altos) e tira o atalho
# antigo da pasta Inicializar. Pede o UAC uma única vez, agora.
#
#   powershell -ExecutionPolicy Bypass -File scripts\iniciar-como-admin.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\iniciar-como-admin.ps1 -Desfazer
param([switch]$Desfazer, [switch]$AbrirAgora)

$ErrorActionPreference = 'Stop'
$raiz = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = Join-Path $raiz 'sage.exe'
$nome = 'S.A.G.E.'
$atalho = Join-Path ([Environment]::GetFolderPath('Startup')) 'S.A.G.E..lnk'

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    $args2 = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`"")
    if ($Desfazer) { $args2 += '-Desfazer' }
    if ($AbrirAgora) { $args2 += '-AbrirAgora' }
    $p = Start-Process -FilePath 'powershell.exe' -ArgumentList $args2 -Verb RunAs -Wait -PassThru
    exit $p.ExitCode
}

if ($Desfazer) {
    Unregister-ScheduledTask -TaskName $nome -Confirm:$false -ErrorAction SilentlyContinue
    $s = (New-Object -ComObject WScript.Shell).CreateShortcut($atalho)
    $s.TargetPath = $exe; $s.WorkingDirectory = $raiz; $s.Save()
    Write-Host 'Voltei ao atalho normal da pasta Inicializar.'
    exit 0
}

if (-not (Test-Path -LiteralPath $exe)) { throw "Não achei $exe. Compile ou rode scripts\instalar.ps1 antes." }
$usuario = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$acao = New-ScheduledTaskAction -Execute $exe -WorkingDirectory $raiz
$gatilho = New-ScheduledTaskTrigger -AtLogOn -User $usuario
$quem = New-ScheduledTaskPrincipal -UserId $usuario -LogonType Interactive -RunLevel Highest
$ajustes = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $nome -Action $acao -Trigger $gatilho -Principal $quem -Settings $ajustes -Description 'Abre o S.A.G.E. como administrador ao entrar no Windows.' -Force | Out-Null
if (Test-Path -LiteralPath $atalho) { Remove-Item -LiteralPath $atalho -Force }
Write-Host "Pronto: o S.A.G.E. abre como administrador ao entrar no Windows (tarefa '$nome')."
if ($AbrirAgora) { Start-ScheduledTask -TaskName $nome }
