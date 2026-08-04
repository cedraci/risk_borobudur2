$ErrorActionPreference = "Stop"
Push-Location frontend
npm ci
npm run build
Pop-Location
cargo build --release -p server
Write-Host "Done. Run: .\target\release\server.exe"
