import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type {
  BootstrapView,
  CapacityView,
  ProjectView,
  ProjectsView,
  ReleaseHistory,
  ReleaseRecord,
  SaveSettingsInput,
  SettingsView,
  SetupCheck,
  StatusView,
  StorageInfo,
} from './types'

export const api = {
  settings: () => invoke<SettingsView>('settings'),
  saveSettings: (input: SaveSettingsInput) => invoke<SettingsView>('save_settings', { input }),
  setupCheck: () => invoke<SetupCheck>('setup_check'),
  bootstrapFolder: () => invoke<BootstrapView>('bootstrap_folder'),
  projects: () => invoke<ProjectsView>('projects'),
  capacity: () => invoke<CapacityView>('capacity'),
  activate: (path: string) => invoke<ProjectView>('activate', { path }),
  newProject: (name: string) => invoke<ProjectView>('new_project', { name }),
  release: (path: string) => invoke<ReleaseRecord>('release', { path }),
  recoverRelease: () => invoke<ReleaseRecord | null>('recover_release'),
  status: () => invoke<StatusView>('status'),
  releaseHistory: () => invoke<ReleaseHistory>('release_history'),
  storageInfo: () => invoke<StorageInfo>('storage_info'),
  allFilesAccess: () => invoke<boolean>('all_files_access'),
  requestAllFilesAccess: () => invoke<boolean>('request_all_files_access'),
  onReleaseProgress: (handler: (record: ReleaseRecord) => void) =>
    listen<ReleaseRecord>('release-progress', (event) => handler(event.payload)),
}

export function message(error: unknown): string {
  if (typeof error === 'string') return error
  if (error instanceof Error) return error.message
  return JSON.stringify(error)
}
