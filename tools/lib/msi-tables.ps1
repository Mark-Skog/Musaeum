# Reads, without installing anything, what a Windows Installer package says about itself: its
# properties and its files. Prints JSON. Used by tools/smoke-bundle.mjs.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File msi-tables.ps1 -Path PACKAGE.msi
param([Parameter(Mandatory = $true)][string]$Path)

$ErrorActionPreference = 'Stop'
$installer = New-Object -ComObject WindowsInstaller.Installer

function Invoke-Member($target, [string]$name, [string]$kind, $arguments) {
    $flags = [System.Reflection.BindingFlags]::$kind
    return $target.GetType().InvokeMember($name, $flags, $null, $target, $arguments)
}

function Read-Rows([string]$query, [int]$columns) {
    $view = Invoke-Member $database 'OpenView' 'InvokeMethod' @($query)
    [void](Invoke-Member $view 'Execute' 'InvokeMethod' $null)
    $rows = @()
    while ($true) {
        $record = Invoke-Member $view 'Fetch' 'InvokeMethod' $null
        if ($null -eq $record) { break }
        $row = @()
        for ($column = 1; $column -le $columns; $column++) {
            $row += [string](Invoke-Member $record 'StringData' 'GetProperty' @($column))
        }
        $rows += , $row
    }
    [void](Invoke-Member $view 'Close' 'InvokeMethod' $null)
    return , $rows
}

# 0: read only.
$database = Invoke-Member $installer 'OpenDatabase' 'InvokeMethod' @($Path, 0)
$properties = @{}
foreach ($row in (Read-Rows 'SELECT `Property`, `Value` FROM `Property`' 2)) {
    $properties[$row[0]] = $row[1]
}
$files = @()
foreach ($row in (Read-Rows 'SELECT `FileName`, `FileSize`, `Version` FROM `File`' 3)) {
    $name = $row[0]
    if ($name.Contains('|')) { $name = $name.Split('|')[1] }
    $files += [pscustomobject]@{ name = $name; size = [int64]$row[1]; version = $row[2] }
}
[System.Runtime.InteropServices.Marshal]::ReleaseComObject($database) | Out-Null
[System.Runtime.InteropServices.Marshal]::ReleaseComObject($installer) | Out-Null
[pscustomobject]@{ properties = $properties; files = $files } | ConvertTo-Json -Depth 4 -Compress
