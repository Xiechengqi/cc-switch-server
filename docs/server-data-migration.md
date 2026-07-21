# Server Data Directory Migration

cc-switch-server does not provide generic configuration import/export. Move a
server to a new host by copying its complete data directory while both the old
and new processes are stopped.

The default directory is `$HOME/.cc-switch-server`. A deployment using
`--config-dir` or `CC_SWITCH_SERVER_CONFIG_DIR` must migrate that resolved path
instead.

## Preconditions

1. Record `cc-switch-server version` on the source host. Run the same or a newer
   compatible build on the target host.
2. Record the service user, data-directory path, startup flags, and environment.
3. If `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` is set, transfer that secret
   through the deployment secret manager. It takes precedence over
   `accounts.key` and must remain byte-for-byte identical. The same root key
   decrypts Account tokens and, through a purpose-separated HKDF key, S2
   Provider credential slots.
4. Confirm the target can reach the configured Router and all upstream Provider
   endpoints directly. Server does not use system or application-level outbound
   proxies.

## Cold Migration

1. Stop cc-switch-server on the source host and verify no process is writing the
   data directory.
2. Archive or synchronize the complete directory, including hidden files,
   permissions, `accounts.json`, `accounts.key`, `server.json`, `providers.json`,
   Share/tunnel state, usage journals, logs, and backup snapshots.
3. Verify the transfer with a checksum or an equivalent integrity check.
4. Keep cc-switch-server stopped on the target, place the directory at the
   resolved target path, and restore ownership to the target service user.
   Directories should remain private and secret-bearing files must not become
   group/world readable.
5. Apply the source `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY` on the target when
   the source used that environment variable.
6. Start only the target instance. Never run source and target concurrently:
   they share the Router installation identity, Client subdomain, and Share
   tunnel identities.

Example archive flow for the default root-owned installation:

```bash
rc-service cc-switch-server stop
tar -C /root -cpf /tmp/cc-switch-server-data.tar .cc-switch-server
sha256sum /tmp/cc-switch-server-data.tar

# On the target, with cc-switch-server still stopped:
tar -C /root -xpf /tmp/cc-switch-server-data.tar
chown -R root:root /root/.cc-switch-server
chmod 700 /root/.cc-switch-server
rc-service cc-switch-server start
```

Adapt the service manager, user, group, and path to the deployment. Do not place
the archive in source control or an unencrypted public location.

## Acceptance

After startup, verify all of the following before deleting the source copy:

- Startup logs show no account decryption, store decode, or Router signature
  error.
- Existing password/email login works and the owner identity is unchanged.
- OAuth accounts load without `needs relogin`; an explicit subscription refresh
  and a Provider model test succeed.
- The existing Client URL reconnects in Router without creating a second Client.
- Every Share URL reconnects with the same binding, owner, ShareTo ACL, limits,
  and market state.
- Recent usage/request history and model pricing are present.

For rollback, stop the target before restarting the untouched source instance.
Do not allow both instances to reconnect to Router at the same time.

The Web UI backup/restore feature remains an operational snapshot mechanism for
one installation. It is not a replacement for complete data-directory migration.

## Provider Store Format

Do not combine host migration with an implicit Provider format conversion.
Existing S1 installations remain S1 after an ordinary startup; fresh data
directories write S2 on the first Provider commit. To convert an existing
installation, run the read-only preflight first:

```bash
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-provider-store
```

Stop the Server before every write action. `--apply` creates and validates an
S1 snapshot before replacing `providers.json`; `--rollback` restores that
snapshot; `--cleanup-snapshot` is the only supported deletion path:

```bash
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-provider-store --apply
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-provider-store --rollback
cc-switch-server --config-dir "$CONFIG_DIR" config migrate-provider-store --cleanup-snapshot
```

The data-directory lock rejects these actions while another Server process is
running. Keep `provider-migrations/s1-to-s2/` through the compatibility window.
S1 and older-Web readers cannot be removed until two stable bridge releases and
at least 14 observation days have been recorded.

S2 protects against disclosure of an isolated `providers.json` or Provider-only
backup that does not contain the root key. It does not protect against disclosure
of the complete data directory, the environment root key, or compromise of the
Server OS user. A staged restore of S2 data must have the matching `accounts.key`
or environment key before any live file is replaced.
