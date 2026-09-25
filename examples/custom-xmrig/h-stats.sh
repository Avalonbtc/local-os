#!/bin/bash
local response
response=$(curl --fail --silent --max-time 5 "http://127.0.0.1:$CUSTOM_API_PORT/2/summary") || return 1
khs=$(jq -er '.hashrate.total[0] / 1000' <<< "$response") || return 1
stats=$(jq -c '{algo:.algo,ver:.version,uptime:.uptime,ar:[.results.shares_good,(.results.shares_total-.results.shares_good)],connected:(.connection.pool != null and .connection.uptime > 0)}' <<< "$response")
