#!/bin/bash
jq -n --arg wallet "$CUSTOM_TEMPLATE" --arg url "${CUSTOM_URL%%$'\n'*}" --arg password "${CUSTOM_PASS:-x}" --arg algo "$CUSTOM_ALGO" --argjson port "$CUSTOM_API_PORT" '{autosave:false,cpu:{enabled:true},http:{enabled:true,host:"127.0.0.1",port:$port,"access-token":null,restricted:true},pools:[{url:$url,user:$wallet,pass:$password,algo:$algo}]}' > "$CUSTOM_CONFIG_FILENAME"
