import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  ManifestWriteResult,
  ScanBundle,
  ScanSettings,
  TransferProgressEvent,
  TransferSummary,
  VerifyReport,
} from './types'

export function pickFolder(): Promise<string | null> {
  return invoke('pick_folder')
}

export function scan(settings: ScanSettings): Promise<ScanBundle> {
  return invoke('scan', { settings })
}

export function transfer(settings: ScanSettings, dryRun: boolean): Promise<TransferSummary> {
  return invoke('transfer', { settings, dryRun })
}

export function writeManifest(destination: string, source?: string): Promise<ManifestWriteResult> {
  return invoke('write_manifest_cmd', { destination, source: source ?? null })
}

export function verify(destination: string): Promise<VerifyReport> {
  return invoke('verify_cmd', { destination })
}

export function hashFile(path: string): Promise<string> {
  return invoke('hash_file', { path })
}

export function onTransferProgress(
  handler: (event: TransferProgressEvent) => void,
): Promise<UnlistenFn> {
  return listen<TransferProgressEvent>('transfer-progress', (e) => handler(e.payload))
}
