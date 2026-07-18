' Launch the ShadowPlay recorder (PowerShell, auto-mic) fully hidden.
CreateObject("WScript.Shell").Run "pwsh -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""C:\Users\obisp\.config\shadowplay-record.ps1""", 0, False
