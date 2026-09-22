import { useEffect, useMemo, useState } from 'react'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { VirtualList } from '../components/VirtualList'
import { hashFile } from '../api'
import type { AnalyzeReport, CollapseReport, ScanSettings, VersionFamily } from '../types'

interface Props {
  settings: ScanSettings
  onChange: (settings: ScanSettings) => void
  onRescan: () => void
  scanning: boolean
  collapse: CollapseReport | null
  analyze: AnalyzeReport
}

function fileName(path: string): string {
  const parts = path.split(/[\\/]/)
  return parts[parts.length - 1] || path
}

function familyMatches(family: VersionFamily, needle: string): boolean {
  if (!needle) return true
  const lower = needle.toLowerCase()
  return family.members.some((m) => m.path.toLowerCase().includes(lower))
}

const EMPTY_FAMILIES: VersionFamily[] = []

export function VersionHistoryScreen({
  settings,
  onChange,
  onRescan,
  scanning,
  collapse,
  analyze,
}: Props) {
  const [search, setSearch] = useState('')
  const [dropActive, setDropActive] = useState(false)
  const [lookupResult, setLookupResult] = useState<string | null>(null)

  function set<K extends keyof ScanSettings>(key: K, value: ScanSettings[K]) {
    onChange({ ...settings, [key]: value })
  }

  useEffect(() => {
    let unlisten: (() => void) | undefined
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === 'enter' || event.payload.type === 'over') {
          setDropActive(true)
        } else if (event.payload.type === 'leave') {
          setDropActive(false)
        } else if (event.payload.type === 'drop') {
          setDropActive(false)
          void lookup(event.payload.paths[0])
        }
      })
      .then((fn) => {
        unlisten = fn
      })
    return () => unlisten?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [collapse, analyze])

  async function lookup(path: string) {
    setLookupResult(`Hashing ${fileName(path)}…`)
    try {
      const hash = await hashFile(path)
      const dupGroup = analyze.duplicate_groups.find((g) => g.hash === hash)
      if (dupGroup) {
        setLookupResult(
          `Exact match: byte-identical to ${dupGroup.files.length - 1} other file(s) already in the scanned source.`,
        )
        return
      }
      const family = collapse?.version_report.families.find((f) =>
        f.members.some((m) => m.path === path),
      )
      if (family) {
        const member = family.members.find((m) => m.path === path)!
        setLookupResult(
          `Part of a version family: rank ${member.rank} of ${family.members.length}${member.is_tip ? ' (tip)' : ''}.`,
        )
        return
      }
      setLookupResult('No match found in the current scan (not a duplicate or version-family member).')
    } catch (err) {
      setLookupResult(`Lookup failed: ${err}`)
    }
  }

  const families = collapse?.version_report.families ?? EMPTY_FAMILIES
  const filteredFamilies = useMemo(
    () => families.filter((f) => familyMatches(f, search)),
    [families, search],
  )

  return (
    <div className="screen">
      <h2>Version history</h2>

      <div className="form-card">
        <p className="section-title">Mode</p>
        <div className="mode-toggle">
          <button
            className={settings.mode === 'copy' ? 'active' : ''}
            onClick={() => set('mode', 'copy')}
          >
            Copy
          </button>
          <button
            className={settings.mode === 'collapse' ? 'active' : ''}
            onClick={() => set('mode', 'collapse')}
          >
            Collapse
          </button>
        </div>
        <div className="field-row">
          <label>Keep newest</label>
          <input
            type="range"
            min={1}
            max={10}
            value={settings.keepNewest}
            onChange={(e) => set('keepNewest', Number(e.target.value))}
            disabled={settings.mode !== 'collapse'}
          />
          <span>{settings.keepNewest}</span>
        </div>
        <div className="checkbox-row">
          <input
            type="checkbox"
            id="archive-vh"
            checked={settings.archive}
            disabled={settings.mode !== 'collapse'}
            onChange={(e) => set('archive', e.target.checked)}
          />
          <label htmlFor="archive-vh">Archive collapsed versions instead of dropping them</label>
        </div>
        <button className="primary-button" disabled={scanning} onClick={onRescan}>
          {scanning ? 'Applying…' : 'Apply and rescan'}
        </button>
      </div>

      {settings.mode !== 'collapse' ? (
        <p style={{ color: 'var(--text-dim)' }}>
          Switch to Collapse mode and rescan to see version families.
        </p>
      ) : (
        <>
          <div className="field-row">
            <label>Search</label>
            <input
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder="Filter families by path"
            />
          </div>

          <div className={`dropzone${dropActive ? ' active' : ''}`}>
            Drop a file here to find which version family it belongs to.
          </div>
          {lookupResult && <p style={{ color: 'var(--text-dim)' }}>{lookupResult}</p>}

          <VirtualList
            className="list-container"
            items={filteredFamilies}
            itemHeight={100}
            emptyMessage="No version families found."
            renderItem={(family: VersionFamily) => (
              <div
                className="list-row"
                style={{ flexDirection: 'column', alignItems: 'stretch', gap: 3, padding: '8px 10px' }}
              >
                {family.members.map((m) => (
                  <div key={m.path} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                    <span className="badge neutral">#{m.rank}</span>
                    {m.is_tip && <span className="badge tip">tip</span>}
                    <span className="truncate mono" style={{ flex: 1 }} title={m.path}>
                      {fileName(m.path)}
                    </span>
                    <span style={{ color: 'var(--text-dim)', fontSize: 11 }}>
                      {new Date(m.date_unix_millis).toLocaleDateString()} · {m.date_source.replace('_', ' ')}
                    </span>
                  </div>
                ))}
              </div>
            )}
          />

          {(collapse?.version_report.flagged_for_manual_merge.length ?? 0) > 0 && (
            <>
              <p className="section-title">Flagged for manual merge (diverged, never auto-picked)</p>
              <div className="list-container" style={{ maxHeight: 160 }}>
                {collapse!.version_report.flagged_for_manual_merge.map((p) => (
                  <div key={`${p.a}|${p.b}`} className="list-row" style={{ height: 30 }}>
                    <span className="truncate" title={p.a}>
                      {fileName(p.a)}
                    </span>
                    <span style={{ color: 'var(--text-dim)' }}>vs</span>
                    <span className="truncate" title={p.b}>
                      {fileName(p.b)}
                    </span>
                  </div>
                ))}
              </div>
            </>
          )}
        </>
      )}
    </div>
  )
}
