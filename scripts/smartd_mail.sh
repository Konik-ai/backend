#!/bin/bash
set -euo pipefail

ADDRESS_BLOCK="${SMARTD_ADDRESS:-}"
FROM_ADDRESS="${ADDRESS_BLOCK%% *}"
RECIPIENTS="${ADDRESS_BLOCK// /, }"

if [[ -z "${FROM_ADDRESS}" ]]; then
    FROM_ADDRESS="ryleymcc@shaw.ca"
fi

BODY_FILE="$(mktemp)"
trap 'rm -f "$BODY_FILE"' EXIT
cat > "$BODY_FILE"

if ! grep -qiE '^to:' "$BODY_FILE"; then
    {
        [[ -n "${RECIPIENTS}" ]] && echo "To: ${RECIPIENTS}"
        [[ -n "${SMARTD_SUBJECT:-}" ]] && echo "Subject: ${SMARTD_SUBJECT}"
        echo
        cat "$BODY_FILE"
    } | /usr/sbin/sendmail -t -f "$FROM_ADDRESS"
else
    /usr/sbin/sendmail -t -f "$FROM_ADDRESS" < "$BODY_FILE"
fi
