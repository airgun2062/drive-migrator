import { VirtualList } from '../components/VirtualList'
import { formatBytes } from '../planning'
import type { PlanEntry, ReconcilePlan, ReconcileSummary } from '../types'

interface Props {
  plan: ReconcilePlan
  scanning: boolean
  onRescan: () => void
  onGoToTransfer: () => void
}

const SEGMENTS: Array<{ key: keyof ReconcileSummary; label: string; color: string }> = [
  { key: 'verified', label: 'Verified', color: 'var(--ok)' },
  { key: 'missing', label: 'Missing', color: 'var(--danger)' },
  { key: 'partial', label: 'Partial', color: 'var(--warn)' },
  { key: 'moved', label: 'Moved', color: '#8e6fd1' },
  { key: 'conflict', label: 'Conflict', color: 'var(--warn)' },
  { key: 'blocked', label: 'Blocked', color: 'var(--danger)' },
  { key: 'destination_only', label: 'Destination only', color: 'var(--text-dim)' },
]

export function ReconcileScreen({ plan, scanning, onRescan, onGoToTransfer }: Props) {
  const total = Object.values(plan.summary).reduce((a, b) => a + b, 0) || 1

  return (
    <div className="screen">
      <h2>Resume and reconcile</h2>
      <p style={{ color: 'var(--text-dim)', margin: 0 }}>
        {plan.source_root} → {plan.destination_root}
      </p>

      <div className="summary-bar">
        {SEGMENTS.map(
          (s) =>
            plan.summary[s.key] > 0 && (
              <div
                key={s.key}
                className="segment"
                style={{ width: `${(plan.summary[s.key] / total) * 100}%`, background: s.color }}
                title={`${s.label}: ${plan.summary[s.key]}`}
              />
            ),
        )}
      </div>
      <div className="summary-legend">
        {SEGMENTS.map((s) => (
          <span key={s.key}>
            <span className="dot" style={{ background: s.color }} />
            {s.label}: {plan.summary[s.key]}
          </span>
        ))}
      </div>

      <div className="field-row">
        <button className="secondary-button" disabled={scanning} onClick={onRescan}>
          {scanning ? 'Rescanning…' : 'Rescan'}
        </button>
        <button className="primary-button" onClick={onGoToTransfer}>
          Continue to transfer
        </button>
      </div>

      <VirtualList
        className="list-container"
        items={plan.entries}
        itemHeight={30}
        emptyMessage="No files found."
        renderItem={(entry: PlanEntry) => (
          <div className="list-row">
            <span className={`badge ${entry.state.state}`}>{entry.state.state.replace('_', ' ')}</span>
            <span className="mono truncate" style={{ flex: 1 }} title={entry.relative_path}>
              {entry.relative_path}
            </span>
            <span style={{ color: 'var(--text-dim)' }}>
              {entry.size != null ? formatBytes(entry.size) : '—'}
            </span>
          </div>
        )}
      />
    </div>
  )
}
