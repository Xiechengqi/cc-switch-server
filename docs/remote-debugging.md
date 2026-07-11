# Remote Debugging

Remote debugging is intended for diagnosing a Server through its Router Client Tunnel without SSH access to the host. It does not make administrative APIs anonymous.

## Access model

1. Sign in to the Web admin once and open Settings → Advanced → API Management.
2. Generate a debug token with a 1-24 hour lifetime.
3. Enable only the required capabilities: runtime diagnostics, redacted logs, restart, or upgrade.
4. Send the token as `Authorization: Bearer <debug-token>`.
5. Revoke the token and disable state-changing capabilities after the investigation.

The token is independent of Web sessions and normal API tokens. It is generated with 256 bits of entropy; only its one-way digest and expiration are stored in `server.json`, and plaintext is returned once at generation. Tokens in query strings are rejected by Router Client Tunnel handling.

## Endpoints

```text
GET  /web-api/debug/runtime
GET  /web-api/debug/diagnostics
GET  /web-api/debug/logs/tail?lines=100
POST /web-api/debug/restart
GET  /web-api/debug/operations/{operationId}
POST /web-api/debug/upgrade
GET  /web-api/debug/upgrade/status?taskId={taskId}
GET  /web-api/debug/upgrade/stream?taskId={taskId}
```

Router allows only these exact paths to bypass its Web login check and forwards the debug bearer token to Server for capability and expiration validation. Generic invoke/admin paths remain private.

## Restart evidence

Restart requests are executed by the detached self-update helper. The helper persists `restart-operation.json` before the old process exits, captures old/new PID and stage transitions, and completes only after the replacement `/version` endpoint reports a different PID and the expected commit. Upgrade state remains in `upgrade-state.json`.

If the replacement cannot start, the operation file and `server.log` retain the helper failure. Router also retains its normal tunnel `last_seen_at`/offline lifecycle, which distinguishes a replacement that never reconnected from an HTTP stream interruption.

Log API output is line-limited, masks the host log path, and redacts common credential assignments. It should still be treated as operationally sensitive.
