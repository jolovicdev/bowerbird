# Security

## Reporting a vulnerability

Report it privately through
[GitHub's private vulnerability reporting](https://github.com/jolovicdev/bowerbird/security/advisories/new),
not in a public issue. Say which version and platform you ran, and include
the steps or files that show the problem. Fixes ship in the next release.

## What Bowerbird protects

- **Notebooks from someone else.** A notebook is a folder that can be copied
  between people. Bowerbird opens one only when every source it names is a
  file inside the folder and nothing in the folder is a symbolic link, so a
  notebook can't make it read, write or delete files elsewhere.
- **The server on a network.** Every API request needs the token the server
  prints at startup. Links added through the server reach only the public
  internet unless it runs with `--allow-private-links`, so a token holder
  can't make it read the machine it runs on or that machine's network.
  Addresses a web page chooses, such as its pictures, are held to the public
  internet too, apart from the page's own host. The server speaks plain HTTP;
  put a TLS proxy in front of it before using it across a network you don't
  trust.
- **Downloads.** Each model file Bowerbird downloads, and each PDFium archive
  the builds use, is checked against its recorded SHA-256 before use.
- **Quotes.** A citation shows as verified only when its quoted words appear
  in the source it names.

Anyone who holds the token can read and change every notebook in the
server's library, and anyone who can write to your library folder or
settings file controls what Bowerbird does. Reports that need either are
out of scope.
