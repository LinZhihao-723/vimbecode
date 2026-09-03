# The ground truth the write path is checked against: what Windows itself says is on the clipboard.
#
# It is `Get-Clipboard` and nothing else, but what it reads is handed back in a file rather than on
# standard output. The console this is started from has a code page -- 936 on the machine this was
# written on -- and anything printed through it is converted into that code page on the way out,
# which would destroy exactly the characters the round trip is being checked for. A file written as
# UTF-8 goes nowhere near the console.
#
# The exit status says which of the three answers this is, because an empty file is what both an
# empty clipboard and an empty string leave behind.
#
#   0  the clipboard holds text, which is in the file
#   2  the clipboard could not be read, and the reason is in the file
#   3  the clipboard holds no text at all

param([string] $Out)

$ErrorActionPreference = 'Stop'
$utf8 = New-Object System.Text.UTF8Encoding($false)

try {
    $text = Get-Clipboard -Raw
} catch {
    [System.IO.File]::WriteAllBytes($Out, $utf8.GetBytes($_.Exception.Message))
    exit 2
}

if ($null -eq $text) {
    exit 3
}

[System.IO.File]::WriteAllBytes($Out, $utf8.GetBytes($text))
exit 0
