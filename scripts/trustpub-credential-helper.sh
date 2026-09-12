#!/usr/bin/env bash
# trustpub-credential-helper.sh — cargo credential provider for crates.io
# trusted publishing (TrustPub OIDC) inside GitHub Actions.
#
# Cargo invokes this as:
#   <helper> --cargo-plugin
# and speaks the cargo credential protocol (JSON lines on stdio):
#   helper -> cargo : {"v":[1]}                         (protocol hello)
#   cargo  -> helper : {"v":1,"registry":{"index-url":...,"name":...},
#                       "kind":"get","operation":"read"|"publish"|...}
#   helper -> cargo : {"Ok":{"kind":"get","token":"...",
#                           "operation_independent":false,"cache":"never"}}
#
# operation_independent:false + cache:"never" force cargo to request a
# fresh token for every publish operation — combined with the backend's
# hardcoded 30-minute token expiry, a token can never straddle a publish.
#
# Registry wire format verified against rust-lang/crates-io-auth-action
# v1.0.5 (pinned SHA c6f97d42) and the crates.io backend controller
# (src/controllers/trustpub/tokens/exchange/mod.rs):
#   mint:     GET $ACTIONS_ID_TOKEN_REQUEST_URL&audience=crates.io
#             Authorization: Bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN
#             -> {"value": "<jwt>"}
#   exchange: POST https://crates.io/api/v1/trusted_publishing/tokens
#             Content-Type: application/json
#             body: {"jwt": "<jwt>"}
#             -> {"token": "<registry token>"}   (30-minute expiry)
#
# FAIL-CLOSED: any failure exits nonzero without a valid stdout
# response. Token values are never written to stderr and never logged.

set -euo pipefail

readonly AUDIENCE='crates.io'
readonly EXCHANGE_URL='https://crates.io/api/v1/trusted_publishing/tokens'
readonly USER_AGENT='rust-camel-trustpub-credential-helper/1.0.0 (+https://github.com/kennycallado/rust-camel)'

log() { printf '[trustpub-credential-helper] %s\n' "$*" >&2; }
die() { log "ERROR: $*"; exit 1; }

[ "${1:-}" = '--cargo-plugin' ] || die "must be invoked by cargo with --cargo-plugin (got: '${1:-<none>}')"

: "${ACTIONS_ID_TOKEN_REQUEST_URL:?ACTIONS_ID_TOKEN_REQUEST_URL unset — job needs id-token: write}"
: "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:?ACTIONS_ID_TOKEN_REQUEST_TOKEN unset — job needs id-token: write}"

command -v curl >/dev/null 2>&1 || die 'curl not found'
command -v jq >/dev/null 2>&1 || die 'jq not found'

# 1. Protocol hello — cargo waits for this line before sending the request.
printf '{"v":[1]}\n'

# 2. Read exactly one credential request line.
IFS= read -r request || die 'no credential request received on stdin'
kind="$(printf '%s' "$request" | jq -r '.kind // empty')"
[ "$kind" = 'get' ] || die "unsupported credential request kind '${kind:-<none>}' (helper is get-only)"
index_url="$(printf '%s' "$request" | jq -r '.registry["index-url"] // empty')"
case "$index_url" in
  *crates.io-index*|*index.crates.io*) ;; # crates.io (git or sparse index)
  *) die "refusing non-crates.io registry '${index_url:-<none>}'" ;;
esac

mint_url="${ACTIONS_ID_TOKEN_REQUEST_URL}"
case "$mint_url" in
  *\?*) mint_url="${mint_url}&audience=${AUDIENCE}" ;;
  *) mint_url="${mint_url}?audience=${AUDIENCE}" ;;
esac

# 3. Mint the job-scoped OIDC JWT for audience crates.io.
#    The bearer credential is passed via a curl --config stream, not argv
#    (argv is world-readable through /proc on shared runners).
jwt_json="$(curl --silent --show-error --fail --max-time 30 --config - \
  --request GET "$mint_url" <<EOF
header = "Authorization: Bearer ${ACTIONS_ID_TOKEN_REQUEST_TOKEN}"
EOF
)" || die 'OIDC JWT request failed'
jwt="$(printf '%s' "$jwt_json" | jq -r '.value // empty')"
[ -n "$jwt" ] || die 'OIDC JWT response missing .value'

# 4. Exchange the JWT for a short-lived crates.io registry token.
#    Secrets travel via stdin only (jq -Rs / --data-binary @-). On HTTP
#    failure the server-authored error detail (if any) goes to stderr —
#    error bodies never contain the token, and we never print a success
#    body outside this variable.
exchange_body="$(printf '%s' "$jwt" | jq -Rs '{jwt: .}')"
exchange_raw="$(printf '%s' "$exchange_body" | curl --silent --show-error --max-time 30 \
  --header "Content-Type: application/json" \
  --header "User-Agent: ${USER_AGENT}" \
  --request POST --data-binary @- --write-out '\n%{http_code}' "$EXCHANGE_URL")" || die 'crates.io trustpub exchange failed (transport)'
http_code="${exchange_raw##*$'\n'}"
exchange_json="${exchange_raw%$'\n'*}"
if [ "$http_code" -lt 200 ] 2>/dev/null || [ "$http_code" -ge 300 ] 2>/dev/null || [ -z "$http_code" ]; then
  detail="$(printf '%s' "$exchange_json" | jq -r '.errors[0].detail // empty' 2>/dev/null || true)"
  die "crates.io trustpub exchange failed (HTTP ${http_code:-none})${detail:+: $detail}"
fi
token="$(printf '%s' "$exchange_json" | jq -r '.token // empty')"
[ -n "$token" ] || die 'exchange response missing .token'

# 5. Emit the credential response for cargo — stdout ONLY, via stdin
#    (not argv). operation_independent:false + cache:"never" => cargo
#    must request a fresh token for every publish operation.
#    jq -c is LOAD-BEARING: without it jq pretty-prints multi-line and
#    cargo's single read_line gets a bare `{` — "failed to deserialize
#    response". Verified against cargo 1.98.1 credential/process.rs.
printf '%s' "$token" | jq -cRs '{Ok: {kind: "get", token: ., operation_independent: false, cache: "never"}}'
