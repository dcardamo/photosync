// A tiny device-side Syncthing REST double for the emulator E2E test. It is
// not a sync engine: the real Syncthing facts live in the Rust live suite.
import http from 'node:http';

const port = Number(process.env.PORT ?? 8384);
const apiKey = process.env.API_KEY ?? 'e2e-api-key';
let active = [];
let release = false;

function json(res, status, value) {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(value));
}

function readBody(req) {
  return new Promise((resolve) => {
    let data = '';
    req.on('data', (chunk) => { data += chunk; });
    req.on('end', () => { resolve(data ? JSON.parse(data) : {}); });
  });
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (url.pathname === '/test/health') return json(res, 200, { ok: true });
  if (url.pathname === '/test/state' && req.method === 'POST') {
    const body = await readBody(req);
    if (Array.isArray(body.active)) active = body.active;
    if (typeof body.release === 'boolean') release = body.release;
    return json(res, 200, { active, release });
  }
  if (req.headers['x-api-key'] !== apiKey) return json(res, 403, { error: 'bad api key' });

  if (url.pathname === '/rest/system/status') {
    return json(res, 200, { myID: 'E2E-DEVICE-0000000-0000000-0000000-0000000' });
  }
  if (url.pathname === '/rest/system/connections') {
    return json(res, 200, { connections: {} });
  }
  if (url.pathname === '/rest/db/status') {
    return json(res, 200, {
      state: 'idle',
      stateChanged: new Date().toISOString(),
      error: '',
      errors: 0,
      pullErrors: 0,
      needFiles: 0,
      needBytes: 0,
      globalFiles: 0,
      globalBytes: 0,
      localFiles: 0,
      localBytes: 0,
    });
  }
  if (url.pathname === '/rest/db/scan' && req.method === 'POST') {
    const sub = url.searchParams.get('sub');
    if (sub === '.stprofiles' && release) active = [];
    res.writeHead(200);
    return res.end();
  }
  if (url.pathname === '/rest/db/ignores') {
    return json(res, 200, {
      ignore: ['#include .stprofiles/e2e.txt', '!/.stprofiles', '*'],
      expanded: active.map((path) => `!/${path}`),
    });
  }
  if (url.pathname === '/rest/db/browse') return json(res, 200, []);
  return json(res, 404, { error: 'not found' });
});

server.listen(port, '127.0.0.1', () => {
  console.log(`fake syncthing on http://127.0.0.1:${port}`);
});
