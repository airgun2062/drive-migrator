import { pickFolder } from '../api'
import type { ScanSettings } from '../types'

interface Props {
  settings: ScanSettings
  onChange: (settings: ScanSettings) => void
  onScan: () => void
  scanning: boolean
  error: string | null
}

export function ScanScreen({ settings, onChange, onScan, scanning, error }: Props) {
  function set<K extends keyof ScanSettings>(key: K, value: ScanSettings[K]) {
    onChange({ ...settings, [key]: value })
  }

  async function browse(key: 'source' | 'destination') {
    const path = await pickFolder()
    if (path) set(key, path)
  }

  const canScan = settings.source.trim() !== '' && settings.destination.trim() !== '' && !scanning

  return (
    <div className="screen">
      <h2>Scan</h2>
      <div className="form-card">
        <div className="field-row">
          <label>Source</label>
          <input
            type="text"
            value={settings.source}
            onChange={(e) => set('source', e.target.value)}
            placeholder="Folder to migrate from"
          />
          <button className="secondary-button" onClick={() => browse('source')}>
            Browse…
          </button>
        </div>
        <div className="field-row">
          <label>Destination</label>
          <input
            type="text"
            value={settings.destination}
            onChange={(e) => set('destination', e.target.value)}
            placeholder="Folder to migrate to"
          />
          <button className="secondary-button" onClick={() => browse('destination')}>
            Browse…
          </button>
        </div>

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

        {settings.mode === 'collapse' && (
          <>
            <div className="field-row">
              <label>Keep newest</label>
              <input
                type="number"
                min={1}
                value={settings.keepNewest}
                onChange={(e) => set('keepNewest', Math.max(1, Number(e.target.value)))}
              />
            </div>
            <div className="checkbox-row">
              <input
                type="checkbox"
                id="archive"
                checked={settings.archive}
                onChange={(e) => set('archive', e.target.checked)}
              />
              <label htmlFor="archive">Archive collapsed versions to _collapsed/ instead of dropping them</label>
            </div>
            <div className="field-row">
              <label>Protected extensions</label>
              <input
                type="text"
                value={settings.protectedExtensions.join(', ')}
                onChange={(e) =>
                  set(
                    'protectedExtensions',
                    e.target.value
                      .split(',')
                      .map((s) => s.trim())
                      .filter(Boolean),
                  )
                }
                placeholder="raw, dat (never collapsed as a version)"
              />
            </div>
          </>
        )}

        <p className="section-title">Settings</p>
        <div className="checkbox-row">
          <input
            type="checkbox"
            id="onMoved"
            checked={settings.onMovedCopy}
            onChange={(e) => set('onMovedCopy', e.target.checked)}
          />
          <label htmlFor="onMoved">
            Copy files whose content already exists at a different destination path
          </label>
        </div>
        <div className="checkbox-row">
          <input
            type="checkbox"
            id="fat32"
            checked={settings.fat32}
            onChange={(e) => set('fat32', e.target.checked)}
          />
          <label htmlFor="fat32">Reject files over the FAT32 4 GiB limit</label>
        </div>
        <div className="field-row">
          <label>Max path length</label>
          <input
            type="number"
            min={1}
            value={settings.maxPathLength}
            onChange={(e) => set('maxPathLength', Math.max(1, Number(e.target.value)))}
          />
        </div>

        {error && <div className="error-banner">{error}</div>}

        <button className="primary-button" disabled={!canScan} onClick={onScan}>
          {scanning ? 'Scanning…' : 'Scan'}
        </button>
      </div>
    </div>
  )
}
