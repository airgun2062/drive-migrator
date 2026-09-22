import { useCallback, useRef, useState } from 'react'
import './App.css'
import * as api from './api'
import { entriesToTransfer, totalBytes } from './planning'
import { ReconcileScreen } from './screens/ReconcileScreen'
import { ReviewScreen } from './screens/ReviewScreen'
import { ScanScreen } from './screens/ScanScreen'
import { TransferScreen } from './screens/TransferScreen'
import { VersionHistoryScreen } from './screens/VersionHistoryScreen'
import { defaultSettings, type ScanBundle, type ScanSettings, type TransferProgressEvent, type TransferSummary } from './types'

type Screen = 'scan' | 'reconcile' | 'review' | 'versions' | 'transfer'

const SCREENS: Array<{ id: Screen; label: string }> = [
  { id: 'scan', label: 'Scan' },
  { id: 'reconcile', label: 'Resume & reconcile' },
  { id: 'review', label: 'Review' },
  { id: 'versions', label: 'Version history' },
  { id: 'transfer', label: 'Transfer' },
]

function App() {
  const [screen, setScreen] = useState<Screen>('scan')
  const [settings, setSettings] = useState<ScanSettings>(defaultSettings())
  const [bundle, setBundle] = useState<ScanBundle | null>(null)
  const [scanning, setScanning] = useState(false)
  const [scanError, setScanError] = useState<string | null>(null)

  const [transferring, setTransferring] = useState(false)
  const [transferEvents, setTransferEvents] = useState<TransferProgressEvent[]>([])
  const [transferSummary, setTransferSummary] = useState<TransferSummary | null>(null)
  const [transferError, setTransferError] = useState<string | null>(null)
  const [transferStartedAt, setTransferStartedAt] = useState<number | null>(null)
  const unlistenRef = useRef<(() => void) | null>(null)

  const runScan = useCallback(async () => {
    setScanning(true)
    setScanError(null)
    try {
      const result = await api.scan(settings)
      setBundle(result)
      setTransferSummary(null)
      setTransferEvents([])
      setScreen('reconcile')
    } catch (err) {
      setScanError(String(err))
    } finally {
      setScanning(false)
    }
  }, [settings])

  const startTransfer = useCallback(
    async (dryRun: boolean) => {
      setTransferring(true)
      setTransferError(null)
      setTransferEvents([])
      setTransferSummary(null)
      setTransferStartedAt(Date.now())

      unlistenRef.current = await api.onTransferProgress((event) => {
        setTransferEvents((prev) => [...prev, event])
      })

      try {
        const result = await api.transfer(settings, dryRun)
        setTransferSummary(result)
      } catch (err) {
        setTransferError(String(err))
      } finally {
        setTransferring(false)
        unlistenRef.current?.()
        unlistenRef.current = null
      }
    },
    [settings],
  )

  const canLeaveScan = bundle !== null

  return (
    <div className="app">
      <header className="app-header">
        <h1>Drive Migrator</h1>
        {SCREENS.map((s) => (
          <button
            key={s.id}
            className={`nav-tab${screen === s.id ? ' active' : ''}`}
            disabled={s.id !== 'scan' && !canLeaveScan}
            onClick={() => setScreen(s.id)}
          >
            {s.label}
          </button>
        ))}
      </header>
      <div className="app-body">
        {screen === 'scan' && (
          <ScanScreen
            settings={settings}
            onChange={setSettings}
            onScan={runScan}
            scanning={scanning}
            error={scanError}
          />
        )}
        {screen === 'reconcile' && bundle && (
          <ReconcileScreen
            plan={bundle.plan}
            scanning={scanning}
            onRescan={runScan}
            onGoToTransfer={() => setScreen('transfer')}
          />
        )}
        {screen === 'review' && bundle && (
          <ReviewScreen analyze={bundle.analyze} similarity={bundle.similarity} collapse={bundle.collapse} />
        )}
        {screen === 'versions' && bundle && (
          <VersionHistoryScreen
            settings={settings}
            onChange={setSettings}
            onRescan={runScan}
            scanning={scanning}
            collapse={bundle.collapse}
            analyze={bundle.analyze}
          />
        )}
        {screen === 'transfer' && bundle && (
          <TransferScreen
            bundle={bundle}
            settings={settings}
            totalBytesToCopy={totalBytes(entriesToTransfer(bundle, settings))}
            totalFilesToCopy={entriesToTransfer(bundle, settings).length}
            transferring={transferring}
            events={transferEvents}
            summary={transferSummary}
            startedAt={transferStartedAt}
            error={transferError}
            onStart={startTransfer}
          />
        )}
      </div>
    </div>
  )
}

export default App
