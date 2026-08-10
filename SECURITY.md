# Security

Onionloom handles long-lived Tor identity keys. Treat generated secret files and checkpoints as credentials.

## Report a vulnerability

Use [GitHub private vulnerability reporting](https://github.com/chrisch88dev/onionloom/security/advisories/new) or email [chris@chrisch88.dev](mailto:chris@chrisch88.dev), PGP fingerprint `946C 571F 7423 FF1E 565E E5DF C9C0 6176 47EC 61F9`. Do not open a public issue or include real keys. Include the affected commit, operating system, minimal reproduction, observed behavior, and impact.

## Key protections

- randomness comes from the operating system
- expanded secrets and temporary seeds are zeroized
- found public keys are recomputed from their secret before output
- file headers, lengths, hostname checksum, and key relationships are validated
- Unix directories use mode `0700` and key files use mode `0600`
- existing key directories are never overwritten
- secret keys are never intentionally printed
- GPU matches are reconstructed and verified with the CPU backend before output

Windows files inherit the chosen parent directory access rules. Use a directory restricted to the current account.

The GPU receives public points and public prefix masks, not the expanded secret. GPU drivers and shader compilers are part of the trusted execution environment for availability, but a false match cannot bypass CPU verification. Automatic mode runs an exact arithmetic self-test before use and falls back to CPU when adapter discovery, feature checks, shader validation, or measurement fails.

## Limits

A checkpoint contains secret material. Keep it on encrypted local storage and never sync or commit it. Deleting a checkpoint may not erase old blocks on solid-state or copy-on-write storage.

Onionloom cannot protect keys on a compromised host or from a malicious kernel, debugger, swap inspection, memory capture, or unsafe parent-directory permissions.
