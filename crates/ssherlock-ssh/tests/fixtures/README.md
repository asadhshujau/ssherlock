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
| `id_rsa_pkcs8` | `id_rsa` as PKCS#8 PEM | none | yes (same key as `id_rsa`) |
| `id_rsa_pem_encrypted` | `id_rsa` as PKCS#1 PEM, AES-128-CBC | `fixture-passphrase` | yes (same key as `id_rsa`) |

The test server (`../docker/`) has one user, `tester`, with password `tester-password`.

To regenerate a key, run the same `ssh-keygen` command with a fresh file and update this table:

```sh
ssh-keygen -t ed25519 -N '' -C ssherlock-test-ed25519 -f id_ed25519
```

The two converted copies of `id_rsa` come from `ssh-keygen -p` on a copy of it (the copy must be
`0600` or `ssh-keygen` refuses it):

```sh
cp id_rsa id_rsa_pkcs8 && chmod 600 id_rsa_pkcs8 && ssh-keygen -p -m PKCS8 -N '' -P '' -f id_rsa_pkcs8
cp id_rsa id_rsa_pem_encrypted && chmod 600 id_rsa_pem_encrypted \
  && ssh-keygen -p -m PEM -N fixture-passphrase -P '' -f id_rsa_pem_encrypted
```
