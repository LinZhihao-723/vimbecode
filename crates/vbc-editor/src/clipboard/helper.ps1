# The clipboard helper: one PowerShell process, started with the session and spoken to for the
# whole of it over the pipes it was started with.
#
# It answers the frames `protocol.rs` defines, and it answers every one of them: a clipboard it
# could not read is a "failed" frame carrying the reason, never a dropped connection, because the
# editor holding this process open through a locked workstation is the whole point of it being
# long-lived. It exits only when its input ends, which is what the editor's shutdown does to it.

$ErrorActionPreference = 'Stop'

$input_stream = [Console]::OpenStandardInput()
$output_stream = [Console]::OpenStandardOutput()
$utf8 = New-Object System.Text.UTF8Encoding($false)

$READ_TAG = 1
$WRITE_TAG = 2

$TEXT_STATUS = 1
$EMPTY_STATUS = 2
$NON_TEXT_STATUS = 3
$FAILED_STATUS = 4
$STORED_STATUS = 5

function Read-Exactly {
    param([int] $Count)

    $buffer = New-Object byte[] $Count
    $filled = 0
    while ($filled -lt $Count) {
        $read = $input_stream.Read($buffer, $filled, $Count - $filled)
        if ($read -le 0) {
            return $null
        }
        $filled += $read
    }

    return , $buffer
}

function Write-Frame {
    param([byte] $Status, [byte[]] $Body)

    $length = [BitConverter]::GetBytes([int]($Body.Length + 1))
    [Array]::Reverse($length)
    $output_stream.Write($length, 0, 4)
    $output_stream.WriteByte($Status)
    if ($Body.Length -gt 0) {
        $output_stream.Write($Body, 0, $Body.Length)
    }
    $output_stream.Flush()
}

function Write-Clipboard {
    param([byte[]] $Payload)

    $text = $utf8.GetString($Payload, 1, $Payload.Length - 1)
    if ($text.Length -eq 0) {
        Set-Clipboard -Value ''
    } else {
        Set-Clipboard -Value $text
    }
    Write-Frame $STORED_STATUS @()
}

function Read-Clipboard {
    $text = Get-Clipboard -Raw
    if ($null -ne $text) {
        Write-Frame $TEXT_STATUS ($utf8.GetBytes($text))
        return
    }

    if ((Get-Clipboard -Format Image) -or (Get-Clipboard -Format FileDropList)) {
        Write-Frame $NON_TEXT_STATUS @()
        return
    }

    Write-Frame $EMPTY_STATUS @()
}

while ($true) {
    $prefix = Read-Exactly 4
    if ($null -eq $prefix) {
        break
    }
    [Array]::Reverse($prefix)
    $length = [BitConverter]::ToUInt32($prefix, 0)
    if ($length -eq 0) {
        break
    }

    $payload = Read-Exactly $length
    if ($null -eq $payload) {
        break
    }

    try {
        if ($payload[0] -eq $READ_TAG) {
            Read-Clipboard
        } elseif ($payload[0] -eq $WRITE_TAG) {
            Write-Clipboard $payload
        } else {
            Write-Frame $FAILED_STATUS ($utf8.GetBytes("unknown request tag $($payload[0])"))
        }
    } catch {
        Write-Frame $FAILED_STATUS ($utf8.GetBytes($_.Exception.Message))
    }
}
