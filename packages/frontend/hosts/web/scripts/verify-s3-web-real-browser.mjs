#!/usr/bin/env node
// @ts-check

/**
 * Real-browser Headless Chromium E2E gate for S3-Web thin host capabilities.
 *
 * Uses a preinstalled Chromium binary (never runs `playwright install`) and
 * always stops the browser process by its exact PID inside `finally`, cleaning
 * up its temporary profile directory.
 *
 * Verifies in real Chromium (`390x844` mobile viewport):
 *   1. `WEBGL_lose_context` recovery: `webglcontextlost` calls `preventDefault()`,
 *      pauses input, freezes the frame loop; `webglcontextrestored` rebuilds
 *      and redraws without page reload.
 *   2. Unrecoverable GPU loss with failing IDB flush aborts `?shell=legacy`
 *      navigation, preserves the pending CRDT write buffer, exposes a retryable
 *      state, and flushes cleanly into real IndexedDB on retry.
 *   3. Canvas-drawn text selection overlay (highlight, outward teardrop handles,
 *      `Copy | Cut | Paste | Select All` menu) with zero floating DOM nodes and
 *      synchronous gesture-stack clipboard actions (including Cut preserving
 *      text when clipboard write fails).
 *   4. Lazy `hayro` `pdf-worker.wasm` loading (0 workers / 0 `.wasm` bytes
 *      before PDF open), multi-page PDF rasterization onto `#app-canvas`,
 *      in-flight deduplication, LRU cache hit, and clean `dispose()`.
 *
 * Pass `--out-dir <dir>` to capture `web-selection-menu.png` and
 * `web-pdf-worker-preview.png`.
 */

import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import {
  closeSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { extname, join, normalize, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const WEB_ROOT = resolve(fileURLToPath(new URL('..', import.meta.url)));

const MIME_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.ttf': 'font/ttf',
};

function parseOutDir() {
  const idx = process.argv.indexOf('--out-dir');
  if (idx >= 0 && process.argv[idx + 1]) {
    return resolve(process.argv[idx + 1]);
  }
  return null;
}

function isSnapWrapper(bin) {
  if (!bin.startsWith('/usr/bin/chromium')) {
    return false;
  }
  try {
    const fd = openSync(bin, 'r');
    try {
      const buf = Buffer.alloc(256);
      const n = readSync(fd, buf, 0, buf.length, 0);
      const head = buf.subarray(0, n).toString('utf8');
      return head.startsWith('#!') && /snap/i.test(head);
    } finally {
      closeSync(fd);
    }
  } catch {
    return false;
  }
}

function findChromiumBinary() {
  const candidates = [
    process.env.CHROME_PATH,
    process.env.CHROME_BIN,
    '/usr/bin/google-chrome-stable',
    '/usr/bin/google-chrome',
    '/usr/bin/chromium',
    '/usr/bin/chromium-browser',
  ].filter(Boolean);
  let snapFallback = null;
  for (const bin of candidates) {
    if (!bin || !existsSync(bin)) {
      continue;
    }
    if (isSnapWrapper(bin)) {
      snapFallback ??= bin;
      continue;
    }
    return bin;
  }
  const pwDir = join(process.env.HOME || '', '.cache/ms-playwright');
  if (existsSync(pwDir)) {
    const found = execFileSync(
      'find',
      [pwDir, '-name', 'chrome', '-type', 'f'],
      { encoding: 'utf8' }
    )
      .split('\n')
      .map(line => line.trim())
      .find(Boolean);
    if (found) {
      return found;
    }
  }
  if (snapFallback) {
    return snapFallback;
  }
  throw new Error('No preinstalled Chromium binary found on this machine');
}

/**
 * Finds the staged `pdf-worker.<hash>.wasm` file under `hosts/web/assets/`.
 *
 * @returns {{ fileName: string, buildHash: string }}
 */
function resolveStagedPdfWasm() {
  const assetsDir = join(WEB_ROOT, 'assets');
  for (const hash of ['dev-local', 'build-s3']) {
    const candidate = `pdf-worker.${hash}.wasm`;
    if (existsSync(join(assetsDir, candidate))) {
      return { fileName: candidate, buildHash: hash };
    }
  }
  throw new Error(
    `Missing staged pdf-worker wasm in ${assetsDir}. Run stage-preview-workers.mjs first.`
  );
}

const stagedPdf = resolveStagedPdfWasm();

const PROBE_HTML = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />
    <title>AFFiNE S3-Web Verification</title>
    <style>
      html, body {
        margin: 0;
        padding: 0;
        width: 100%;
        height: 100%;
        overflow: hidden;
        background: #f8fafc;
      }
      #app-canvas {
        position: fixed;
        inset: 0;
        width: 100%;
        height: 100%;
        display: block;
        touch-action: none;
      }
      #ime {
        position: fixed;
        top: 0;
        left: 0;
        width: 1px;
        height: 1px;
        opacity: 0;
        border: 0;
        font-size: 16px;
        line-height: 16px;
        background: transparent;
        color: transparent;
        caret-color: transparent;
      }
      #safe-area-probe {
        position: fixed;
        width: 0;
        height: 0;
        visibility: hidden;
        pointer-events: none;
      }
      #embed-layer {
        position: fixed;
        inset: 0;
        pointer-events: none;
      }
      #a11y-tree {
        position: fixed;
        width: 1px;
        height: 1px;
        overflow: hidden;
        clip-path: inset(50%);
      }
    </style>
  </head>
  <body>
    <canvas id="app-canvas" aria-hidden="true"></canvas>
    <textarea id="ime" autocomplete="off" spellcheck="false" aria-label="Document input"></textarea>
    <div id="safe-area-probe" aria-hidden="true"></div>
    <div id="embed-layer"></div>
    <div id="a11y-tree" role="application" aria-label="AFFiNE workspace"></div>
    <script type="module">
      import {
        createGpuRecoveryController,
        focusImeInGesture,
      } from '/src/host.js';
      import {
        createIdbWorkspaceStorage,
        createPendingWriteFlusher,
      } from '/src/idb-storage.js';
      import { createPdfPreviewClient } from '/src/pdf-client.js';
      import {
        attachImeSelectionGuard,
        computeSelectionOverlayLayout,
        createSelectionOverlayController,
        drawSelectionOverlay,
        executeClipboardAction,
      } from '/src/selection-overlay.js';
      import { createWorkerLoader } from '/src/worker-loader.js';

      function buildMultiPagePdf() {
        function buildPageStream(pageNum) {
          if (pageNum === 1) {
            return [
              '0.98 0.99 1.00 rg 0 0 360 480 re f',
              '0.12 0.35 0.85 rg 0 416 360 64 re f',
              'BT /F1 18 Tf 1 1 1 rg 24 442 Td (AFFiNE Mobile Architecture) Tj ET',
              'BT /F1 11 Tf 0.85 0.92 1.00 rg 24 424 Td (Pure Safe Rust hayro PDF Worker - Page 1 of 2) Tj ET',
              '0.92 0.95 1.00 rg 24 270 312 120 re f',
              '0.12 0.35 0.85 RG 1.2 w 24 270 312 120 re S',
              'BT /F1 13 Tf 0.10 0.14 0.22 rg 38 366 Td (Quarterly Frame Time Budget) Tj ET',
              '0.15 0.50 0.95 rg 46 284 42 52 re f',
              '0.18 0.68 0.56 rg 108 284 42 64 re f',
              '0.94 0.58 0.16 rg 170 284 42 44 re f',
              '0.44 0.36 0.90 rg 232 284 42 58 re f',
              'BT /F1 11 Tf 0.18 0.22 0.30 rg 24 236 Td (1. Zero third-party runtime JavaScript in Web host.) Tj ET',
              'BT /F1 11 Tf 0.18 0.22 0.30 rg 24 214 Td (2. Lazy pdf-worker.wasm spawns only on PDF open.) Tj ET',
              'BT /F1 11 Tf 0.18 0.22 0.30 rg 24 192 Td (3. Transferable RGBA frames with LRU byte cap.) Tj ET',
              '0.86 0.89 0.94 RG 0.8 w 24 156 m 336 156 l S',
              'BT /F1 10 Tf 0.38 0.44 0.54 rg 24 132 Td (Rendered off the main thread by hayro in WebAssembly.) Tj ET',
              '0.12 0.35 0.85 rg 24 72 96 28 re f',
              'BT /F1 10 Tf 1 1 1 rg 40 82 Td (VERIFIED) Tj ET',
            ].join('\\n') + '\\n';
          }
          return [
            '0.99 0.99 1.00 rg 0 0 360 480 re f',
            '0.15 0.55 0.45 rg 0 416 360 64 re f',
            'BT /F1 18 Tf 1 1 1 rg 24 442 Td (Appendix - Page 2 of 2) Tj ET',
            '0.15 0.55 0.45 rg 40 240 280 120 re f',
          ].join('\\n') + '\\n';
        }

        const s1 = buildPageStream(1);
        const s2 = buildPageStream(2);
        const objects = [
          '1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\\n',
          '2 0 obj << /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >> endobj\\n',
          '3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 360 480] /Resources << /Font << /F1 7 0 R >> >> /Contents 4 0 R >> endobj\\n',
          '4 0 obj << /Length ' + s1.length + ' >> stream\\n' + s1 + 'endstream endobj\\n',
          '5 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 360 480] /Resources << /Font << /F1 7 0 R >> >> /Contents 6 0 R >> endobj\\n',
          '6 0 obj << /Length ' + s2.length + ' >> stream\\n' + s2 + 'endstream endobj\\n',
          '7 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\\n',
        ];
        let out = '%PDF-1.4\\n';
        const offsets = [0];
        for (const obj of objects) {
          offsets.push(out.length);
          out += obj;
        }
        const xrefPos = out.length;
        out += 'xref\\n0 ' + (objects.length + 1) + '\\n';
        out += '0000000000 65535 f \\n';
        for (let i = 1; i < offsets.length; i += 1) {
          out += String(offsets[i]).padStart(10, '0') + ' 00000 n \\n';
        }
        out += 'trailer << /Size ' + (objects.length + 1) + ' /Root 1 0 R >>\\n';
        out += 'startxref\\n' + xrefPos + '\\n%%EOF\\n';
        return new TextEncoder().encode(out);
      }

      const canvas = document.getElementById('app-canvas');
      const ime = document.getElementById('ime');
      const dpr = window.devicePixelRatio || 2;
      const vw = 390;
      const vh = 844;
      canvas.width = vw * dpr;
      canvas.height = vh * dpr;

      window.__S3_API__ = {
        async runWebGlAndIdbChecks() {
          // 1. Real WEBGL_lose_context recovery on a dedicated WebGL2 canvas.
          const glCanvas = document.createElement('canvas');
          glCanvas.width = 64;
          glCanvas.height = 64;
          const gl = glCanvas.getContext('webgl2');
          const ext = gl?.getExtension('WEBGL_lose_context');
          let paused = false;
          let frozen = false;
          let restoredDraws = 0;

          const ctrl = createGpuRecoveryController({
            getHref: () => window.location.href,
            isOAuthCodeConsumed: () => false,
            getActiveCanvas: () => glCanvas,
            pauseInput: () => { paused = true; },
            resumeInput: () => { paused = false; },
            freezeFrameLoop: () => { frozen = true; },
            unfreezeFrameLoop: () => { frozen = false; },
            onContextRestored: c => {
              const ctx = c?.getContext('webgl2');
              if (ctx) {
                ctx.clearColor(0.1, 0.2, 0.3, 1);
                ctx.clear(ctx.COLOR_BUFFER_BIT);
                restoredDraws += 1;
                return true;
              }
              return false;
            },
            flushPendingWrites: async () => ({ ok: true, pendingCount: 0 }),
            navigate: () => {},
          });

          let lostDefaultPrevented = false;
          let restoredPromise = new Promise(resolve => {
            glCanvas.addEventListener('webglcontextrestored', async () => {
              const r = await ctrl.handleWebGlContextRestored();
              resolve(r);
            });
          });
          let lostPromise = new Promise(resolve => {
            glCanvas.addEventListener('webglcontextlost', ev => {
              ctrl.handleWebGlContextLost(ev);
              lostDefaultPrevented = ev.defaultPrevented;
              resolve({ paused, frozen, status: ctrl.getState().status });
            });
          });

          let webglTested = false;
          let lostSnapshot = null;
          let restoreSnapshot = null;
          const withTimeout = (promise, ms, label) =>
            Promise.race([
              promise,
              new Promise((_, reject) =>
                setTimeout(() => reject(new Error('Timeout waiting for ' + label)), ms)
              ),
            ]);
          if (gl && ext) {
            webglTested = true;
            ext.loseContext();
            lostSnapshot = await withTimeout(lostPromise, 5000, 'webglcontextlost');
            // Wait one macrotask so Chromium finishes processing ev.defaultPrevented
            // from the webglcontextlost dispatch before restoreContext() runs.
            await new Promise(r => setTimeout(r, 0));
            ext.restoreContext();
            restoreSnapshot = await withTimeout(restoredPromise, 5000, 'webglcontextrestored');
          }

          // 2. Real IndexedDB flush circuit breaker: fail blocks navigation, retry persists and navigates.
          const storage = createIdbWorkspaceStorage({
            flavour: 'local',
            type: 'workspace',
            id: 's3-web-gpu-fallback',
          });
          const flusher = createPendingWriteFlusher(storage);
          flusher.enqueue({
            docId: 'doc-unsaved',
            bin: new TextEncoder().encode('crdt-delta-s3'),
          });
          let allowFlush = false;
          const navUrls = [];
          const fallbackCtrl = createGpuRecoveryController({
            getHref: () => 'https://app.affine.pro/workspace/ws-1/doc-unsaved?code=once&tab=notes',
            isOAuthCodeConsumed: () => true,
            pauseInput: () => { ime.readOnly = true; },
            resumeInput: () => { ime.readOnly = false; },
            flushPendingWrites: async () => {
              if (!allowFlush) {
                throw new Error('simulated-idb-quota-error');
              }
              return flusher.flush();
            },
            navigate: url => navUrls.push(url),
          });

          const blocked = await fallbackCtrl.triggerFallback('context-restore-timeout');
          const pendingWhileBlocked = flusher.pendingCount();
          allowFlush = true;
          const retried = await fallbackCtrl.retryFallback();
          const persisted = await storage.getDocRaw('doc-unsaved');
          storage.close();
          await new Promise(resolve => {
            const req = indexedDB.deleteDatabase(storage.dbName);
            req.onsuccess = resolve;
            req.onerror = resolve;
          });

          return {
            webglTested,
            lostDefaultPrevented,
            lostSnapshot,
            restoreSnapshot,
            restoredDraws,
            pausedAfterRestore: paused,
            frozenAfterRestore: frozen,
            blocked,
            pendingWhileBlocked,
            navCountWhileBlocked: blocked.navigated ? 1 : 0,
            retried,
            navUrls,
            persistedUpdates: persisted?.updates.length ?? 0,
          };
        },

        async renderSelectionSceneAndTestClipboard() {
          const ctx = canvas.getContext('2d');
          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);

          // Draw clean mobile document surface on #app-canvas.
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(0, 0, vw, vh);

          // Top status & header bar.
          ctx.fillStyle = '#f8fafc';
          ctx.fillRect(0, 0, vw, 96);
          ctx.fillStyle = '#e2e8f0';
          ctx.fillRect(0, 95, vw, 1);

          ctx.fillStyle = '#0f172a';
          ctx.font = '600 14px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('9:41', 22, 30);
          ctx.font = '700 17px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('AFFiNE Mobile Spec', 22, 74);

          ctx.fillStyle = '#1e6efb';
          ctx.font = '600 13px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('Canvas Host', 292, 74);

          // Document body paragraphs.
          ctx.fillStyle = '#0f172a';
          ctx.font = '700 22px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('Pure Canvas Selection', 24, 142);

          const lines = [
            'The thin web host draws every selection',
            'highlight, teardrop handle, and action bar',
            'directly on #app-canvas with zero floating',
            'DOM menus or third-party JavaScript.',
          ];
          ctx.font = '400 16px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillStyle = '#1e293b';
          const lineY = [232, 262, 292, 322];
          lines.forEach((line, idx) => {
            ctx.fillText(line, 24, lineY[idx]);
          });

          const selRects = [
            { x: 24, y: 214, width: 312, height: 26 },
            { x: 24, y: 244, width: 326, height: 26 },
          ];

          const overlayInput = {
            viewport: { width: vw, height: vh, safeArea: { top: 44, bottom: 34 } },
            rects: selRects,
            caretStart: { x: 24, y: 214, height: 26, dir: 'ltr' },
            caretEnd: { x: 350, y: 244, height: 26, dir: 'ltr' },
          };

          let docBody = 'The thin web host draws every selection highlight, teardrop handle';
          let virtualClipboard = '';
          let failNextClipboardWrite = false;
          const clipboardMock = {
            async writeText(text) {
              if (failNextClipboardWrite) {
                throw new Error('ClipboardDenied');
              }
              virtualClipboard = text;
            },
            async readText() {
              return virtualClipboard;
            },
          };

          const actionResults = [];
          const controller = createSelectionOverlayController({
            getInput: () => overlayInput,
            onAction: async action => {
              const res = await executeClipboardAction({
                action,
                getSelectedText: () => 'highlight, teardrop handle',
                deleteSelection: () => {
                  docBody = docBody.replace(' highlight, teardrop handle', '');
                },
                insertText: text => {
                  docBody += ' ' + text;
                },
                selectAll: () => {},
                clipboard: clipboardMock,
                isSecureContext: true,
                imeEl: ime,
                doc: document,
              });
              actionResults.push(res);
              return res;
            },
          });

          const layout = controller.getLayout();
          drawSelectionOverlay(ctx, layout, { theme: 'light' });

          // Footer note on canvas.
          ctx.fillStyle = '#64748b';
          ctx.font = '500 12px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('DOM check: 0 floating menu elements mounted', 24, 412);

          // Focus #ime synchronously and verify guard.
          const imeFocused = focusImeInGesture(ime, { inputMode: 'text' });
          const guard = attachImeSelectionGuard(ime, () => true);

          // Test Copy, failing Cut (preserves text), succeeding Cut, and Paste via controller hit-test.
          const clickItem = async id => {
            const item = layout.menu.items.find(i => i.id === id);
            const r = controller.onPointerDown(item.x + item.width / 2, item.y + item.height / 2);
            await r.actionPromise;
            controller.onPointerUp();
          };

          await clickItem('copy');
          const afterCopyClip = virtualClipboard;

          failNextClipboardWrite = true;
          const beforeFailedCut = docBody;
          await clickItem('cut');
          const preservedAfterFailedCut = docBody === beforeFailedCut;

          failNextClipboardWrite = false;
          await clickItem('cut');
          const deletedAfterOkCut = !docBody.includes('highlight, teardrop handle');

          await clickItem('paste');
          const pastedBack = docBody.includes('highlight, teardrop handle');
          guard.dispose();

          const bodyChildIds = [...document.body.children]
            .filter(el => el.tagName !== 'SCRIPT')
            .map(el => el.tagName.toLowerCase() + '#' + el.id);

          return {
            imeFocused,
            imeFontSize: getComputedStyle(ime).fontSize,
            bodyChildIds,
            embedLayerChildren: document.getElementById('embed-layer').children.length,
            menuX: layout.menu.x,
            menuRightMargin: vw - (layout.menu.x + layout.menu.width),
            afterCopyClip,
            preservedAfterFailedCut,
            deletedAfterOkCut,
            pastedBack,
            actionCount: actionResults.length,
          };
        },

        async renderPdfPreviewScene() {
          const manifest = {
            shellVersion: 'mobile-v1',
            buildHash: ${JSON.stringify(stagedPdf.buildHash)},
            buildId: ${JSON.stringify(stagedPdf.buildHash)},
            assetBaseUrl: window.location.origin + '/',
            workers: {
              mermaid: { script: '/src/workers/mermaid-worker.js', wasm: '/assets/mermaid-worker.${stagedPdf.buildHash}.wasm' },
              math: { script: '/src/workers/math-worker.js', wasm: '/assets/math-worker.${stagedPdf.buildHash}.wasm' },
              typst: { script: '/src/workers/typst-worker.js', wasm: '/assets/typst-worker.${stagedPdf.buildHash}.wasm' },
              pdf: { script: '/src/workers/pdf-worker.js', wasm: '/assets/${stagedPdf.fileName}' },
            },
            fonts: {},
          };
          const loader = createWorkerLoader(manifest, { baseUrl: window.location.href });
          const pdfClient = createPdfPreviewClient({ workerLoader: loader, maxCacheBytes: 8 * 1024 * 1024 });

          const spawnedBeforePdf = loader.hasLoadedWorker('pdf');
          const pdfBytes = buildMultiPagePdf();
          const pageCount = await pdfClient.getPageCount({ docKey: 'spec-pdf', pdfBytes });

          // Trigger deduplicated concurrent render of page 0 + render of page 1.
          const [p1a, p1b, p2] = await Promise.all([
            pdfClient.renderPage({ docKey: 'spec-pdf', pdfBytes, pageIndex: 0, scale: 1 }),
            pdfClient.renderPage({ docKey: 'spec-pdf', pdfBytes, pageIndex: 0, scale: 1 }),
            pdfClient.renderPage({ docKey: 'spec-pdf', pdfBytes, pageIndex: 1, scale: 1 }),
          ]);
          const p1Cached = await pdfClient.renderPage({ docKey: 'spec-pdf', pdfBytes, pageIndex: 0, scale: 1 });

          // Draw mobile document shell + in-canvas PDF preview card on #app-canvas.
          const ctx = canvas.getContext('2d');
          ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
          ctx.fillStyle = '#f1f5f9';
          ctx.fillRect(0, 0, vw, vh);

          // Top bar.
          ctx.fillStyle = '#ffffff';
          ctx.fillRect(0, 0, vw, 92);
          ctx.fillStyle = '#e2e8f0';
          ctx.fillRect(0, 91, vw, 1);
          ctx.fillStyle = '#0f172a';
          ctx.font = '600 14px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('9:41', 22, 30);
          ctx.font = '700 17px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('Quarterly Report.pdf', 22, 72);
          ctx.fillStyle = '#1e6efb';
          ctx.font = '600 12px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('hayro Wasm', 298, 72);

          // Card frame.
          const cardX = 16;
          const cardY = 112;
          const cardW = vw - 32;
          const cardH = 540;
          ctx.save();
          ctx.shadowColor = 'rgba(15, 23, 42, 0.12)';
          ctx.shadowBlur = 18;
          ctx.shadowOffsetY = 6;
          ctx.fillStyle = '#ffffff';
          ctx.beginPath();
          ctx.roundRect(cardX, cardY, cardW, cardH, 14);
          ctx.fill();
          ctx.restore();

          // Blit hayro RGBA bitmap into an offscreen canvas and draw crisply inside the card.
          const off = document.createElement('canvas');
          off.width = p1a.widthPx;
          off.height = p1a.heightPx;
          const offCtx = off.getContext('2d');
          const imgData = new ImageData(new Uint8ClampedArray(p1a.rgba), p1a.widthPx, p1a.heightPx);
          offCtx.putImageData(imgData, 0, 0);

          const pad = 12;
          const drawW = cardW - pad * 2;
          const drawH = Math.min(cardH - 64, (drawW * p1a.heightPx) / p1a.widthPx);
          ctx.drawImage(off, cardX + pad, cardY + pad, drawW, drawH);

          // Card footer with pagination indicator.
          const footerY = cardY + cardH - 42;
          ctx.fillStyle = '#e2e8f0';
          ctx.fillRect(cardX + 12, footerY - 10, cardW - 24, 1);
          ctx.fillStyle = '#0f172a';
          ctx.font = '600 13px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText('Page 1 / ' + pageCount, cardX + 20, footerY + 14);
          ctx.fillStyle = '#475569';
          ctx.font = '500 12px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.fillText(p1a.widthPx + 'x' + p1a.heightPx + ' RGBA • Cached', cardX + 190, footerY + 14);

          // Bottom summary pill.
          ctx.fillStyle = '#1e293b';
          ctx.beginPath();
          ctx.roundRect(24, 680, vw - 48, 44, 10);
          ctx.fill();
          ctx.fillStyle = '#f8fafc';
          ctx.font = '500 12px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif';
          ctx.textAlign = 'center';
          ctx.fillText('Lazy hayro pdf-worker.wasm · 0 PDFium · 0 DOM overlays', vw / 2, 706);
          ctx.textAlign = 'left';

          const statsBeforeDispose = pdfClient.getStats();
          pdfClient.dispose();
          const statsAfterDispose = pdfClient.getStats();

          return {
            spawnedBeforePdf,
            pageCount,
            p1Width: p1a.widthPx,
            p1Height: p1a.heightPx,
            p1ByteLength: p1a.rgba.byteLength,
            p1DedupMatch: p1a.rgba.byteLength === p1b.rgba.byteLength,
            p2Width: p2.widthPx,
            p2Height: p2.heightPx,
            p1FromCacheOnSecondCall: p1Cached.fromCache,
            statsBeforeDispose,
            statsAfterDispose,
          };
        },
      };

      window.__S3_READY__ = true;
    </script>
  </body>
</html>`;

const outDir = parseOutDir();
if (outDir) {
  mkdirSync(outDir, { recursive: true });
}

let wasmRequestCount = 0;

const server = createServer((req, res) => {
  const url = new URL(req.url || '/', 'http://127.0.0.1');
  if (url.pathname === '/s3-probe.html') {
    res.writeHead(200, { 'content-type': MIME_TYPES['.html'] });
    res.end(PROBE_HTML);
    return;
  }
  if (url.pathname.includes('pdf-worker') && url.pathname.endsWith('.wasm')) {
    wasmRequestCount += 1;
  }
  const rel = normalize(url.pathname).replace(/^\/+/, '');
  const full = resolve(WEB_ROOT, rel);
  if (!full.startsWith(WEB_ROOT) || !existsSync(full)) {
    res.writeHead(404);
    res.end('not found');
    return;
  }
  res.writeHead(200, {
    'content-type': MIME_TYPES[extname(full)] || 'application/octet-stream',
  });
  res.end(readFileSync(full));
});

const chromeBin = findChromiumBinary();
const profileDir = mkdtempSync(join(tmpdir(), 'affine-s3-web-e2e-'));
await new Promise(resolveListen =>
  server.listen(0, '127.0.0.1', resolveListen)
);
const address = server.address();
const port = typeof address === 'object' && address ? address.port : 0;

const chromeProc = spawn(
  chromeBin,
  [
    '--headless=new',
    '--no-sandbox',
    '--enable-unsafe-swiftshader',
    '--disable-dev-shm-usage',
    '--no-first-run',
    '--no-default-browser-check',
    '--remote-debugging-port=0',
    `--user-data-dir=${profileDir}`,
    'about:blank',
  ],
  { stdio: ['ignore', 'ignore', 'pipe'] }
);

let ws = null;
try {
  const wsUrl = await new Promise((resolveWs, rejectWs) => {
    let stderrBuf = '';
    const timer = setTimeout(
      () => rejectWs(new Error(`Chromium DevTools timeout: ${stderrBuf}`)),
      15000
    );
    chromeProc.stderr.on('data', chunk => {
      stderrBuf += String(chunk);
      const m = stderrBuf.match(/DevTools listening on (ws:\/\/\S+)/);
      if (m) {
        clearTimeout(timer);
        resolveWs(m[1]);
      }
    });
    chromeProc.once('exit', code => {
      clearTimeout(timer);
      rejectWs(new Error(`Chromium exited early (${code}): ${stderrBuf}`));
    });
  });

  const debugPort = new URL(wsUrl).port;
  const targets = await (
    await fetch(`http://127.0.0.1:${debugPort}/json/list`)
  ).json();
  const pageTarget = targets.find(t => t.type === 'page') || targets[0];
  ws = new WebSocket(pageTarget.webSocketDebuggerUrl);
  await new Promise((ok, fail) => {
    ws.addEventListener('open', ok, { once: true });
    ws.addEventListener('error', fail, { once: true });
  });

  let nextId = 1;
  const pending = new Map();
  ws.addEventListener('message', ev => {
    const msg = JSON.parse(String(ev.data));
    if (msg.id && pending.has(msg.id)) {
      const { ok, fail } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) fail(new Error(msg.error.message));
      else ok(msg.result);
    }
  });
  const send = (method, params = {}) =>
    new Promise((ok, fail) => {
      const id = nextId++;
      const timer = setTimeout(() => {
        pending.delete(id);
        fail(new Error(`CDP timeout after 25s for ${method}`));
      }, 25000);
      pending.set(id, {
        ok: val => {
          clearTimeout(timer);
          ok(val);
        },
        fail: err => {
          clearTimeout(timer);
          fail(err);
        },
      });
      ws.send(JSON.stringify({ id, method, params }));
    });
  const evalJs = async expression => {
    const r = await send('Runtime.evaluate', {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    if (r.exceptionDetails) {
      throw new Error(
        r.exceptionDetails.exception?.description || r.exceptionDetails.text
      );
    }
    return r.result?.value;
  };

  await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', {
    width: 390,
    height: 844,
    deviceScaleFactor: 2,
    mobile: true,
  });
  await send('Page.navigate', {
    url: `http://127.0.0.1:${port}/s3-probe.html`,
  });

  let ready = false;
  for (let i = 0; i < 100; i += 1) {
    ready = Boolean(await evalJs('Boolean(window.__S3_READY__)'));
    if (ready) break;
    await new Promise(r => setTimeout(r, 50));
  }
  assert.equal(ready, true, 'S3-Web probe page did not initialize in time');

  // 1. WebGL2 context loss/restore + IDB circuit breaker.
  const gpuIdb = await evalJs('window.__S3_API__.runWebGlAndIdbChecks()');
  assert.equal(
    gpuIdb.webglTested,
    true,
    'WebGL2 + WEBGL_lose_context must run'
  );
  assert.equal(gpuIdb.lostDefaultPrevented, true);
  assert.equal(gpuIdb.lostSnapshot.paused, true);
  assert.equal(gpuIdb.lostSnapshot.frozen, true);
  assert.equal(gpuIdb.restoreSnapshot.recovered, true);
  assert.equal(gpuIdb.restoredDraws, 1);
  assert.equal(gpuIdb.pausedAfterRestore, false);
  assert.equal(gpuIdb.frozenAfterRestore, false);
  assert.equal(gpuIdb.blocked.navigated, false);
  assert.equal(gpuIdb.blocked.retryable, true);
  assert.equal(gpuIdb.pendingWhileBlocked, 1);
  assert.equal(gpuIdb.retried.navigated, true);
  assert.equal(gpuIdb.persistedUpdates, 1);
  assert.equal(gpuIdb.navUrls.length, 1);
  assert.equal(new URL(gpuIdb.navUrls[0]).searchParams.get('shell'), 'legacy');
  assert.equal(new URL(gpuIdb.navUrls[0]).searchParams.has('code'), false);

  // Zero pdf-worker.wasm requests before PDF is opened!
  assert.equal(
    wasmRequestCount,
    0,
    'pdf-worker.wasm must not be fetched before a PDF is opened'
  );

  // 2. Canvas selection overlay + clipboard + screenshot 1.
  const selection = await evalJs(
    'window.__S3_API__.renderSelectionSceneAndTestClipboard()'
  );
  assert.equal(selection.imeFocused, true);
  assert.equal(selection.imeFontSize, '16px');
  assert.deepEqual(selection.bodyChildIds, [
    'canvas#app-canvas',
    'textarea#ime',
    'div#safe-area-probe',
    'div#embed-layer',
    'div#a11y-tree',
  ]);
  assert.equal(selection.embedLayerChildren, 0);
  assert.ok(selection.menuX >= 12);
  assert.ok(selection.menuRightMargin >= 12);
  assert.equal(selection.afterCopyClip, 'highlight, teardrop handle');
  assert.equal(selection.preservedAfterFailedCut, true);
  assert.equal(selection.deletedAfterOkCut, true);
  assert.equal(selection.pastedBack, true);

  if (outDir) {
    const shot1 = await send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(
      join(outDir, 'web-selection-menu.png'),
      Buffer.from(shot1.data, 'base64')
    );
  }

  // Still zero pdf-worker.wasm requests before PDF scene runs.
  assert.equal(wasmRequestCount, 0);

  // 3. Lazy hayro pdf-worker.wasm multi-page render + screenshot 2.
  const pdf = await evalJs('window.__S3_API__.renderPdfPreviewScene()');
  assert.equal(pdf.spawnedBeforePdf, false);
  assert.equal(wasmRequestCount, 1);
  assert.equal(pdf.pageCount, 2);
  assert.ok(pdf.p1Width > 0 && pdf.p1Height > 0);
  assert.equal(pdf.p1ByteLength, pdf.p1Width * pdf.p1Height * 4);
  assert.equal(pdf.p1DedupMatch, true);
  assert.ok(pdf.p2Width > 0 && pdf.p2Height > 0);
  assert.equal(pdf.p1FromCacheOnSecondCall, true);
  assert.ok(pdf.statsBeforeDispose.dedupHits >= 1);
  assert.ok(pdf.statsBeforeDispose.cacheHits >= 1);
  assert.equal(pdf.statsAfterDispose.disposed, true);
  assert.equal(pdf.statsAfterDispose.cacheEntries, 0);

  if (outDir) {
    const shot2 = await send('Page.captureScreenshot', { format: 'png' });
    writeFileSync(
      join(outDir, 'web-pdf-worker-preview.png'),
      Buffer.from(shot2.data, 'base64')
    );
  }

  console.log(
    `Real-browser S3-Web E2E passed (${chromeBin}):`,
    JSON.stringify({
      webglRecovered: gpuIdb.restoreSnapshot.recovered,
      idbBlockedThenRetried:
        !gpuIdb.blocked.navigated && gpuIdb.retried.navigated,
      selectionDomNodes: selection.bodyChildIds.length,
      pdfPages: pdf.pageCount,
      pdfWasmRequests: wasmRequestCount,
      screenshotsDir: outDir,
    })
  );
} finally {
  try {
    ws?.close();
  } catch {}
  try {
    if (chromeProc.pid) {
      process.kill(chromeProc.pid, 'SIGTERM');
    }
  } catch {}
  await new Promise(r => setTimeout(r, 200));
  try {
    if (chromeProc.pid) {
      process.kill(chromeProc.pid, 'SIGKILL');
    }
  } catch {}
  server.close();
  rmSync(profileDir, { recursive: true, force: true });
}
