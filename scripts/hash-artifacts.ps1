[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateScript({ Test-Path -LiteralPath $_ -PathType Leaf })]
    [string[]]$Path
)

$Path | ForEach-Object {
    $file = Get-Item -LiteralPath $_
    $hash = Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
    [pscustomobject]@{ Name = $file.Name; Bytes = $file.Length; SHA256 = $hash.Hash }
}
