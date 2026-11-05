# FTP / SFTP plugin: changelog

The plugin's own versions, apart from the program's. Releases are tagged `ftp-v<version>` in the RustySalamander repository.

## 0.1.1-alpha (2026-11-05)

- **Data connection right below the port:** in the connect form, the FTP data connection mode (Automatic, Passive, Active) follows the server and port it belongs with.

## 0.1.0-alpha (2026-11-05)

- **FTP, FTPS and SFTP servers in a panel:** Plugins → Connect to FTP or SFTP server… connects with a password (or, for SFTP, a private key) and opens the server's files in the active panel as `sftp://`, `ftps://` or `ftp://`. Copying in and out, dragging out of the window (streamed, without a temporary copy), deleting, renaming and new folders work as on SCP servers, with progress and cancelling; a cancelled upload leaves no part on the server.
- **SFTP:** over SSH in pure Rust; server keys are checked against known_hosts, the plugin's own, OpenSSH's and the SCP plugin's.
- **FTPS:** explicit TLS; a certificate no public authority vouches for is shown with its SHA-256 fingerprint first and remembered once trusted; a changed one is refused.
- **FTP:** passive mode, MLSD or LIST (Unix and DOS forms), UTF-8 names, file times kept with MFMT; up to three connections per server, so browsing doesn't wait for a copy, and connections the server closed while idle are opened again.
- **FTP data connections, found automatically:** passive mode (EPSV, then PASV) is tried first and active mode (PORT/EPRT) when passive doesn't get through; the way that works is kept for the session. The connect form can also ask for passive or active mode only.
- **Only valid choices in the form:** "Sign in with" is for SFTP only (FTP signs in with the password), "Data connection" for FTP and FTPS only; what doesn't apply is greyed out.
- Built for plugin interface 8; needs RustySalamander 0.9.2-beta or later (its form uses choices that follow other choices).
