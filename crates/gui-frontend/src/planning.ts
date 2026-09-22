import type { CollapseAction, PlanEntry, ScanBundle, ScanSettings } from './types'

// Mirrors engine::collapse::apply_collapse_to_plan's filtering (crates/engine/src/collapse.rs)
// on the frontend, so the Reconcile and Transfer screens can show an accurate
// "what will actually be copied" total before a transfer runs. Only Missing/
// Partial entries are ever excluded - an Archive decision still copies (just
// redirected), and a Moved entry is untouched by collapse either way, exactly
// as the Rust side behaves.
export function entriesToTransfer(bundle: ScanBundle, settings: ScanSettings): PlanEntry[] {
  const actionByPath = new Map<string, CollapseAction>()
  if (bundle.collapse) {
    for (const decision of bundle.collapse.decisions) {
      actionByPath.set(decision.path, decision.action)
    }
  }

  return bundle.plan.entries.filter((entry) => {
    const state = entry.state.state
    const touchable =
      state === 'missing' || state === 'partial' || (state === 'moved' && settings.onMovedCopy)
    if (!touchable) return false

    if (
      settings.mode === 'collapse' &&
      (state === 'missing' || state === 'partial') &&
      entry.source_path
    ) {
      if (actionByPath.get(entry.source_path) === 'collapse') return false
    }
    return true
  })
}

export function totalBytes(entries: PlanEntry[]): number {
  return entries.reduce((sum, e) => sum + (e.size ?? 0), 0)
}

export function formatBytes(bytes: number): string {
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`
}

export function formatDuration(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '—'
  const s = Math.round(seconds)
  const m = Math.floor(s / 60)
  const rem = s % 60
  if (m === 0) return `${rem}s`
  const h = Math.floor(m / 60)
  const remM = m % 60
  if (h === 0) return `${remM}m ${rem}s`
  return `${h}h ${remM}m`
}
