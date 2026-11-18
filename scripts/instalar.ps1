# Prepara o S.A.G.E. num PC novo, de dentro do clone do repositório:
#   1. restaura as chaves e a memória do cofre (config\secrets.enc), pedindo a senha;
#   2. pega o sage.exe: compila com o Rust, se houver, ou baixa o pronto do release sage-latest (gh);
#   3. faz abrir com o Windows (como administrador, com -Admin) e abre o S.A.G.E.
#
# No PC novo:
#   winget install Git.Git GitHub.cli
#   gh auth login
#   gh repo clone Rikudai/SAGE-rs C:\SAGE
#   powershell -ExecutionPolicy Bypass -File C:\SAGE\scripts\instalar.ps1 -Admin
param([string]$SenhaArquivo, [switch]$Admin, [switch]$Compilar, [switch]$NaoAbrir)

. "$PSScriptRoot\cofre.ps1"
$raiz = Get-SageRoot
Set-Location $raiz

# 1. chaves
$config = Join-Path $raiz 'config\config.json'
$cofre = Join-Path $raiz 'config\secrets.enc'
if (Test-Path -LiteralPath $cofre) {
    if (Test-Path -LiteralPath $config) {
        Write-Host 'Já existe um config\config.json; restauro só o que faltar (a memória).'
        $senha = Read-CofreSenha -SenhaArquivo $SenhaArquivo
        $n = Unprotect-SageCofre -Raiz $raiz -Senha $senha -SemSobrescrever
    } else {
        $senha = Read-CofreSenha -SenhaArquivo $SenhaArquivo
        $n = Unprotect-SageCofre -Raiz $raiz -Senha $senha
    }
    Write-Host "Chaves e memória: $($n.Count) arquivo(s) restaurado(s)."
} elseif (-not (Test-Path -LiteralPath $config)) {
    Write-Warning 'Sem cofre e sem config\config.json: o S.A.G.E. abre sem chaves (configure em Configurações).'
}

# 2. executável
$exe = Join-Path $raiz 'sage.exe'
$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if ($Compilar -or ($cargo -and -not (Test-Path -LiteralPath $exe))) {
    if (-not $cargo) { throw 'O Rust (cargo) não está instalado. Instale com: winget install Rustlang.Rustup' }
    if (Test-Path 'C:\mingw64\bin') { $env:PATH = "C:\mingw64\bin;$env:PATH" }
    & cargo build --release -p sage
    if ($LASTEXITCODE -ne 0) { throw 'O build falhou.' }
    Copy-Item -LiteralPath (Join-Path $raiz 'target\release\sage.exe') -Destination $exe -Force
} elseif (-not (Test-Path -LiteralPath $exe)) {
    if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { throw 'Preciso do GitHub CLI: winget install GitHub.cli; gh auth login' }
    & gh release download sage-latest -p sage.exe -D $raiz --clobber
    if ($LASTEXITCODE -ne 0) { throw 'Não consegui baixar o sage.exe do release sage-latest.' }
}
Write-Host "Executável: $exe"

# 3. inicialização
if ($Admin) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot 'iniciar-como-admin.ps1') -AbrirAgora
} else {
    $atalho = Join-Path ([Environment]::GetFolderPath('Startup')) 'S.A.G.E..lnk'
    $s = (New-Object -ComObject WScript.Shell).CreateShortcut($atalho)
    $s.TargetPath = $exe; $s.WorkingDirectory = $raiz; $s.Save()
    if (-not $NaoAbrir) { Start-Process -FilePath $exe -WorkingDirectory $raiz }
}
Write-Host 'Pronto. O S.A.G.E. está instalado neste PC.'
