import { useEffect, useState } from 'react'
import { VirtualList } from '../components/VirtualList'
import { formatBytes, formatDuration } from '../planning'
import type { ScanBundle, ScanSettings, TransferProgressEvent, TransferSummary } from '../types'

interface Props {
  bundle: ScanBundle
  settings: ScanSettings
  totalBytesToCopy: number
  totalFilesToCopy: number
  transferring: boolean
  events: TransferProgressEvent[]
  summary: TransferSummary | null
  startedAt: number | null
  error: string | null
  onStart: (dryRun: boolean) => void
}

interface QueueItem {
  relativePath: string
  outcome: 'copied' | 'failed'
  bytes: number | null
  detail: string | null
}

export function TransferScreen({
  totalBytesToCopy,
  totalFilesToCopy,
  transferring,
  events,
  summary,
  startedAt,
  error,
  onStart,
}: Props) {
  const [now, setNow] = useState(() => Date.now())

  useEffect(() => {
    if (!transferring) return
    const id = setInterval(() => setNow(Date.now()), 500)
    return () => clearInterval(id)
  }, [transferring])

  const bytesDone = events.reduce((sum, e) => sum + (e.bytes ?? 0), 0)
  const filesDone = events.length
  const failedCount = events.filter((e) => e.outcome === 'failed').length
  const elapsedSeconds = startedAt ? (now - startedAt) / 1000 : 0
  const bytesPerSecond = elapsedSeconds > 0 ? bytesDone / elapsedSeconds : 0
  const remainingBytes = Math.max(0, totalBytesToCopy - bytesDone)
  const eta = bytesPerSecond > 0 ? remainingBytes / bytesPerSecond : NaN
  const progressPct = totalBytesToCopy > 0 ? Math.min(100, (bytesDone / totalBytesToCopy) * 100) : 0

  const queue: QueueItem[] = summary
    ? summary.results.map((r) => ({
        relativePath: r.relative_path,
        outcome: r.outcome,
        bytes: r.bytes,
        detail: r.detail,
      }))
    : events.map((e) => ({
        relativePath: e.relativePath,
        outcome: e.outcome,
        bytes: e.bytes,
        detail: null,
      }))

  return (
    <div className="screen">
      <h2>Transfer</h2>

      <div className="stat-row">
        <div className="stat">
          <span className="value">
            {filesDone} / {totalFilesToCopy}
          </span>
          <span className="label">Files</span>
        </div>
        <div className="stat">
          <span className="value">
            {formatBytes(bytesDone)} / {formatBytes(totalBytesToCopy)}
          </span>
          <span className="label">Bytes</span>
        </div>
        <div className="stat">
          <span className="value">{transferring ? formatDuration(eta) : '—'}</span>
          <span className="label">ETA</span>
        </div>
        <div className="stat">
          <span className="value">{failedCount}</span>
          <span className="label">Failed</span>
        </div>
      </div>

      <div className="summary-bar">
        <div className="segment" style={{ width: `${progressPct}%`, background: 'var(--accent)' }} />
      </div>

      {error && <div className="error-banner">{error}</div>}

      {!transferring && !summary && (
        <div className="field-row">
          <button className="secondary-button" onClick={() => onStart(true)}>
            Dry run
          </button>
          <button className="primary-button" onClick={() => onStart(false)}>
            Start transfer
          </button>
        </div>
      )}

      {summary && (
        <p style={{ color: 'var(--text-dim)' }}>
          Done: {summary.copied} copied, {summary.failed} failed, {summary.skipped_verified} already
          verified, {formatBytes(summary.bytes_copied)} written.
        </p>
      )}

      <p className="section-title">Review queue</p>
      <VirtualList
        className="list-container"
        items={queue}
        itemHeight={30}
        emptyMessage="No files transferred yet."
        renderItem={(item: QueueItem) => (
          <div className="list-row">
            <span className={`badge ${item.outcome}`}>{item.outcome}</span>
            <span className="mono truncate" style={{ flex: 1 }}>
              {item.relativePath}
            </span>
            {item.bytes != null && <span style={{ color: 'var(--text-dim)' }}>{formatBytes(item.bytes)}</span>}
            {item.detail && (
              <span className="truncate" style={{ color: 'var(--danger)', maxWidth: 260 }} title={item.detail}>
                {item.detail}
              </span>
            )}
          </div>
        )}
      />
    </div>
  )
}
