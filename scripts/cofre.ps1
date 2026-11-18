# Cofre do S.A.G.E.: criptografa as chaves de API (config\config.json) e a memória
# (data\memory\wiki) em config\secrets.enc, que pode ir para o GitHub. Só abre com a senha.
#
# Formato: PBKDF2-SHA256 (600 mil iterações) -> AES-256-CBC + HMAC-SHA256 (encrypt-then-MAC).
# Funciona no Windows PowerShell 5.1 e no PowerShell 7. Usado por chaves-exportar.ps1,
# chaves-importar.ps1 e instalar.ps1 (dot-source: . "$PSScriptRoot\cofre.ps1").

$ErrorActionPreference = 'Stop'
$script:CofreIteracoes = 600000

function Get-SageRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
}

function Read-CofreSenha([string]$SenhaArquivo, [switch]$Confirmar) {
    if ($SenhaArquivo) {
        $linha = Get-Content -LiteralPath $SenhaArquivo -TotalCount 1 -Encoding UTF8
        if (-not $linha) { throw 'O arquivo de senha está vazio.' }
        return [string]$linha
    }
    $a = Read-Host 'Senha do cofre do S.A.G.E.' -AsSecureString
    $senha = [Runtime.InteropServices.Marshal]::PtrToStringUni([Runtime.InteropServices.Marshal]::SecureStringToBSTR($a))
    if ($Confirmar) {
        $b = Read-Host 'Repita a senha' -AsSecureString
        $senha2 = [Runtime.InteropServices.Marshal]::PtrToStringUni([Runtime.InteropServices.Marshal]::SecureStringToBSTR($b))
        if ($senha -ne $senha2) { throw 'As senhas não batem.' }
    }
    if ($senha.Length -lt 8) { throw 'Use uma senha com pelo menos 8 caracteres.' }
    return $senha
}

function Get-CofreChaves([string]$Senha, [byte[]]$Sal, [int]$Iteracoes) {
    $kdf = New-Object System.Security.Cryptography.Rfc2898DeriveBytes($Senha, $Sal, $Iteracoes, [System.Security.Cryptography.HashAlgorithmName]::SHA256)
    try { $bytes = $kdf.GetBytes(64) } finally { $kdf.Dispose() }
    return @{ Enc = [byte[]]$bytes[0..31]; Mac = [byte[]]$bytes[32..63] }
}

function Get-CofreMac([byte[]]$Chave, [byte[]]$Iv, [byte[]]$Dados) {
    $h = New-Object System.Security.Cryptography.HMACSHA256 -ArgumentList (, $Chave)
    try {
        $tudo = New-Object byte[] ($Iv.Length + $Dados.Length)
        [Array]::Copy($Iv, 0, $tudo, 0, $Iv.Length)
        [Array]::Copy($Dados, 0, $tudo, $Iv.Length, $Dados.Length)
        return $h.ComputeHash($tudo)
    } finally { $h.Dispose() }
}

function Test-BytesIguais([byte[]]$A, [byte[]]$B) {
    if ($A.Length -ne $B.Length) { return $false }
    $d = 0
    for ($i = 0; $i -lt $A.Length; $i++) { $d = $d -bor ($A[$i] -bxor $B[$i]) }
    return ($d -eq 0)
}

# Arquivos que entram no cofre (relativos à raiz).
function Get-CofreArquivos([string]$Raiz) {
    $lista = New-Object System.Collections.Generic.List[string]
    if (Test-Path -LiteralPath (Join-Path $Raiz 'config\config.json')) { $lista.Add('config/config.json') }
    $wiki = Join-Path $Raiz 'data\memory\wiki'
    if (Test-Path -LiteralPath $wiki) {
        Get-ChildItem -LiteralPath $wiki -Recurse -File -Filter *.md | ForEach-Object {
            $rel = $_.FullName.Substring($Raiz.Length + 1) -replace '\\', '/'
            $lista.Add($rel)
        }
    }
    return $lista
}

function Protect-SageCofre([string]$Raiz, [string]$Senha) {
    $arquivos = [ordered]@{}
    foreach ($rel in (Get-CofreArquivos $Raiz)) {
        $arquivos[$rel] = [Convert]::ToBase64String([IO.File]::ReadAllBytes((Join-Path $Raiz $rel)))
    }
    if ($arquivos.Count -eq 0) { throw 'Não achei o config\config.json para guardar.' }
    $plano = [Text.Encoding]::UTF8.GetBytes(($arquivos | ConvertTo-Json -Compress))

    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    $sal = New-Object byte[] 16; $rng.GetBytes($sal)
    $iv = New-Object byte[] 16; $rng.GetBytes($iv)
    $rng.Dispose()
    $k = Get-CofreChaves $Senha $sal $script:CofreIteracoes

    $aes = [System.Security.Cryptography.Aes]::Create()
    $aes.KeySize = 256; $aes.Mode = 'CBC'; $aes.Padding = 'PKCS7'; $aes.Key = $k.Enc; $aes.IV = $iv
    try { $cifra = $aes.CreateEncryptor().TransformFinalBlock($plano, 0, $plano.Length) } finally { $aes.Dispose() }
    $mac = Get-CofreMac $k.Mac $iv $cifra

    $saida = [ordered]@{
        v = 1
        kdf = 'pbkdf2-sha256'
        iter = $script:CofreIteracoes
        salt = [Convert]::ToBase64String($sal)
        iv = [Convert]::ToBase64String($iv)
        mac = [Convert]::ToBase64String($mac)
        arquivos = @($arquivos.Keys)
        data = [Convert]::ToBase64String($cifra)
    }
    $destino = Join-Path $Raiz 'config\secrets.enc'
    [IO.File]::WriteAllText($destino, ($saida | ConvertTo-Json), (New-Object Text.UTF8Encoding($false)))
    return @{ Caminho = $destino; Arquivos = @($arquivos.Keys) }
}

function Unprotect-SageCofre([string]$Raiz, [string]$Senha, [switch]$SemSobrescrever) {
    $origem = Join-Path $Raiz 'config\secrets.enc'
    if (-not (Test-Path -LiteralPath $origem)) { throw 'Não existe config\secrets.enc.' }
    $c = Get-Content -LiteralPath $origem -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($c.v -ne 1) { throw "Versão de cofre desconhecida: $($c.v)" }
    $sal = [Convert]::FromBase64String($c.salt)
    $iv = [Convert]::FromBase64String($c.iv)
    $cifra = [Convert]::FromBase64String($c.data)
    $k = Get-CofreChaves $Senha $sal ([int]$c.iter)
    $mac = Get-CofreMac $k.Mac $iv $cifra
    if (-not (Test-BytesIguais $mac ([Convert]::FromBase64String($c.mac)))) {
        throw 'Senha errada (ou o cofre foi alterado).'
    }
    $aes = [System.Security.Cryptography.Aes]::Create()
    $aes.KeySize = 256; $aes.Mode = 'CBC'; $aes.Padding = 'PKCS7'; $aes.Key = $k.Enc; $aes.IV = $iv
    try { $plano = $aes.CreateDecryptor().TransformFinalBlock($cifra, 0, $cifra.Length) } finally { $aes.Dispose() }
    $arquivos = [Text.Encoding]::UTF8.GetString($plano) | ConvertFrom-Json

    $gravados = New-Object System.Collections.Generic.List[string]
    foreach ($p in $arquivos.PSObject.Properties) {
        $rel = $p.Name
        if ($rel -match '\.\.' -or [IO.Path]::IsPathRooted($rel)) { continue }
        $alvo = Join-Path $Raiz ($rel -replace '/', '\')
        if ((Test-Path -LiteralPath $alvo) -and $SemSobrescrever) { continue }
        $dir = Split-Path -Parent $alvo
        if (-not (Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
        if ((Test-Path -LiteralPath $alvo) -and $rel -eq 'config/config.json') { Copy-Item -LiteralPath $alvo -Destination "$alvo.bak" -Force }
        [IO.File]::WriteAllBytes($alvo, [Convert]::FromBase64String([string]$p.Value))
        $gravados.Add($rel)
    }
    return $gravados
}
