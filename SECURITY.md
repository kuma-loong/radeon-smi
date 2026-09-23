# Security policy

Please report security issues privately through GitHub's repository security
advisory feature. Do not open a public issue for a suspected vulnerability.

`radeon-smi` is read-only and runs with the user's device permissions. It does
not require setuid, a root service, or elevated installation scripts. GPU
device access is controlled by the operating system's DRM node permissions.
