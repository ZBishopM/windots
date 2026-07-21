' Run the autostart layout script once at login, hidden (no console flash).
CreateObject("WScript.Shell").Run "pwsh -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File ""C:\Users\obisp\.config\rice-autostart.ps1""", 0, False
