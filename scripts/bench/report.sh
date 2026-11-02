#!/usr/bin/env bash
# Markdown comparison table from a bench results.ndjson. The best value of
# each row is in bold.
# Usage: scripts/bench/report.sh ~/.cache/speck-bench/<run>/results.ndjson
set -eu
jq -rs '
  (map(.runtime) | unique) as $rts
  | group_by(.metric) as $groups
  | (["metric"] + $rts + ["unit", "note"] | "| " + join(" | ") + " |"),
    ("|" + ([range(0; ($rts | length) + 3)] | map("---") | join("|")) + "|"),
    ($groups[]
      | . as $rows
      | ($rows[0].better) as $better
      | ([$rows[] | select(.value | type == "number") | .value]) as $nums
      | (if $better == "lower" then ($nums | min) elif $better == "higher" then ($nums | max) else null end) as $best
      | ([$rts[] as $rt | ($rows | map(select(.runtime == $rt)) | first) as $r
          | if $r == null then "-"
            elif ($best != null and ($nums | length) > 1 and $r.value == $best) then "**\($r.value)**"
            else "\($r.value)" end]) as $cells
      | "| " + ([$rows[0].metric] + $cells + [$rows[0].unit, ($rows[0].note | gsub("\\|"; "/"))] | join(" | ")) + " |")
' "$1"
