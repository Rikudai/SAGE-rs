# Instala a voz local do S.A.G.E. (sem API externa): ambiente Python em .venv-voz, o motor
# Kokoro (voz pt-BR "pf_dora") e o Piper (pt_BR "faber"), e os modelos em data\voz\modelos.
#
#   powershell -ExecutionPolicy Bypass -File scripts\voz-local-instalar.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\voz-local-instalar.ps1 -ComRvc   # + timbre do Rimuru (RVC, ~2 GB)
#
# Depois: Configurações -> Voz -> Provedor "local".
param([switch]$ComRvc)

$ErrorActionPreference = 'Stop'
$raiz = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$venv = Join-Path $raiz '.venv-voz'
$modelos = Join-Path $raiz 'data\voz\modelos'
New-Item -ItemType Directory -Force -Path $modelos, (Join-Path $raiz 'data\voz\rimuru') | Out-Null

$py = Join-Path $venv 'Scripts\python.exe'
if (-not (Test-Path -LiteralPath $py)) {
    Write-Host 'Criando o ambiente Python (.venv-voz)...'
    $base = (Get-Command py -ErrorAction SilentlyContinue)
    if ($base) { & py -3.12 -m venv $venv } else { & python -m venv $venv }
}
& $py -m pip install --upgrade pip --quiet
Write-Host 'Instalando Kokoro e Piper...'
& $py -m pip install --quiet kokoro-onnx piper-tts numpy
if ($LASTEXITCODE -ne 0) { throw 'pip falhou ao instalar o Kokoro/Piper.' }

function Baixar([string]$url, [string]$nome) {
    $dest = Join-Path $modelos $nome
    if (Test-Path -LiteralPath $dest) { return }
    Write-Host "Baixando $nome..."
    Invoke-WebRequest -Uri $url -OutFile "$dest.part" -UseBasicParsing
    Move-Item -LiteralPath "$dest.part" -Destination $dest -Force
}
$k = 'https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0'
Baixar "$k/kokoro-v1.0.int8.onnx" 'kokoro-v1.0.int8.onnx'
Baixar "$k/voices-v1.0.bin" 'voices-v1.0.bin'
$p = 'https://huggingface.co/rhasspy/piper-voices/resolve/main/pt/pt_BR/faber/medium'
Baixar "$p/pt_BR-faber-medium.onnx" 'pt_BR-faber-medium.onnx'
Baixar "$p/pt_BR-faber-medium.onnx.json" 'pt_BR-faber-medium.onnx.json'

if ($ComRvc) {
    Write-Host 'Instalando o RVC (PyTorch para CPU)...'
    & $py -m pip install --quiet torch torchaudio --index-url https://download.pytorch.org/whl/cpu
    & $py -m pip install --quiet rvc-python
    if ($LASTEXITCODE -ne 0) { Write-Warning 'O rvc-python não instalou; a voz local funciona sem o timbre do Rimuru.' }
}
Write-Host 'Pronto. Escolha o provedor de voz "local" em Configurações -> Voz.'
