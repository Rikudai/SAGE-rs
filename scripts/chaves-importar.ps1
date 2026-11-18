# Restaura as chaves de API (config\config.json) e a memória a partir do cofre config\secrets.enc.
#
#   powershell -ExecutionPolicy Bypass -File scripts\chaves-importar.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\chaves-importar.ps1 -SemSobrescrever
param([string]$SenhaArquivo, [switch]$SemSobrescrever)

. "$PSScriptRoot\cofre.ps1"
$raiz = Get-SageRoot
$senha = Read-CofreSenha -SenhaArquivo $SenhaArquivo
$gravados = Unprotect-SageCofre -Raiz $raiz -Senha $senha -SemSobrescrever:$SemSobrescrever
Write-Host "Restaurei $($gravados.Count) arquivo(s) do cofre."
