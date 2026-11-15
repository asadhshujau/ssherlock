# Test fixtures

Every key here was generated with `ssh-keygen` for SSHerlock's tests and has never been used
against a real server. They're public on purpose. Don't reuse them anywhere.

| File | Type | Passphrase | Authorized on the test server |
|------|------|------------|-------------------------------|
| `id_ed25519` | Ed25519 | none | yes |
| `id_ecdsa` | ECDSA P-256 | none | yes |
| `id_rsa` | RSA 3072 | none | yes |
| `id_ed25519_encrypted` | Ed25519 | `fixture-passphrase` | yes |
| `id_unauthorized` | Ed25519 | none | no, for "key refused" tests |

The test server (`../docker/`) has one user, `tester`, with password `tester-password`.

To regenerate a key, run the same `ssh-keygen` command with a fresh file and update this table:

```sh
ssh-keygen -t ed25519 -N '' -C ssherlock-test-ed25519 -f id_ed25519
```
