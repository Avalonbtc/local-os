// Machines ticked in the fleet list. Shared so the top bar can turn into the HiveOS bulk bar
// ("‹ 取消选定 N 矿机" plus action icons) while the list keeps its checkboxes.
import { useSyncExternalStore } from "react";

let selected: string[] = [];
const listeners = new Set<() => void>();

export function setSelection(next: string[]) {
  selected = next;
  listeners.forEach((listener) => listener());
}

export function useSelection() {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => selected,
  );
}
