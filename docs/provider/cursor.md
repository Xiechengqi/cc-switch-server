# Cursor AgentService acceptance

Cursor Provider profiles remain `experimental` until OAuth and API-key credentials pass separate real-account runs. Static tests establish only `wired` and `statically-tested`; they never set `live-verified`.

## Typed credential contract

- Cursor OAuth may store multiple accounts. Every OAuth Provider binds one explicit Cursor OAuth account and its exact identity generation. Every API-key Provider uses one Provider-owned static key.
- A request never switches Provider, account, or key because of load, cooldown, 401, 429, quota, or network failure.
- OAuth may force-refresh the same account once after an initial 401. API key may invalidate and exchange the same key once. No committed stream is replayed.
- OAuth always uses the CLI wire rail: CLI headers, CLI RunRequest, empty RequestContext, and CLI completion policy. API key always uses the SDK wire rail: SDK headers, SDK RunRequest, rich RequestContext, and SDK completion policy. Session scope includes the rail and its protocol revision.
- Production inference defaults to authenticated `POST https://api2.cursor.sh/aiserver.v1.ServerConfigService/GetServerConfig`. The protobuf response must contain both `agentUrl` and `agentnUrl`; both origins must be HTTPS, default-port, pathless `api5.cursor.sh` hosts, and inference uses `agentUrl + /agent.v1.AgentService/Run`. Discovery is cached for one hour in an exact scope containing App, Provider revision, credential generation, runtime fingerprint, rail, principal, and access-token digest.
- API-key exchange defaults to `https://api2.cursor.sh/auth/exchange_user_api_key`. OAuth refresh uses the same endpoint with the bound account's refresh token as Bearer and an empty JSON body. `CC_SWITCH_CURSOR_SERVER_CONFIG_ENDPOINT`, `CC_SWITCH_CURSOR_OAUTH_AGENT_ENDPOINT`, `CC_SWITCH_CURSOR_APIKEY_AGENT_ENDPOINT`, and `CC_SWITCH_CURSOR_APIKEY_EXCHANGE_ENDPOINT` are optional complete-HTTPS overrides. Userinfo, fragments, missing paths, and non-HTTPS production URLs fail closed before credential/network work.
- A discovery 401 and an AgentService 401 share one recovery budget. The first eligible 401 may refresh or re-exchange only the same bound credential and replay once; a second 401 is terminal.
- `CC_SWITCH_CURSOR_OAUTH_AGENT_ENABLED` and `CC_SWITCH_CURSOR_APIKEY_AGENT_ENABLED` independently disable a rail. A disabled or failed rail never falls back to the other.
- Provider model-test dry-run validates the selected rail, endpoint policy, exact credential binding, request conversion, and model policy without making an outbound request. Network mode uses the normal native AgentService forwarder pinned to that Provider and requires the downstream protocol's terminal event. Test responses expose only a redacted endpoint-policy label.
- `cc-switch-server doctor` validates only rails that have enabled Cursor Provider surfaces. An invalid override or disabled runtime rail is a failing check, and diagnostics never print endpoint or credential values.

## Completion contract

- Business completion is rail-specific. API Key SDK requires `TurnEnded` or a surfaced tool call. OAuth CLI accepts those and the observed KV-after-visible-text terminal. KV without visible text never completes either rail.
- Every surfaced tool call retains one client-visible call id across pause and resume. Gemini stream and JSON responses emit that id in `functionCall.id`; a later `functionResponse.id` takes precedence, with `name` accepted only as a compatibility fallback for older clients.
- A business completion signal ends the RPC and closes the local stream immediately, matching the Cursor SDK transport; it does not wait for a later HTTP EOF or gRPC trailer.
- If the response reaches transport termination before a business completion signal, a plain HTTP/2 EOF is not sufficient: the transport must first supply `grpc-status: 0` or one valid successful Connect end-stream JSON envelope, after which the request still fails as an incomplete business response.
- Truncated protobuf, invalid UTF-8, incomplete/oversized/compressed frames, malformed or failed end-stream envelopes, data after a terminal in the same decoded frame batch, and plain EOF fail with 502 when observed before business completion.
- `TurnEnded` without text, reasoning, or a surfaced tool call is an empty failure. Non-stream requests return 502; streams emit an in-band protocol error, never a normal success terminal, and persist failed usage.

## Model selection contract

- Router Share resolution selects the Cursor Provider before model parsing. A `cursor:*` name can only change the wire model or mode inside that already-selected Provider; it never selects another Provider, account, key, or Share.
- Aliases are `cursor`, `cursor-agent`, `cursor-plan`, `cursor-ask`, `cursor-composer`, and `cursor-composer-fast`. Mode prefixes are `cursor:`, `cursor-agent:`, `cursor-plan:`, and `cursor-ask:`.
- Agent, Ask, and Plan encode protobuf mode values 1, 2, and 3. A trailing `-fast` on any wire model is removed from `model_id` and encoded as `fast=true`.
- An explicit Cursor alias or prefix takes precedence over a Cursor Provider's single-model default. An ordinary unprefixed request still follows the committed RuntimePlan; passthrough mode may carry a bare Cursor wire model.
- OAuth and API-key catalogs expose the aliases. API-key discovery retains a bare upstream model ID only when it does not collide, and always exposes `cursor:<id>` plus Agent/Plan/Ask namespaced variants. Duplicate aliases and collisions are removed deterministically.

## Required real matrix

Run every row independently for OAuth and API key: Anthropic Messages, OpenAI Chat, OpenAI Responses, and Gemini ingress; non-stream text; stream text and terminal event; Agent/Ask/Plan modes; arbitrary `-fast` model; reasoning; data-URI image; remote image; declared tool call; tool result continuation; client cancellation; first-frame timeout; inter-frame timeout; 401 recovery; second 401; 403; 429 with `Retry-After`; 5xx before output; disconnect after output; malformed and oversized Connect frame; invalid gzip; missing/malformed/failed Connect terminal before business completion; nonzero gRPC trailer before business completion; concurrent limit saturation; parked session after server restart; alias and namespaced model discovery; collision handling; log and error redaction.

Expected invariants: 401 recovery retains the original principal; 429 and saturation never select another identity; malformed frames, gzip failures, and nonzero trailers observed before a business completion never produce a successful response; restart returns `409 cursor_session_lost`; Cursor usage records set `usageEstimated=true`; no access token, refresh token, exchange token, API key, or private runtime endpoint appears in logs or evidence.

## SDK differential oracle

Use the `composer-api` official `@cursor/sdk` bridge as a test oracle, not a production dependency:

```bash
CC_SWITCH_SHARE_URL=https://share.example.com \
ROUTER_API_TOKEN=... \
ROUTER_API_TOKEN_HEADER=Authorization \
CURSOR_SDK_ORACLE_URL=http://127.0.0.1:8787 \
CURSOR_SDK_ORACLE_TOKEN=... \
CURSOR_TEST_MODEL=composer-2.5 \
node scripts/smoke/cursor-sdk-differential.mjs
```

`CC_SWITCH_SHARE_URL` 必须是 Router 暴露的 Share URL；该验收不会调用 Server 的 `15721` 推理路径。

The script exercises Anthropic, Chat, Responses, and Gemini Server ingress. Composer-api supplies the Chat/Responses SDK oracle for equivalent canonical prompts. The script records only semantic summaries: HTTP status class, content presence, terminal event presence, finish reasons, and declared tool names. It does not print credentials or full response bodies.
