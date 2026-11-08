#!/usr/bin/env bash
# Offline oracle audit. Source code is Apache-2.0; see upstream/LICENSE.
# Calls original functions with isolated files; no real requests or credentials.
set -eu
cd "$(dirname "$0")/.."
test "$(git -C upstream rev-parse HEAD)" = dcaffa81af551e3f259ddb1433a170aff8b840b6
task_tmp=$(mktemp -d /tmp/portal-desktop-audit.XXXXXX)
trap 'rm -rf "$task_tmp"' EXIT
sed '$d' upstream/src/usr/bin/portal-login > "$task_tmp/core"
source "$task_tmp/core"
STATUS_FILE="$task_tmp/status"
LOG_FILE="$task_tmp/log"
log_i() { :; }; log_w() { :; }; log_e() { :; }; log_d() { :; }
passed=0
check() {
    local label="$1" expected="$2" actual="$3"
    if [ "$expected" != "$actual" ]; then
        printf 'FAIL %s expected=%s actual=%s\n' "$label" "$expected" "$actual"
        exit 1
    fi
    printf 'PASS %s\n' "$label"
    passed=$((passed+1))
}
# Original encryption uses cat /dev/urandom. Substitute only that read.
cat() {
    if [ "$#" = 1 ] && [ "$1" = /dev/urandom ]; then printf xK9z;
    else command cat "$@"; fi
}
node - "$task_tmp/vectors" <<'NODE'
const fs = require('node:fs');
const crypto = require('node:crypto');
const vectors = ['testpass', '123456789012', '1234567890123',
  'ABCDEFGHIJKLMNOPQRSTUVWXYZ012345', '1234567890123456789012345678901!'];
const lines = vectors.map(plain => {
  if (/^[A-Za-z0-9]{32}$/.test(plain)) return `${plain}\t${plain}`;
  const raw = Buffer.from('xK9z' + plain, 'utf8');
  const padded = Buffer.alloc(Math.ceil(raw.length / 16) * 16);
  raw.copy(padded);
  const cipher = crypto.createCipheriv('aes-128-ecb', Buffer.from('5a3b9f207411a8ed'), null);
  cipher.setAutoPadding(false);
  return `${plain}\t${Buffer.concat([cipher.update(padded), cipher.final()]).toString('hex')}`;
});
fs.writeFileSync(process.argv[2], lines.join('\n') + '\n');
NODE
while IFS=$'\t' read -r plain expected; do
    check "AES/length-${#plain}" "$expected" "$(encrypt_pass "$plain" 5a3b9f207411a8ed)"
done < "$task_tmp/vectors"

P_HOST=http://192.168.199.200
_direct_curl() {
    local endpoint=""
    for endpoint in "$@"; do :; done
    printf '%s\n' "$endpoint" >> "$task_tmp/requests"
    case "$endpoint" in
        */assets/js/raas.js) printf '%s' "$js_first" ;;
        */js/raas.js) printf '%s' "$js_second" ;;
        *) return 22 ;;
    esac
}
uci() { printf '%s\n' "$*" >> "$task_tmp/writes"; }
reset_case() {
    CFG_KEY=5a3b9f207411a8ed
    js_first=""; js_second=""
    : > "$task_tmp/requests"; : > "$task_tmp/writes"
}
reset_case
js_first='var unrelated="abcdefghijklmnop";'
result=0; detect_key || result=$?
check key/unrelated-return 1 "$result"
check key/unrelated-keeps-config 5a3b9f207411a8ed "$CFG_KEY"
check key/unrelated-no-write 0 "$(wc -l < "$task_tmp/writes" | tr -d ' ')"
check key/fallback-three-paths 3 "$(wc -l < "$task_tmp/requests" | tr -d ' ')"

reset_case
js_second='CryptoJS.enc.Utf8.parse("0123456789abcdef")'
detect_key
check key/fallback-value 0123456789abcdef "$CFG_KEY"
check key/fallback-two-paths 2 "$(wc -l < "$task_tmp/requests" | tr -d ' ')"
check key/persist-two-operations 2 "$(wc -l < "$task_tmp/writes" | tr -d ' ')"

reset_case
js_first='Utf8.parse("5a3b9f207411a8ed")'
js_second='Utf8.parse("0123456789abcdef")'
detect_key
check key/same-value-stops 1 "$(wc -l < "$task_tmp/requests" | tr -d ' ')"
check key/same-value-no-write 0 "$(wc -l < "$task_tmp/writes" | tr -d ' ')"

reset_case
js_first='<html>Utf8.parse("0123456789abcdef"</html>'
detect_key
check key/regex-accepts-html-and-missing-parenthesis 0123456789abcdef "$CFG_KEY"

reset_case
js_first="Utf8.parse(\"0123456789abcdef'"
detect_key
check key/regex-does-not-match-quote-types 0123456789abcdef "$CFG_KEY"

reset_case
js_first='Utf8.parse("0123456789abcdef")'
uci() { return 1; }
result=0; detect_key || result=$?
check key/persist-failure-does-not-block-login 0 "$result"
check key/persist-failure-keeps-runtime-candidate 0123456789abcdef "$CFG_KEY"

# Drive original parsing and login decision functions from reusable JSON fixtures.
node - "$task_tmp/portal-vectors" "$task_tmp/login-vectors" <<'NODE'
const fs = require('node:fs');
const data = JSON.parse(fs.readFileSync('fixtures/protocol.json', 'utf8'));
fs.writeFileSync(process.argv[2], data.portal_urls.map(x =>
  [x.id, x.url, x.result, x.bras ?? '-', x.clientip ?? '-'].join('\t')).join('\n') + '\n');
fs.writeFileSync(process.argv[3], data.login_results.map(x =>
  [x.id, JSON.stringify({ret:x.login_ret}), JSON.stringify({ret:x.ack_ret}),
   x.probe_result, x.result, x.state].join('\t')).join('\n') + '\n');
NODE
_get_wan_ip() { printf '10.23.45.67\n'; }
_save_params_uci() { :; }
while IFS=$'\t' read -r id url expected bras clientip; do
    result=0; _parse_portal_url "$url" || result=$?
    check "URL/$id/result" "$expected" "$result"
    if [ "$result" = 0 ]; then
        check "URL/$id/bras" "$bras" "$P_BRAS"
        check "URL/$id/clientip" "$clientip" "$P_CLIENTIP"
    fi
done < "$task_tmp/portal-vectors"

encrypt_pass() { printf 'synthetic-ciphertext\n'; }
_curl_api() {
    local endpoint=""
    for endpoint in "$@"; do :; done
    case "$endpoint" in
        */api/login.php\?*) printf '%s\n' "$login_response" ;;
        */api/ack_auth.php\?*) printf '%s\n' "$ack_response" ;;
        *) return 0 ;;
    esac
}
jsonfilter() { node upstream/tests/jsonfilter.cjs "$@"; }
probe_portal() { return "$probe_result"; }
_up_seconds() { printf '100\n'; }
CFG_USER=synthetic-user; CFG_PASS=synthetic-password
while IFS=$'\t' read -r id login_response ack_response probe_result expected state; do
    P_HOST=http://192.168.199.200; P_BRAS=hn; P_CLIENTIP=10.23.45.67
    P_WLANUSERIP=10.23.45.67; P_WLANACNAME=hn
    P_CLIENTMAC=aa:bb:cc:dd:ee:ff; P_PAIP=172.16.100.200; P_VLAN=1407.1051
    result=0; do_login || result=$?
    check "login/$id/result" "$expected" "$result"
    check "login/$id/state" "$state" "$_state"
done < "$task_tmp/login-vectors"
printf 'RESULT %s passed, 0 failed\n' "$passed"
