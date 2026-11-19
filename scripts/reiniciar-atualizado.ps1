# Reinicia o S.A.G.E. aplicando a versão nova, sem depender dele (roda fora do app).
#
#   -Compilar   compila (cargo build --release) e roda todos os testes antes (passando ou não)
#   -Novo X     instala o exe X já compilado e testado (o fluxo de auto-edição usa este)
#   -EsperarPid fecha esse processo do S.A.G.E. só quando a versão nova estiver pronta
#
# Depois: troca o sage.exe (guarda o anterior em sage.old.exe), reabre, espera o S.A.G.E.
# novo escrever data\saude.json e, se ele não abrir em -Prazo segundos, volta a versão
# anterior. O resultado fica em data\resultado_atualizacao.json, que o S.A.G.E. anuncia.
#
#   powershell -ExecutionPolicy Bypass -File scripts\reiniciar-atualizado.ps1 -Compilar
param([switch]$Compilar, [string]$Novo = '', [int]$EsperarPid = 0, [string]$Id = '', [int]$Prazo = 90)

$ErrorActionPreference = 'Continue'
$raiz = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$exe = Join-Path $raiz 'sage.exe'
$velho = Join-Path $raiz 'sage.old.exe'
$dados = Join-Path $raiz 'data'
$saude = Join-Path $dados 'saude.json'
$saida = Join-Path $dados 'resultado_atualizacao.json'
$pendente = Join-Path $dados 'atualizacao_pendente.json'
$log = Join-Path $raiz 'logs\atualizacao.log'
if (-not $Id) { $Id = 'upd-' + (Get-Date -Format 'yyyyMMdd-HHmmss') }
New-Item -ItemType Directory -Force -Path $dados, (Split-Path $log) | Out-Null

function Log([string]$t) {
    $linha = '{0} [reinício {1}] {2}' -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $Id, $t
    Add-Content -LiteralPath $log -Value $linha -Encoding UTF8
    Write-Host $linha
}

$res = [ordered]@{ id = $Id; status = ''; build = ''; testes = ''; resumo = ''; commit = ''; quando = ''; anunciado = $false }
if (Test-Path -LiteralPath $pendente) {
    try {
        $p = Get-Content -LiteralPath $pendente -Raw -Encoding UTF8 | ConvertFrom-Json
        $res.resumo = [string]$p.resumo; $res.testes = [string]$p.testes; $res.commit = [string]$p.commit
    } catch { }
    Remove-Item -LiteralPath $pendente -Force -ErrorAction SilentlyContinue
}

function Fim([string]$status) {
    $res.status = $status
    $res.quando = (Get-Date).ToString('o')
    if (-not $res.commit) { $res.commit = (& git -C $raiz rev-parse --short HEAD 2>$null) }
    [IO.File]::WriteAllText($saida, ($res | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))
    Log "fim: $status"
    exit 0
}

function Abrir {
    Remove-Item -LiteralPath $saude -Force -ErrorAction SilentlyContinue
    return Start-Process -FilePath $exe -WorkingDirectory $raiz -ArgumentList '--depois-de-atualizar' -PassThru
}

# Espera o S.A.G.E. novo dizer que abriu (data\saude.json com o pid dele).
function Conferir($proc) {
    $limite = (Get-Date).AddSeconds($Prazo)
    while ((Get-Date) -lt $limite) {
        Start-Sleep -Milliseconds 700
        if ($proc.HasExited) { return $false }
        if (Test-Path -LiteralPath $saude) {
            try {
                $s = Get-Content -LiteralPath $saude -Raw -Encoding UTF8 | ConvertFrom-Json
                if ([int]$s.pid -eq $proc.Id) { return $true }
            } catch { }
        }
    }
    return $false
}

# 1. Compila e testa (o S.A.G.E. atual segue aberto e respondendo enquanto isso).
if ($Compilar) {
    $env:PATH = "C:\mingw64\bin;$env:USERPROFILE\.cargo\bin;$env:PATH"
    Push-Location $raiz
    Log 'compilando (cargo build --release -p sage)'
    $b = & cargo build --release -p sage 2>&1 | Out-String
    $okBuild = ($LASTEXITCODE -eq 0)
    $res.build = if ($okBuild) { 'ok' } else { 'falhou' }
    if (-not $okBuild) {
        $erros = ($b -split "`n" | Where-Object { $_ -match '^error' } | Select-Object -First 5) -join ' | '
        $res.resumo = "A compilação falhou: $erros"
        Pop-Location
        Fim 'falhou_build'
    }
    Log 'rodando os testes (cargo test --workspace)'
    $t = & cargo test --workspace 2>&1 | Out-String
    Pop-Location
    $passou = 0; $falhou = 0; $nomes = @()
    foreach ($l in ($t -split "`n")) {
        if ($l -match 'test result: .*?(\d+) passed; (\d+) failed') { $passou += [int]$Matches[1]; $falhou += [int]$Matches[2] }
        elseif ($l -match '^test (.+) \.\.\. FAILED') { $nomes += $Matches[1].Trim() }
    }
    $res.testes = if ($falhou -eq 0) { "$passou testes passaram" } else { "$passou passaram, $falhou falharam ($(($nomes | Select-Object -First 5) -join ', '))" }
    Log "testes: $($res.testes)"
    $Novo = Join-Path $raiz 'target\release\sage.exe'
}
if (-not $Novo -or -not (Test-Path -LiteralPath $Novo)) { $res.resumo = "Não achei o exe novo ($Novo)."; Fim 'falhou_build' }

# 2. Fecha o S.A.G.E. atual (dá uns segundos para ele terminar de falar).
Start-Sleep -Seconds 4
if ($EsperarPid -gt 0) {
    $alvo = Get-Process -Id $EsperarPid -ErrorAction SilentlyContinue
    if ($alvo) { Stop-Process -Id $EsperarPid -Force -ErrorAction SilentlyContinue; $alvo.WaitForExit(15000) | Out-Null }
}
# Também o que estiver rodando como sage.old.exe (o sage_update antigo renomeia o exe em uso).
Get-Process -Name sage, sage.old -ErrorAction SilentlyContinue | Where-Object { $_.Path -in @($exe, $velho) } | ForEach-Object {
    Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue; $_.WaitForExit(10000) | Out-Null
}

# 3. Troca o exe, guardando o anterior.
if ((Resolve-Path -LiteralPath $Novo).Path -ne $exe) {
    $trocou = $false
    for ($i = 0; $i -lt 10 -and -not $trocou; $i++) {
        try {
            if (Test-Path -LiteralPath $exe) { Copy-Item -LiteralPath $exe -Destination $velho -Force -ErrorAction SilentlyContinue }
            Copy-Item -LiteralPath $Novo -Destination $exe -Force -ErrorAction Stop
            $trocou = $true
        } catch { Start-Sleep -Seconds 1 }
    }
    if (-not $trocou) { $res.resumo = 'O sage.exe ficou travado e não consegui trocar.'; Abrir | Out-Null; Fim 'revertido' }
    if ($Novo -like '*sage.new.exe') { Remove-Item -LiteralPath $Novo -Force -ErrorAction SilentlyContinue }
}

# 4. Reabre e confere; se não abrir, volta a versão anterior.
Log 'abrindo a versão nova'
$proc = Abrir
if (Conferir $proc) { Fim 'atualizado' }
Log 'a versão nova não abriu; voltando a anterior'
if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue; Start-Sleep -Seconds 2 }
if (Test-Path -LiteralPath $velho) { Copy-Item -LiteralPath $velho -Destination $exe -Force }
$proc2 = Abrir
if (Conferir $proc2) { Fim 'revertido' }
Fim 'falhou'
