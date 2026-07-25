# Remote SSH from Mobile to Windows PC

Termius + Tailscale + psmux = persistent terminal sessions from your phone.

## 1. Install OpenSSH Server (Admin)

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service -Name sshd -StartupType 'Automatic'
New-NetFirewallRule -Name 'OpenSSH-Server' -DisplayName 'OpenSSH Server' -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22
```

## 2. Generate SSH key

```powershell
ssh-keygen -t ed25519 -f "$env:USERPROFILE\.ssh\id_ed25519" -N '""'
Get-Content "$env:USERPROFILE\.ssh\id_ed25519.pub" | Add-Content "$env:ProgramData\ssh\administrators_authorized_keys"
```

> Windows admins use `administrators_authorized_keys`, not `~/.ssh/authorized_keys`.

## 3. Hardening (Admin)

Edit `C:\ProgramData\ssh\sshd_config`:

```
PasswordAuthentication no
AllowUsers <your-username>
PermitRootLogin no
```

Restrict firewall to Tailscale:

```powershell
Set-NetFirewallRule -Name 'OpenSSH-Server' -RemoteAddress '100.64.0.0/10'
Restart-Service sshd
```

## 4. Install Tailscale (both PC and phone)

```powershell
winget install tailscale
tailscale up
```

On your phone — install the app, log into the same account, enable connection.

Get your Tailscale IP:

```powershell
tailscale ip -4
```

## 5. Install psmux

```powershell
winget install psmux
```

## 6. Configure Termius

- **Host**: your Tailscale IP (`100.x.x.x`)
- **Port**: `22`
- **Username**: your Windows username
- **Key**: paste the content of `C:\Users\<you>\.ssh\id_ed25519` (private key, including headers)

## 7. Usage

Connect from Termius, then:

```
psmux new -s dev
```

- `Ctrl+B` `D` — detach (session stays alive)
- `psmux attach -t dev` — reattach after reconnecting
- `Ctrl+B` `C` — new window
- `Ctrl+B` `%` / `"` — split panes
