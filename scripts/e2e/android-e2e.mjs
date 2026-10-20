// Emulator end-to-end test: setup, activate and release through the real UI.
// The device-side Syncthing REST is a small double; the hub status server is
// real. The real Syncthing selection facts are covered by the Rust live suite.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { mkdir } from 'node:fs/promises';

const { chromium } = createRequire(new URL('../../app/package.json', import.meta.url))('playwright');
const serial = process.env.ANDROID_SERIAL ?? 'emulator-5560';
const appId = process.env.APP_ID ?? 'app.photosync.debug';
const adb = (...args) => execFileSync('adb', ['-s', serial, ...args], { encoding: 'utf8' }).trim();
const root = new URL('../../', import.meta.url).pathname;
const deviceRoot = '/sdcard/Download/photosync-e2e';
const token = process.env.STATUS_TOKEN ?? 'e2e-status-token-0123456789';
const syncthingHost = process.env.E2E_SYNCTHING_HOST ?? 'http://127.0.0.1:18384';
const statusHost = process.env.E2E_STATUS_HOST ?? 'http://127.0.0.1:18787';

const pid = adb('shell', 'pidof', appId);
assert.ok(pid, 'the app is running');
const port = adb('forward', 'tcp:0', `localabstract:webview_devtools_remote_${pid}`);

let browser;
try {
  const readyBy = Date.now() + 15_000;
  while (true) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      if (targets.some((target) => JSON.parse(target.description || '{}').attached)) break;
    } catch {}
    assert.ok(Date.now() < readyBy, 'WebView DevTools becomes ready');
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  browser = await chromium.connectOverCDP(`http://127.0.0.1:${port}`, { noDefaults: true });
  const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const target = targets.find((candidate) =>
    candidate.url.startsWith('http://tauri.localhost/') &&
    JSON.parse(candidate.description || '{}').attached
  );
  assert.ok(target, 'the app WebView is attached');
  let page;
  for (const candidate of browser.contexts().flatMap((context) => context.pages())) {
    const session = await candidate.context().newCDPSession(candidate);
    const { targetInfo } = await session.send('Target.getTargetInfo');
    await session.detach();
    if (targetInfo.targetId === target.id) { page = candidate; break; }
  }
  assert.ok(page, 'found the app page');

  await page.waitForSelector('.brand', { timeout: 20_000 });
  const setupVisible = await page.locator('#syncthing-url').count();
  if (setupVisible) {
    await page.fill('#syncthing-url', 'http://127.0.0.1:8384');
    await page.fill('#api-key', process.env.API_KEY ?? 'e2e-api-key');
    await page.fill('#folder-id', 'photos');
    await page.fill('#device-name', 'e2e');
    await page.fill('#folder-root', deviceRoot);
    await page.fill('#status-url', 'http://127.0.0.1:8787');
    await page.fill('#status-token', token);
    await page.fill('#always-on', 'inbox/PixelMob');
    await page.getByRole('button', { name: 'Save and check' }).click();
    await page.getByRole('button', { name: 'Bootstrap folder' }).waitFor({ timeout: 20_000 });
    await page.getByRole('button', { name: 'Bootstrap folder' }).click();
    await page.waitForTimeout(1500);
  }

  await page.getByRole('button', { name: 'Projects' }).waitFor({ timeout: 20_000 });
  await page.getByRole('button', { name: 'Projects' }).click();
  const card = page.locator('.project', { hasText: 'Demo' }).first();
  try {
    await card.waitFor({ timeout: 20_000 });
  } catch (error) {
    const text = await page.locator('main').innerText();
    await page.screenshot({ path: `${root}/.data/e2e/projects-failure.png` });
    throw new Error(`Demo project did not appear. UI said:\n${text}`, { cause: error });
  }
  assert.equal(await card.getByText('not on device').count(), 1, 'Demo starts inactive');

  // The double reports the profile lines the app is about to activate. It does
  // not need to read the device; the release step only checks that the lines
  // disappear after the profile rewrite.
  await fetch(`${syncthingHost}/test/state`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      active: ['originals/Projects/Demo', 'published/Projects/Demo'],
      release: false,
    }),
  });
  await card.getByRole('button', { name: 'Activate' }).click();
  await page.locator('.project', { hasText: 'Demo' }).locator('.badge', { hasText: 'active' }).waitFor({ timeout: 20_000 });

  await fetch(`${syncthingHost}/test/state`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ active: [], release: true }),
  });
  await page.locator('.project', { hasText: 'Demo' }).getByRole('button', { name: 'Release…' }).click();
  await page.getByRole('button', { name: 'Release with gate' }).click();
  const modal = page.locator('.modal');
  await modal.getByText(/Release (completed|failed|aborted)/).waitFor({ timeout: 90_000 });
  const modalText = await modal.innerText();
  if (!modalText.includes('Release completed')) {
    assert.fail(`release did not complete:\n${modalText}`);
  }
  const check = execFileSync('adb', ['-s', serial, 'shell', `test -e '${deviceRoot}/originals/Projects/Demo/a.jpg' && echo yes || echo no`], { encoding: 'utf8' }).trim();
  assert.equal(check, 'no', 'release deleted the local originals');
  const publishedCheck = execFileSync('adb', ['-s', serial, 'shell', `test -e '${deviceRoot}/published/Projects/Demo/a.jpg' && echo yes || echo no`], { encoding: 'utf8' }).trim();
  assert.equal(publishedCheck, 'no', 'release deleted the local published twin');

  const response = await fetch(`${statusHost}/v1/tree?prefix=originals/Projects/Demo`, {
    headers: { authorization: `Bearer ${token}` },
  });
  assert.equal(response.status, 200, 'hub status server stayed reachable');
  const tree = await response.json();
  assert.ok(
    tree.entries.some((entry) => entry.path === 'originals/Projects/Demo/a.jpg'),
    'the hub still has the released file'
  );

  await mkdir(`${root}/.data/e2e`, { recursive: true });
  await page.screenshot({ path: `${root}/.data/e2e/release-complete.png` });
  console.log('emulator e2e passed');
} finally {
  try { adb('forward', '--remove', `tcp:${port}`); } catch {}
  await browser?.close();
}
