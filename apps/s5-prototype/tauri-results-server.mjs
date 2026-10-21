import { createServer } from 'node:http';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import process from 'node:process';

const allowedOrigins = new Set(['tauri://localhost', 'http://tauri.localhost', 'null']);
const resultsDirectory = process.env.S5_BENCH_RESULTS_DIR || 'results';
const resultsPrefix = process.env.S5_BENCH_PREFIX || 'tauri';
const server = createServer(async (request, response) => {
  const origin = request.headers.origin;
  if (origin && !allowedOrigins.has(origin)) { response.writeHead(403).end(); return; }
  response.setHeader('Access-Control-Allow-Origin', origin ?? 'tauri://localhost');
  response.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
  response.setHeader('Access-Control-Allow-Headers', 'content-type');
  if (request.method === 'OPTIONS') { response.writeHead(204).end(); return; }
  if (request.method === 'GET' && request.url === '/health') { response.writeHead(200).end('ok'); return; }
  if (request.method !== 'POST' || request.url !== '/result') { response.writeHead(404).end(); return; }
  try {
    let body = '';
    for await (const chunk of request) {
      body += chunk;
      if (body.length > 1_000_000) throw new Error('result payload too large');
    }
    const { filename, result } = JSON.parse(body);
    if (typeof filename !== 'string' || !/^[a-zA-Z0-9-]+$/.test(filename) || typeof result !== 'object' || result === null) throw new Error('invalid result payload');
    await mkdir(resultsDirectory, { recursive: true });
    await writeFile(join(resultsDirectory, `${resultsPrefix}-${filename}.json`), JSON.stringify(result, null, 2));
    response.writeHead(200, { 'content-type': 'application/json' }).end('{"saved":true}');
  } catch (error) {
    response.writeHead(400, { 'content-type': 'text/plain' }).end(String(error));
  }
});
server.listen(5174, '127.0.0.1');
