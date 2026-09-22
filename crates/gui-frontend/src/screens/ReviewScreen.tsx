import { useMemo, useState } from 'react'
import { VirtualList } from '../components/VirtualList'
import { formatBytes } from '../planning'
import type {
  AnalyzeReport,
  CollapseReport,
  DuplicateGroup,
  Relationship,
  SimilarPair,
  SimilarityReport,
} from '../types'

interface Props {
  analyze: AnalyzeReport
  similarity: SimilarityReport
  collapse: CollapseReport | null
}

const RELATIONSHIP_LABEL: Record<Relationship, string> = {
  near_duplicate: 'Near duplicate',
  diverged: 'Diverged (manual merge)',
  b_is_more_complete: 'B is more complete',
  a_is_more_complete: 'A is more complete',
}

function fileName(path: string): string {
  const parts = path.split(/[\\/]/)
  return parts[parts.length - 1] || path
}

export function ReviewScreen({ analyze, similarity, collapse }: Props) {
  const [tab, setTab] = useState<'exact' | 'near'>('exact')

  const actionByPath = useMemo(() => {
    const map = new Map<string, string>()
    if (collapse) for (const d of collapse.decisions) map.set(d.path, d.action)
    return map
  }, [collapse])

  return (
    <div className="screen">
      <h2>Review</h2>
      <div className="stat-row">
        <div className="stat">
          <span className="value">{analyze.duplicate_groups.length}</span>
          <span className="label">Exact-duplicate groups</span>
        </div>
        <div className="stat">
          <span className="value">{formatBytes(analyze.reclaimable_bytes)}</span>
          <span className="label">Reclaimable (exact only)</span>
        </div>
        <div className="stat">
          <span className="value">{similarity.pairs.length}</span>
          <span className="label">Near-duplicate / version pairs</span>
        </div>
        <div className="stat">
          <span className="value">{similarity.files_skipped_unsupported}</span>
          <span className="label">Files skipped (unsupported format)</span>
        </div>
      </div>

      <div className="mode-toggle" style={{ maxWidth: 360 }}>
        <button className={tab === 'exact' ? 'active' : ''} onClick={() => setTab('exact')}>
          Exact duplicates
        </button>
        <button className={tab === 'near' ? 'active' : ''} onClick={() => setTab('near')}>
          Near duplicates
        </button>
      </div>

      {tab === 'exact' ? (
        <VirtualList
          className="list-container"
          items={analyze.duplicate_groups}
          itemHeight={54}
          emptyMessage="No exact duplicates found."
          renderItem={(group: DuplicateGroup) => (
            <div className="list-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 2, padding: '6px 10px' }}>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <span className="mono" style={{ color: 'var(--text-dim)' }}>
                  {group.hash.slice(0, 12)}
                </span>
                <span>{formatBytes(group.size)}</span>
                <span style={{ color: 'var(--text-dim)' }}>{group.files.length} copies</span>
              </div>
              <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                {group.files.map((f) => {
                  const action = actionByPath.get(f)
                  return (
                    <span key={f} className="truncate" title={f} style={{ maxWidth: 260 }}>
                      {action && <span className={`badge ${action}`}>{action}</span>}{' '}
                      {fileName(f)}
                    </span>
                  )
                })}
              </div>
            </div>
          )}
        />
      ) : (
        <VirtualList
          className="list-container"
          items={similarity.pairs}
          itemHeight={54}
          emptyMessage="No near-duplicate or version relationships found."
          renderItem={(pair: SimilarPair) => (
            <div className="list-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 2, padding: '6px 10px' }}>
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <span className={`badge ${pair.relationship === 'diverged' ? 'conflict' : 'neutral'}`}>
                  {RELATIONSHIP_LABEL[pair.relationship]}
                </span>
                <span style={{ color: 'var(--text-dim)' }}>
                  estimated similarity {(pair.jaccard_estimate * 100).toFixed(0)}%
                </span>
                <span style={{ color: 'var(--text-dim)' }}>
                  coverage A→B {(pair.coverage_a_to_b * 100).toFixed(0)}% · B→A{' '}
                  {(pair.coverage_b_to_a * 100).toFixed(0)}%
                </span>
              </div>
              <div className="truncate" title={pair.a}>
                A: {fileName(pair.a)}
              </div>
              <div className="truncate" title={pair.b}>
                B: {fileName(pair.b)}
              </div>
            </div>
          )}
        />
      )}
      <p style={{ color: 'var(--text-dim)', fontSize: 12 }}>
        "Estimated similarity" is the MinHash/LSH Jaccard estimate, not a calibrated confidence
        score - SPEC.md section 4's confidence calibration is still pending ~200 hand-labeled
        real-data pairs (P5 open item).
      </p>
    </div>
  )
}
