# Router Share/Gateway acceptance (legacy filename)

> The filename is retained for bookmark compatibility. This document no longer
> tests or requires the retired standalone Token Market. “Market” below means
> Router-built Share Market/Client Market only.

## Inputs

```bash
export SERVER_URL=http://127.0.0.1:15721
export CC_SWITCH_SERVER_TOKEN=<server-session-token>
export ROUTER_BASE_URL=https://router.example.com
export ROUTER_API_TOKEN=<router-share-token>
export CC_SWITCH_SHARE_URL=https://share.example.com
export SHARE_ID=<share-id>
```

Do not set or introduce a standalone Market URL, Market bearer, or external
Token Market credential. Missing real Router/Share/Gateway inputs are recorded
as `blocked-inputs`; fixtures are not live acceptance.

## Checks

1. `GET /v1/healthz` on Router returns healthy database status.
2. A Gateway fixture signs `GET /v1/gateway/shares` with its Ed25519 private
   key and the exact wire-body hash; a normal Router API token is not a
   Gateway credential. Before the future neutral grant contract exists, the
   ordinary Share inventory is empty: self-reported owner email, `freeAccess`
   and email ShareTo grants never authorize a Gateway.
3. Router Share URL dispatches Claude, Codex, and Gemini protocol probes through
   the signed ingress to the expected Server Share binding.
4. Duplicate request observations with the same request id are idempotent and
   do not double-count usage; cross-Gateway takeover, unauthorized Share IDs,
   legacy downstream identity/API-key/USD/settlement fields and `settled`
   status all fail closed.
5. Share Market grant/revoke pending edits apply once, acknowledge once, and
   preserve descriptor revision/fingerprint.
6. Client Market host provisioning/cleanup remains independent of Gateway
   observations and Share entitlements.
7. Retired `/v1/markets*`, `/v1/market/*`, and `/_market/proxy/*` routes return
   `410 Gone` and never fall through to a tunnel or UI handler.

## Commands

```bash
scripts/smoke/router-share-smoke.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
```

The standalone `cc-switch-market` repository is not part of this acceptance
path. Migration 21 physically retires the old Router live/archive tables after
checksum verification; only non-identifying aggregate receipts and eligible
canonical Share usage remain. No old Market writer or runtime dependency is
accepted.
