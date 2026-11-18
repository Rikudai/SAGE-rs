# Guarda as chaves de API e a memória do S.A.G.E. no cofre criptografado config\secrets.enc.
# O cofre pode ir para o GitHub: sem a senha ninguém lê. Depois rode o sage_publish (ou git push).
#
#   powershell -ExecutionPolicy Bypass -File scripts\chaves-exportar.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\chaves-exportar.ps1 -SenhaArquivo C:\caminho\senha.txt
param([string]$SenhaArquivo)

. "$PSScriptRoot\cofre.ps1"
$raiz = Get-SageRoot
$senha = Read-CofreSenha -SenhaArquivo $SenhaArquivo -Confirmar
$r = Protect-SageCofre -Raiz $raiz -Senha $senha
Write-Host "Cofre salvo em $($r.Caminho) com $($r.Arquivos.Count) arquivo(s):"
$r.Arquivos | ForEach-Object { Write-Host "  - $_" }
Write-Host 'No outro PC: clone o repositório e rode scripts\instalar.ps1 (pede a mesma senha).'
