#!/bin/bash
set -ex

export wallet_db_path="/data/wallet_db"
export log_level="info"

if [[ -z "$1" ]];then
  [[ -n "$store_id" ]] && [[ -n "$peer_id" ]] && exec ./firm-wallet-service
  # If store_id or peer_id is not set use the suffix of ${HOSTNAME}.
  # Will exit if the suffix of ${HOSTNAME} is not a number.
  [[ "${HOSTNAME}" =~ -([0-9]+)$ ]] || exit 1
  ordinal=${BASH_REMATCH[1]}
  (( store_id = ordinal + 1 ))
  export store_id=${store_id}
  export peer_id=${store_id}
  exec ./firm-wallet-service
else
  exec "$@"
fi
