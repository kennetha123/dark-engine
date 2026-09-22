#!/usr/bin/env bash
# Enforces docs/PLAN.md §2 rule 1: simulation crates never depend on presentation.
# Add new sim crates to SIM_CRATES and new presentation crates to FORBIDDEN.
set -euo pipefail

SIM_CRATES=(dark_core dark_time dark_net dark_world dark_sprite dark_assets dark_physics dark_sim dark_combat dark_life dark-host)
FORBIDDEN='^(winit|wgpu|pollster|dark_platform|dark_render|dark_audio|dark_spine|libfmod|rusty_spine|egui|cosmic-text|swash) '

status=0
for crate in "${SIM_CRATES[@]}"; do
  # Run cargo tree on its own so a typo in SIM_CRATES fails loudly instead of passing.
  tree=$(cargo tree -p "$crate" -e normal --prefix none)
  if hits=$(grep -E "$FORBIDDEN" <<<"$tree" | sort -u) && [ -n "$hits" ]; then
    echo "error: sim crate '$crate' depends on presentation crates:"
    echo "$hits" | sed 's/^/  /'
    status=1
  fi
done

if [ "$status" -eq 0 ]; then
  echo "sim dependency check passed (${SIM_CRATES[*]})"
fi
exit "$status"
