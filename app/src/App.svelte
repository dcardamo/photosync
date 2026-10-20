<script lang="ts">
  import { onMount } from 'svelte'
  import { api, message } from './lib/api'
  import type {
    BootstrapView,
    CapacityView,
    ProjectView,
    ProjectsView,
    ReleaseHistory,
    ReleaseRecord,
    Settings,
    SettingsView,
    SetupCheck,
    StatusView,
  } from './lib/types'

  let settingsView: SettingsView | null = null
  let draft: Settings | null = null
  let setup: SetupCheck | null = null
  let bootstrapResult: BootstrapView | null = null
  let projectsView: ProjectsView | null = null
  let capacityView: CapacityView | null = null
  let statusView: StatusView | null = null
  let history: ReleaseHistory | null = null
  let activeRelease: ReleaseRecord | null = null

  let tab: 'projects' | 'status' | 'settings' | 'setup' = 'projects'
  let busy = ''
  let error = ''
  let notice = ''
  let search = ''
  let year = 'all'
  let showNew = false
  let newName = ''
  let confirmRelease: ProjectView | null = null
  let apiKey = ''
  let statusToken = ''
  let unlisten: (() => void) | null = null
  let refreshing = false

  $: configured = settingsView?.configured ?? false
  $: years = [...new Set((projectsView?.projects ?? [])
    .map((p) => p.year)
    .filter((y): y is number => typeof y === 'number'))].sort((a, b) => b - a)
  $: filtered = (projectsView?.projects ?? []).filter((p) =>
    (year === 'all' || String(p.year) === year) &&
    p.name.toLowerCase().includes(search.toLowerCase())
  )
  $: activeCount = (projectsView?.projects ?? []).filter((p) => p.active).length
  $: usedPercent = capacityView && capacityView.totalBytes > 0
    ? Math.min(100, (capacityView.activeBytes / capacityView.totalBytes) * 100)
    : 0

  onMount(() => {
    void boot()
    const timer = setInterval(() => {
      const downloading = (projectsView?.projects ?? []).some((project) =>
        project.active && project.hubSize > project.localBytes
      )
      if (configured && tab === 'projects' && !busy && downloading) void refresh()
    }, 5000)
    return () => { unlisten?.(); clearInterval(timer) }
  })

  async function boot() {
    busy = 'Loading…'
    try {
      const view = await api.settings()
      settingsView = view
      draft = cloneSettings(view.settings)
      if (!view.configured) {
        tab = 'setup'
      } else {
        await refresh()
        await runSetupCheck()
        try {
          const recovered = await api.recoverRelease()
          if (recovered) activeRelease = recovered
        } catch (e) {
          error = message(e)
        }
      }
      unlisten = await api.onReleaseProgress((record) => { activeRelease = record })
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  function cloneSettings(value: Settings): Settings {
    return {
      ...value,
      always_on: [...value.always_on],
      layout: { ...value.layout },
      cached_base_ignores: [...value.cached_base_ignores],
    }
  }

  async function refresh() {
    if (refreshing) return
    refreshing = true
    try {
      const [p, c, s, h] = await Promise.allSettled([
        api.projects(), api.capacity(), api.status(), api.releaseHistory(),
      ])
      if (p.status === 'fulfilled') projectsView = p.value
      if (c.status === 'fulfilled') capacityView = c.value
      if (s.status === 'fulfilled') statusView = s.value
      if (h.status === 'fulfilled') history = h.value
      const failure = [p, c, s, h].find((r) => r.status === 'rejected')
      if (failure && failure.status === 'rejected') error = message(failure.reason)
    } finally {
      refreshing = false
    }
  }

  async function saveSettings() {
    if (!draft) return
    busy = 'Saving…'
    error = ''
    notice = ''
    try {
      const view = await api.saveSettings({
        settings: draft,
        apiKey: apiKey.length > 0 ? apiKey : null,
        statusToken: statusToken.length > 0 ? statusToken : null,
      })
      settingsView = view
      draft = cloneSettings(view.settings)
      apiKey = ''
      statusToken = ''
      notice = 'Settings saved.'
      await runSetupCheck()
      if (view.configured) await refresh()
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  async function runSetupCheck() {
    try {
      setup = await api.setupCheck()
      if (setup.storage) {
        draft = draft ? { ...draft } : draft
      }
    } catch (e) {
      error = message(e)
    }
  }

  async function bootstrapFolder() {
    busy = 'Bootstrapping…'
    error = ''
    notice = ''
    try {
      bootstrapResult = await api.bootstrapFolder()
      if (bootstrapResult.mismatch) {
        error = 'The existing .stignore belongs to something else. photosync will not overwrite it. Remove or move it, then try again.'
      } else {
        notice = bootstrapResult.created
          ? 'Profile and .stignore written.'
          : 'Profile and .stignore were already in place.'
      }
      await runSetupCheck()
      await refresh()
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  async function requestStorage() {
    busy = 'Requesting access…'
    try {
      await api.requestAllFilesAccess()
      await runSetupCheck()
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  async function activate(project: ProjectView) {
    busy = `Activating ${project.name}…`
    error = ''
    notice = ''
    try {
      await api.activate(project.path)
      notice = `${project.name} is active. Syncthing will pull it now.`
      await refresh()
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  async function createProject() {
    if (!newName.trim()) return
    busy = 'Creating…'
    error = ''
    try {
      const project = await api.newProject(newName.trim())
      showNew = false
      newName = ''
      notice = `${project.name} created and activated locally.`
      await refresh()
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  async function runRelease() {
    const project = confirmRelease
    if (!project) return
    confirmRelease = null
    busy = `Releasing ${project.name}…`
    error = ''
    notice = ''
    activeRelease = null
    try {
      const record = await api.release(project.path)
      activeRelease = record
      if (record.phase === 'completed') {
        notice = `${project.name} released. Local files are gone; the hub still has them.`
      } else if (record.phase === 'aborted') {
        notice = 'Release aborted; the project remains active.'
      } else {
        error = record.error ?? `Release ended in ${phaseLabel(record.phase)}.`
      }
      await refresh()
    } catch (e) {
      error = message(e)
      await refresh()
    } finally {
      busy = ''
    }
  }

  async function recover() {
    busy = 'Recovering…'
    try {
      const record = await api.recoverRelease()
      if (record) activeRelease = record
      await refresh()
    } catch (e) {
      error = message(e)
    } finally {
      busy = ''
    }
  }

  function toggle(project: ProjectView) {
    if (project.active) {
      confirmRelease = project
    } else {
      void activate(project)
    }
  }

  function bytes(value: number): string {
    if (!Number.isFinite(value) || value <= 0) return '0 B'
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']
    let at = 0
    let n = value
    while (n >= 1024 && at < units.length - 1) { n /= 1024; at++ }
    return `${n >= 10 || at === 0 ? n.toFixed(0) : n.toFixed(1)} ${units[at]}`
  }

  function date(value: string | null | undefined): string {
    if (!value) return 'never'
    const parsed = new Date(value)
    return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString()
  }

  function phaseLabel(phase: string): string {
    return phase.replaceAll('_', ' ')
  }

  function stepIcon(status: string): string {
    return status === 'passed' ? '✓' : status === 'failed' ? '✕' : status === 'running' ? '●' : status === 'skipped' ? '–' : '○'
  }

  function updateAlwaysOn(value: string) {
    if (!draft) return
    draft.always_on = value.split(',').map((v) => v.trim()).filter(Boolean)
  }
</script>

<header class="top">
  <div class="top-inner">
    <div class="brand">photo<span>sync</span></div>
    <span class="dot {statusView?.syncthingOk ? 'ok' : configured ? 'bad' : ''}" title="Syncthing connection"></span>
    <span class="muted small grow">
      {#if busy}{busy}
      {:else if statusView?.folderState}{statusView.folderState}
      {:else}local selection for the travel device{/if}
    </span>
    {#if configured}
      <button class="ghost small" on:click={() => void refresh()} disabled={!!busy}>Refresh</button>
    {/if}
  </div>
  {#if configured}
    <nav class="tabs">
      <button class:active={tab === 'projects'} on:click={() => (tab = 'projects')}>Projects</button>
      <button class:active={tab === 'status'} on:click={() => (tab = 'status')}>Status</button>
      <button class:active={tab === 'settings'} on:click={() => (tab = 'settings')}>Settings</button>
    </nav>
  {/if}
</header>

<main class="shell">
  {#if error}<div class="error">{error}</div>{/if}
  {#if notice}<div class="notice">{notice}</div>{/if}

  {#if !configured || tab === 'setup'}
    <div class="section-title"><h1>Set up photosync</h1></div>
    <p class="hint">
      Syncthing-Fork already shares the folder with the hub. photosync only writes
      the selection profile inside that folder; it never edits Syncthing's config.
    </p>

    {#if draft}
      <section class="card">
        <h2>1. Syncthing on this device</h2>
        <p class="hint">The local Syncthing-Fork REST API. Cleartext is allowed only for localhost.</p>
        <div class="field">
          <label for="syncthing-url">REST URL</label>
          <input id="syncthing-url" bind:value={draft.syncthing_url} placeholder="http://127.0.0.1:8384" />
        </div>
        <div class="field">
          <label for="api-key">API key {settingsView?.apiKeySet ? '(saved; leave blank to keep)' : ''}</label>
          <input id="api-key" type="password" bind:value={apiKey} placeholder="Actions → Settings → API key" autocomplete="off" />
        </div>
        <div class="grid2">
          <div class="field">
            <label for="folder-id">Folder ID</label>
            <input id="folder-id" bind:value={draft.folder_id} placeholder="photos" />
          </div>
          <div class="field">
            <label for="device-name">Device profile name</label>
            <input id="device-name" bind:value={draft.device_name} placeholder="pixelmob" />
          </div>
        </div>
        <div class="field">
          <label for="folder-root">Folder root on this device</label>
          <input id="folder-root" bind:value={draft.folder_root} placeholder="/storage/emulated/0/Photos" />
        </div>
      </section>

      <section class="card">
        <h2>2. Hub status server</h2>
        <p class="hint">HTTPS over the tailnet. The release gate reads the hub's disk, not Syncthing's view of it.</p>
        <div class="field">
          <label for="status-url">Status server URL</label>
          <input id="status-url" bind:value={draft.status_server_url} placeholder="https://hub.example.ts.net" />
        </div>
        <div class="field">
          <label for="status-token">Bearer token {settingsView?.statusTokenSet ? '(saved; leave blank to keep)' : ''}</label>
          <input id="status-token" type="password" bind:value={statusToken} placeholder="token from the hub" autocomplete="off" />
        </div>
        <div class="field">
          <label for="always-on">Always-on paths (comma separated)</label>
          <input id="always-on" value={draft.always_on.join(', ')} on:input={(e) => updateAlwaysOn((e.currentTarget as HTMLInputElement).value)} placeholder="inbox/PixelMob" />
        </div>
      </section>

      <section class="card">
        <h2>3. Storage and safety</h2>
        <div class="row wrap">
          <span class="badge" class:active={setup?.allFiles}>{setup?.allFiles ? 'All files access granted' : 'All files access required'}</span>
          <button on:click={() => void requestStorage()} disabled={!!busy}>Grant all files access</button>
        </div>
        <p class="hint">photosync must walk and delete inside the pool. A refusal leaves the app usable for setup, but activation and release stay disabled.</p>
        <div class="field">
          <label for="headroom">Free-space headroom (%)</label>
          <input id="headroom" type="number" min="0" max="100" bind:value={draft.headroom_percent} />
        </div>
      </section>

      <div class="row wrap sticky-actions">
        <button class="primary" on:click={() => void saveSettings()} disabled={!!busy}>Save and check</button>
        <button on:click={() => void bootstrapFolder()} disabled={!!busy}>Bootstrap folder</button>
      </div>
    {/if}

    {#if setup}
      <section class="card">
        <h2>Setup check</h2>
        <ul class="list">
          <li><span class="dot" class:ok={setup.syncthingOk} class:bad={!setup.syncthingOk}></span> Syncthing API — {setup.syncthingOk ? 'reachable' : setup.syncthingError ?? 'unreachable'}</li>
          <li><span class="dot" class:ok={setup.folderOk} class:bad={!setup.folderOk}></span> Folder — {setup.folderOk ? setup.folderState : setup.folderError ?? 'not available'}</li>
          <li><span class="dot" class:ok={setup.statusOk} class:bad={!setup.statusOk}></span> Hub status server — {setup.statusOk ? `${setup.baseIgnores} base ignore lines` : setup.statusError ?? 'unreachable'}</li>
          <li><span class="dot" class:ok={setup.profilePresent} class:bad={!setup.profilePresent}></span> Profile file — {setup.profilePresent ? 'present' : 'missing'}</li>
          <li><span class="dot" class:ok={setup.stignorePresent} class:bad={!setup.stignorePresent}></span> .stignore — {setup.stignorePresent ? `points at ${setup.stignoreDevice ?? 'another device'}` : 'missing'}</li>
          <li><span class="dot" class:ok={setup.allFiles} class:bad={!setup.allFiles}></span> All files access — {setup.allFiles ? 'granted' : 'refused'}</li>
        </ul>
        {#if setup.storage}
          <p class="hint">Pool: {bytes(setup.storage.freeBytes)} free of {bytes(setup.storage.totalBytes)}.</p>
        {/if}
      </section>
    {/if}

    {#if bootstrapResult?.mismatch}
      <div class="error">
        Existing .stignore was left alone. It is not the three-line photosync bootstrap:
        <pre class="mono">{bootstrapResult.existing}</pre>
      </div>
    {/if}
  {:else}
    {#if tab === 'projects'}
      <div class="section-title">
        <h1>Projects</h1>
        <span class="muted small">{activeCount} active</span>
      </div>

      {#if capacityView}
        <section class="capacity">
          <div class="meta">
            <span>{activeCount} active project{activeCount === 1 ? '' : 's'} · {bytes(capacityView.activeBytes)}</span>
            <span>{bytes(capacityView.freeBytes)} free</span>
          </div>
          <div class="bar" class:over={capacityView.projectedFreeBytes < capacityView.headroomBytes}>
            <div style={`width:${usedPercent}%`}></div>
          </div>
          <div class="meta">
            <span>Headroom {capacityView.headroomPercent}% ({bytes(capacityView.headroomBytes)})</span>
            <span>{bytes(capacityView.totalBytes)} total</span>
          </div>
        </section>
      {/if}

      <div class="row wrap" style="margin-top:0.9rem">
        <input class="grow" bind:value={search} placeholder="Search projects" />
        <select bind:value={year} style="width:auto">
          <option value="all">All years</option>
          {#each years as y}<option value={String(y)}>{y}</option>{/each}
        </select>
        <button class="primary" on:click={() => (showNew = true)}>New project</button>
      </div>

      {#if projectsView?.stale}
        <div class="notice">The hub is unreachable; showing the last project list. Sizes may be stale.</div>
      {/if}

      <section class="card">
        {#if filtered.length === 0}
          <p class="hint">No projects{search ? ' match this search' : ' yet'}. Create one, or refresh after the hub is reachable.</p>
        {:else}
          {#each filtered as project (project.path)}
            <div class="project">
              <div class="grow">
                <div class="name">{project.name}</div>
                <div class="path">{project.path}</div>
                <div class="small muted">{bytes(project.hubSize)} · {project.files} files{project.year ? ` · ${project.year}` : ''}</div>
                {#if project.active && project.hubSize > 0}
                  <div class="bar small"><div style={`width:${Math.min(100, (project.localBytes / project.hubSize) * 100)}%`}></div></div>
                  <div class="small muted">{bytes(project.localBytes)} of {bytes(project.hubSize)} on device</div>
                {/if}
              </div>
              <div class="side">
                <span class="badge" class:active={project.active}>{project.active ? 'active' : 'not on device'}</span>
                {#if project.active}
                  <button class="danger small" on:click={() => toggle(project)} disabled={!!busy}>Release…</button>
                {:else}
                  <button class="small" on:click={() => toggle(project)} disabled={!!busy || !setup?.allFiles}>Activate</button>
                {/if}
              </div>
            </div>
          {/each}
        {/if}
      </section>
    {:else if tab === 'status'}
      <div class="section-title"><h1>Status</h1><button class="ghost" on:click={() => void refresh()} disabled={!!busy}>Refresh</button></div>
      <section class="card">
        <h2>Syncthing</h2>
        <p class="hint">{statusView?.folderState ?? 'unknown'} {statusView?.folderError ? `· ${statusView.folderError}` : ''}</p>
        <p class="hint">{statusView?.syncthingOk ? 'Local API reachable.' : statusView?.syncthingError ?? 'Local API unreachable.'}</p>
      </section>
      <section class="card">
        <h2>Hub</h2>
        <p class="hint">{statusView?.hubOk ? 'Status server reachable.' : statusView?.hubError ?? 'Status server unreachable.'}</p>
        <p class="hint">Last successful backup: {date(statusView?.backup)}</p>
        {#if statusView?.storage}
          <p class="hint">Pool: {bytes(statusView.storage.freeBytes)} free of {bytes(statusView.storage.totalBytes)}.</p>
        {/if}
      </section>
      <section class="card">
        <h2>Pending uploads by project</h2>
        <p class="hint">Derived from the device's files against the hub's disk tree. Syncthing's completion view is not used as a safety signal.</p>
        {#if (statusView?.pending ?? []).length === 0}
          <p class="hint">Nothing pending.</p>
        {:else}
          <ul class="list">
            {#each statusView?.pending ?? [] as item}
              <li>
                <div class="row">
                  <span class="grow">{item.name}</span>
                  <span class="badge warn">{item.pendingFiles} file{item.pendingFiles === 1 ? '' : 's'}</span>
                </div>
                <div class="small muted">{bytes(item.pendingBytes)} · {item.hubMissing} missing on hub · {item.mismatch} mismatched</div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
      {#if history?.latest && !['completed', 'aborted', 'failed', 'dry_run_complete'].includes(history.latest.phase)}
        <section class="card">
          <h2>Interrupted release</h2>
          <p class="hint">The app was killed during a release. Recover before doing anything else.</p>
          <button class="primary" on:click={() => void recover()} disabled={!!busy}>Recover</button>
        </section>
      {/if}
    {:else}
      <div class="section-title"><h1>Settings</h1></div>
      {#if draft}
        <section class="card">
          <h2>Syncthing</h2>
          <div class="field"><label for="s-url">REST URL</label><input id="s-url" bind:value={draft.syncthing_url} /></div>
          <div class="field"><label for="s-key">API key {settingsView?.apiKeySet ? '(saved)' : ''}</label><input id="s-key" type="password" bind:value={apiKey} placeholder="leave blank to keep" /></div>
          <div class="grid2">
            <div class="field"><label for="s-folder">Folder ID</label><input id="s-folder" bind:value={draft.folder_id} /></div>
            <div class="field"><label for="s-device">Device profile</label><input id="s-device" bind:value={draft.device_name} /></div>
          </div>
          <div class="field"><label for="s-root">Folder root</label><input id="s-root" bind:value={draft.folder_root} /></div>
        </section>
        <section class="card">
          <h2>Hub</h2>
          <div class="field"><label for="s-status">Status server URL</label><input id="s-status" bind:value={draft.status_server_url} /></div>
          <div class="field"><label for="s-token">Bearer token {settingsView?.statusTokenSet ? '(saved)' : ''}</label><input id="s-token" type="password" bind:value={statusToken} placeholder="leave blank to keep" /></div>
          <div class="field"><label for="s-always">Always-on paths</label><input id="s-always" value={draft.always_on.join(', ')} on:input={(e) => updateAlwaysOn((e.currentTarget as HTMLInputElement).value)} /></div>
          <div class="field"><label for="s-headroom">Headroom (%)</label><input id="s-headroom" type="number" min="0" max="100" bind:value={draft.headroom_percent} /></div>
        </section>
        <section class="card">
          <h2>Folder files</h2>
          <div class="row wrap">
            <span class="badge" class:active={setup?.allFiles}>{setup?.allFiles ? 'All files access' : 'No all files access'}</span>
            <button on:click={() => void requestStorage()} disabled={!!busy}>Grant access</button>
            <button on:click={() => void bootstrapFolder()} disabled={!!busy}>Bootstrap folder</button>
          </div>
          <p class="hint">The .stignore is written once. The profile is rewritten on every activation and release.</p>
        </section>
        <div class="row wrap sticky-actions">
          <button class="primary" on:click={() => void saveSettings()} disabled={!!busy}>Save settings</button>
          <button on:click={() => void runSetupCheck()} disabled={!!busy}>Run setup check</button>
        </div>
      {/if}

      <section class="card">
        <h2>Release history</h2>
        {#if !history || history.records.length === 0}
          <p class="hint">No releases yet.</p>
        {:else}
          <ul class="list">
            {#each history.records as record}
              <li>
                <div class="row">
                  <span class="grow">{record.project.name}</span>
                  <span class="badge" class:warn={record.phase === 'failed'} class:active={record.phase === 'completed'}>{phaseLabel(record.phase)}</span>
                </div>
                <div class="small muted">{date(record.started_at)}{record.error ? ` · ${record.error}` : ''}</div>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}
  {/if}
</main>

{#if showNew}
  <div class="modal-backdrop" role="presentation" on:click={() => (showNew = false)} on:keydown={(e) => e.key === 'Escape' && (showNew = false)}>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="modal" role="dialog" aria-modal="true" tabindex="-1" on:click|stopPropagation>
      <h2>New project</h2>
      <p class="hint">Creates a dated project under {draft?.layout.originals}/{draft?.layout.years}/ and activates it. Works offline.</p>
      <div class="field">
        <label for="new-name">Project name</label>
        <input id="new-name" bind:value={newName} placeholder="Coast" on:keydown={(e) => e.key === 'Enter' && void createProject()} />
      </div>
      <div class="row">
        <button class="primary grow" on:click={() => void createProject()} disabled={!!busy || !newName.trim()}>Create and activate</button>
        <button on:click={() => (showNew = false)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if confirmRelease}
  <div class="modal-backdrop" role="presentation" on:click={() => (confirmRelease = null)} on:keydown={(e) => e.key === 'Escape' && (confirmRelease = null)}>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="modal" role="dialog" aria-modal="true" tabindex="-1" on:click|stopPropagation>
      <h2>Release {confirmRelease.name}?</h2>
      <p class="hint">
        photosync removes the project from the profile, waits for the device to reload its ignores,
        then deletes the local copy. Every file must already be on the hub with the same size and
        mtime, and the hub backup must be newer. Any failure aborts and leaves the project active.
      </p>
      <div class="row">
        <button class="danger grow" on:click={() => void runRelease()} disabled={!!busy}>Release with gate</button>
        <button on:click={() => (confirmRelease = null)}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

{#if activeRelease}
  <div class="modal-backdrop" role="presentation" on:click={() => (activeRelease = null)} on:keydown={(e) => e.key === 'Escape' && (activeRelease = null)}>
    <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
    <div class="modal" role="dialog" aria-modal="true" tabindex="-1" on:click|stopPropagation>
      <h2>{activeRelease.project.name}</h2>
      <p class="hint">Release {phaseLabel(activeRelease.phase)} · started {date(activeRelease.started_at)}</p>
      <div class="steps">
        {#each activeRelease.steps as step}
          <div class="step {step.status}">
            <div class="icon">{stepIcon(step.status)}</div>
            <div class="grow">
              <div class="title">{step.id}. {step.name}</div>
              <div class="detail">{step.detail || (step.status === 'pending' ? 'waiting' : '')}</div>
            </div>
          </div>
        {/each}
      </div>
      {#if activeRelease.error}
        <div class="error" style="margin-top:0.8rem">{activeRelease.error}</div>
      {/if}
      <div class="row" style="margin-top:0.9rem">
        <button class="primary grow" on:click={() => (activeRelease = null)}>Close</button>
      </div>
    </div>
  </div>
{/if}
