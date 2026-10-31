# Private repositories over SSH

Depot supports private Git repositories through project-scoped, read-only SSH
deploy keys. HTTPS credentials and passphrase prompts are not supported.

## Configure a project

1. Generate a dedicated key with `ssh-keygen -t ed25519 -f depot-project -N ''`.
2. Add `depot-project.pub` to the repository as a read-only deploy key. Do not
   enable write access.
3. Obtain the Git host's published SSH host key from a trusted channel. Format
   it as an OpenSSH `known_hosts` line. Do not trust an unverified
   `ssh-keyscan` result by itself.
4. Create the Depot project with a URL such as
   `ssh://git@github.com/owner/repository.git`.
5. In the project dashboard, paste the private key and trusted host-key lines,
   then save. Depot encrypts the private key and does not return it.
6. Run repository inspection and remove the local private-key copy after the
   first successful clone. Keep a secure recovery copy only if your key
   rotation process requires one.

The worker decrypts a key only for the clone operation, writes it into the
worker workspace with mode `0600`, and deletes the temporary credential
directory afterward. Git invokes SSH in batch mode with only that identity and
`StrictHostKeyChecking=yes`. A missing or changed host key fails the job; Depot
never downgrades host verification.

Rotate a deploy key by adding the new public key at the Git provider, replacing
the private key in Depot, confirming inspection succeeds, and then removing the
old public key. Updating credentials replaces the prior encrypted value.
