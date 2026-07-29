# Cursor AgentService acceptance

Cursor Provider profiles remain `experimental` until OAuth and API-key credentials pass separate real-account runs. Static tests establish only `wired` and `statically-tested`; they never set `live-verified`.

## Fixed identity contract

- A Cursor credential type has at most one stored proxy credential.
- Every OAuth Provider binds one explicit Cursor OAuth account. Every API-key Provider uses one static key or one explicitly bound API-key account.
- A request never switches Provider, account, or key because of load, cooldown, 401, 429, quota, or network failure.
- OAuth may force-refresh the same account once after an initial 401. API key may invalidate and exchange the same key once. No committed stream is replayed.
- Production OAuth, public API, exchange, and AgentService requests use audited fixed HTTPS origins.

## Required real matrix

Run every row independently for OAuth and API key: non-stream text; stream text and terminal event; reasoning; data-URI image; remote image; declared tool call; tool result continuation; client cancellation; first-frame timeout; inter-frame timeout; 401 recovery; second 401; 403; 429 with `Retry-After`; 5xx before output; disconnect after output; malformed and oversized Connect frame; invalid gzip; nonzero gRPC trailer; concurrent limit saturation; parked session after server restart; model discovery; log and error redaction.

Expected invariants: 401 recovery retains the original principal; 429 and saturation never select another identity; malformed frames, gzip failures, and nonzero trailers never produce a successful terminal response; restart returns `409 cursor_session_lost`; Cursor usage records set `usageEstimated=true`; no access token, refresh token, exchange token, or API key appears in logs or evidence.

## SDK differential oracle

Use the `composer-api` official `@cursor/sdk` bridge as a test oracle, not a production dependency:

```bash
CURSOR_SERVER_URL=http://127.0.0.1:15721 \
CURSOR_SDK_ORACLE_URL=http://127.0.0.1:8787 \
CURSOR_SERVER_TOKEN=... \
CURSOR_SDK_ORACLE_TOKEN=... \
CURSOR_TEST_MODEL=composer-2.5 \
node scripts/smoke/cursor-sdk-differential.mjs
```

The script records only semantic summaries: HTTP status class, content presence, terminal event presence, finish reasons, and declared tool names. It does not print credentials or full response bodies.
