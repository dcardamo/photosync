export interface ProjectLayout {
  originals: string
  years: string
  projects: string
  published: string
}

export interface Settings {
  syncthing_url: string
  folder_id: string
  device_name: string
  folder_root: string
  status_server_url: string
  headroom_percent: number
  always_on: string[]
  layout: ProjectLayout
  cached_base_ignores: string[]
  journal_dir: string
}

export interface SettingsView {
  settings: Settings
  apiKeySet: boolean
  statusTokenSet: boolean
  configured: boolean
}

export interface StorageInfo {
  path: string
  totalBytes: number
  freeBytes: number
}

export interface BootstrapView {
  created: boolean
  present: boolean
  mismatch: boolean
  existing: string | null
  profilePath: string
  stignorePath: string
}

export interface SetupCheck {
  syncthingOk: boolean
  syncthingError: string | null
  folderOk: boolean
  folderState: string
  folderError: string | null
  statusOk: boolean
  statusError: string | null
  allFiles: boolean
  profilePresent: boolean
  stignorePresent: boolean
  stignoreDevice: string | null
  baseIgnores: number
  storage: StorageInfo | null
}

export interface ProjectView {
  path: string
  name: string
  year: number | null
  kind: 'year' | 'project'
  published: string
  active: boolean
  hubSize: number
  localBytes: number
  files: number
  mtimeNs: number
  stale: boolean
}

export interface ProjectsView {
  projects: ProjectView[]
  stale: boolean
  error: string | null
}

export interface CapacityView {
  totalBytes: number
  freeBytes: number
  activeBytes: number
  projectedFreeBytes: number
  headroomBytes: number
  headroomPercent: number
}

export interface StepView {
  id: number
  name: string
  status: 'pending' | 'running' | 'passed' | 'failed' | 'skipped'
  detail: string
  at: string
}

export interface ReleaseRecord {
  id: string
  project: {
    kind: 'year' | 'project'
    year: number | null
    name: string
    originals: string
    published: string
  }
  phase: string
  steps: StepView[]
  error: string | null
  started_at: string
  updated_at: string
  verified: { path: string; size: number; mtime_ns: number; ctime_ns: number }[]
  deletion_started: boolean
  dry_run: boolean
}

export interface PendingProject {
  path: string
  name: string
  pendingFiles: number
  pendingBytes: number
  hubMissing: number
  mismatch: number
}

export interface StatusView {
  syncthingOk: boolean
  syncthingError: string | null
  folderState: string
  folderError: string | null
  hubOk: boolean
  hubError: string | null
  backup: string | null
  pending: PendingProject[]
  storage: StorageInfo | null
}

export interface ReleaseHistory {
  latest: ReleaseRecord | null
  records: ReleaseRecord[]
}

export interface SaveSettingsInput {
  settings: Settings
  apiKey: string | null
  statusToken: string | null
}
