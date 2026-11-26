// Dedicated Web Worker bridge for `playground` Wasm RaTeX math & Hayro PDF previews.
export function installPlaygroundPreviewBridge(wasmExports, baseUrl = window.location.href) {
  let mathWorker = null;
  let pdfWorker = null;
  let nextId = 1;
  const pendingMath = new Map();
  const pendingPdf = new Map();

  function spawnWorker(scriptRel, wasmRel) {
    const scriptUrl = new URL(scriptRel, baseUrl).toString();
    const wasmUrl = new URL(wasmRel, baseUrl).toString();
    const src = [
      `globalThis.AFFINE_WORKER_WASM_URL = ${JSON.stringify(wasmUrl)};`,
      'const __q = [];',
      'const __onMsg = e => __q.push(e);',
      "globalThis.addEventListener('message', __onMsg);",
      `await import(${JSON.stringify(scriptUrl)});`,
      "globalThis.removeEventListener('message', __onMsg);",
      "for (const e of __q) globalThis.dispatchEvent(new MessageEvent('message', { data: e.data, ports: e.ports }));",
    ].join('\n');
    const blobUrl = URL.createObjectURL(new Blob([src], { type: 'text/javascript' }));
    return new Worker(blobUrl, { type: 'module' });
  }

  function resolveWorkerAsset(key, fallbackRel) {
    const assets = globalThis.window?.__AFFINE_WORKER_ASSETS__;
    const override = assets && typeof assets[key] === 'string' && assets[key].trim();
    return override || fallbackRel;
  }

  function ensureMathWorker() {
    if (!mathWorker) {
      mathWorker = spawnWorker(
        resolveWorkerAsset('mathScript', './workers/math-worker.js'),
        resolveWorkerAsset('mathWasm', './workers/math-worker.wasm')
      );
      mathWorker.addEventListener('message', async event => {
        const msg = event?.data;
        if (!msg || !pendingMath.has(msg.id)) return;
        const req = pendingMath.get(msg.id);
        pendingMath.delete(msg.id);
        if (!msg.ok || !msg.result?.svg) {
          wasmExports.deliver_math_error(
            req.latex,
            req.display,
            req.fontSizePt,
            req.colorCss,
            req.dpr
          );
          return;
        }
        try {
          const coloredSvg = msg.result.svg
            .replaceAll('fill="#000000"', `fill="${req.colorCss}"`)
            .replaceAll('stroke="#000000"', `stroke="${req.colorCss}"`);
          const wMatch = coloredSvg.match(/width="([0-9.]+)"/);
          const hMatch = coloredSvg.match(/height="([0-9.]+)"/);
          const widthPt = wMatch ? Number(wMatch[1]) : 48;
          const heightPt = hMatch ? Number(hMatch[1]) : 24;
          const widthPx = Math.max(1, Math.ceil(widthPt * req.dpr));
          const heightPx = Math.max(1, Math.ceil(heightPt * req.dpr));
          const blob = new Blob([coloredSvg], { type: 'image/svg+xml;charset=utf-8' });
          const url = URL.createObjectURL(blob);
          try {
            const img = new Image();
            await new Promise((resolve, reject) => {
              img.onload = resolve;
              img.onerror = reject;
              img.src = url;
            });
            const canvas = document.createElement('canvas');
            canvas.width = widthPx;
            canvas.height = heightPx;
            const ctx = canvas.getContext('2d');
            ctx.drawImage(img, 0, 0, widthPx, heightPx);
            const rgba = ctx.getImageData(0, 0, widthPx, heightPx).data;
            wasmExports.deliver_math_preview(
              req.latex,
              req.display,
              req.fontSizePt,
              req.colorCss,
              req.dpr,
              widthPt,
              heightPt * 0.78,
              heightPt * 0.22,
              widthPx,
              heightPx,
              rgba
            );
          } finally {
            URL.revokeObjectURL(url);
          }
        } catch {
          wasmExports.deliver_math_error(
            req.latex,
            req.display,
            req.fontSizePt,
            req.colorCss,
            req.dpr
          );
        }
      });
    }
    return mathWorker;
  }

  function ensurePdfWorker() {
    if (!pdfWorker) {
      pdfWorker = spawnWorker(
        resolveWorkerAsset('pdfScript', './workers/pdf-worker.js'),
        resolveWorkerAsset('pdfWasm', './workers/pdf-worker.wasm')
      );
      pdfWorker.addEventListener('message', event => {
        const msg = event?.data;
        if (!msg || !pendingPdf.has(msg.id)) return;
        const req = pendingPdf.get(msg.id);
        pendingPdf.delete(msg.id);
        if (!msg.ok || !msg.result) return;
        if (req.op === 'page_count') {
          wasmExports.deliver_pdf_page_count(req.pdfHash, msg.result.pageCount ?? 0);
        } else if (msg.result.rgba) {
          wasmExports.deliver_pdf_page(
            req.pdfHash,
            req.pageIndex,
            msg.result.pageCount ?? 1,
            req.scale,
            msg.result.widthPx,
            msg.result.heightPx,
            msg.result.rgba
          );
        }
      });
    }
    return pdfWorker;
  }

  window.__playgroundPreview = {
    requestMath(latex, display, fontSizePt, colorCss, dpr) {
      const id = nextId++;
      pendingMath.set(id, { latex, display, fontSizePt, colorCss, dpr });
      ensureMathWorker().postMessage({ id, latex });
    },
    requestPdfPageCount(pdfHash, bytes) {
      const id = nextId++;
      pendingPdf.set(id, { op: 'page_count', pdfHash });
      ensurePdfWorker().postMessage({ id, op: 'page_count', pdfBytes: new Uint8Array(bytes) });
    },
    requestPdfPage(pdfHash, bytes, pageIndex, scale) {
      const id = nextId++;
      pendingPdf.set(id, { op: 'render_page', pdfHash, pageIndex, scale });
      ensurePdfWorker().postMessage({
        id,
        pdfBytes: new Uint8Array(bytes),
        pageIndex,
        scale,
      });
    },
  };
}
