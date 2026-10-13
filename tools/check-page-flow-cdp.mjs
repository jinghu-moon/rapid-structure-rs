/* Drive the served page in a real browser (headless Chrome over CDP) and **assert** that one
 * upload reaches a rendered region list. This is the check that catches the M5 bug class.
 *
 * Why it exists
 * -------------
 * The M5 defect was not a wire-format problem. `/api/jobs/{id}/result` was byte-perfect and the
 * job succeeded server-side; the page threw the accepted job away before polling it, so it never
 * issued a single `GET /api/jobs/{id}`, the scanning animation never stopped, and the region list
 * stayed empty. No amount of field-level contract testing can see that: it is a control-flow bug
 * inside the page. Only executing the page against the real server can.
 *
 * The invariants it asserts (1-4 were all violated by the pre-M5 page):
 *   1. after the upload the page issues at least one `GET /api/jobs/{id}` — i.e. it polls the job
 *      it was given;
 *   2. it reaches `GET /api/jobs/{id}/result` with 200;
 *   3. `.stage` no longer carries `scanning` (the animation is owned and cleared);
 *   4. `#regionList` holds at least one region, and that region carries text.
 *
 * Step 2 added five more, all about the transparent text layer — they can only be measured in a
 * real browser, because "the user can drag across the image and get the recognized characters"
 * is a hit-testing / selection / CSS fact, not a wire fact:
 *   5. with the `chars` tier selected, the layer is visible and carries at least one span whose
 *      `data-kind` is `char`, computed `user-select: text` and a transparent glyph colour;
 *   6. the spans, concatenated in DOM order, are **byte-identical** to `recognition.words[*].text`
 *      of the very `/result` body this run fetched (order included);
 *   7. each span's CSS matrix maps its origin onto its word polygon's p0, and its two column
 *      directions onto p0→p1 and p0→p3 (±1°), with the font size equal to |p0→p3| (±2%): the box
 *      follows the quadrilateral, so a tilted line is **not** replaced by an axis-aligned box;
 *   8. a real mouse drag across the spans produces a non-empty selection that is a contiguous
 *      slice of that line's recognized text. "适应窗口" fits a 3200px-wide sheet into a few hundred
 *      CSS pixels (characters ~2px tall, and a share of boxes collapse under the page's own
 *      sub-pixel guard), so the drag is performed after zooming in with the page's own 放大 button,
 *      and only over a run of consecutive characters whose centre the browser hit-tests back to that
 *      very span — a sub-pixel target cannot prove anything about selection, and failing on one
 *      would be a property of the pointer resolution, not of the layer;
 *   9. that zoom re-lays out the **same** layer: identical span count and identical text, with no
 *      more collapsed boxes than before, the page's own zoom readout changed, and every placed span
 *      still inside the displayed image (the layer shares the overlay's single layout tick).
 *
 * Modes
 * -----
 *   node tools/check-page-flow-cdp.mjs --capture <capture.json>
 *       Assert the invariants against an already-recorded capture (used to prove the check
 *       *bites*: the pre-fix capture must FAIL it).
 *   node tools/check-page-flow-cdp.mjs --base <url> --cdp <port> --image <path> [--out <dir>] [--assert-tilt]
 *       Drive a live server end to end and assert the same invariants. `--assert-tilt` additionally
 *       requires at least one span to be measurably non-axis-aligned — pass it for a tilted test
 *       image, never for a straight one, or the check would assert nothing about tilt.
 */
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';

const argv = process.argv.slice(2);
const arg = (name, fallback = null) => {
  const at = argv.indexOf(`--${name}`);
  return at >= 0 && argv[at + 1] && !argv[at + 1].startsWith('--') ? argv[at + 1] : fallback;
};
const base = arg('base');
const cdpPort = Number(arg('cdp', '0'));
const imagePath = arg('image');
const outDir = arg('out', join('target', 'flow-gate', 'cdp-live'));
const capturePath = arg('capture');
const assertTilt = argv.includes('--assert-tilt');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* Everything the text-layer invariants need, read off the live DOM in one pass. The matrix is taken
 * from the **computed** transform (what the browser actually renders), not from the JS that wrote
 * it, and `text`/`ri`/`k`/`li` are read from the elements — so a layer built from a different
 * response, or in a different order, cannot satisfy the check by accident. */
const LAYER_PROBE = `JSON.stringify((() => {
  const layer = document.querySelector('#textLayer');
  const stage = document.querySelector('.stage');
  const box = stage.getBoundingClientRect();
  const all = [...document.querySelectorAll('#textLayer .tl-word')];
  const lines = [...document.querySelectorAll('#textLayer .tl-line')];
  const rect = (s) => {
    const r = s.getBoundingClientRect();
    /* rect is kept rather than a centre point: a sub-pixel-guarded span reports an **empty** rect at
     * the document origin (display:none), and (0,0) is where the pointer would land if a caller
     * dragged to it — it would then "select" the toolbar instead of the image. */
    return {
      left: r.left,
      top: r.top,
      right: r.right,
      bottom: r.bottom,
      x: r.left + r.width / 2,
      y: r.top + r.height / 2,
      w: r.width,
      h: r.height,
    };
  };
  const geo = (s) => {
    const m = new DOMMatrixReadOnly(getComputedStyle(s).transform);
    return {
      text: s.textContent,
      ri: Number(s.getAttribute('data-ri')),
      kind: s.getAttribute('data-kind'),
      li: lines.indexOf(s.parentElement),
      k: [...s.parentElement.querySelectorAll('.tl-word')].indexOf(s),
      m: [m.a, m.b, m.c, m.d, m.e, m.f],
      font: parseFloat(getComputedStyle(s).fontSize),
      box: rect(s),
      /* the page's own guard hides a box that degenerates below half a display pixel */
      gone: s.style.display === 'none',
    };
  };
  /* the style probe reads a span that is actually on screen, not a collapsed one */
  const firstEl = all.find((s) => s.style.display !== 'none') || all[0];
  const first = firstEl ? getComputedStyle(firstEl) : null;
  return {
    hidden: layer ? layer.hidden : null,
    spanCount: all.length,
    /* A character box that collapses under the page's own guard (sub-pixel after downscaling) is
     * rendered display:none — it stays in the DOM but cannot be hit, and its rect is all zeros.
     * Recorded so the run states honestly how many characters the layer could not place. */
    collapsed: all.filter((s) => s.style.display === 'none').length,
    kinds: [...new Set(all.map((s) => s.getAttribute('data-kind')))],
    layerText: all.map((s) => s.textContent).join(''),
    tilted: all.filter((s) => {
      const m = new DOMMatrixReadOnly(getComputedStyle(s).transform);
      return Math.abs(m.b) + Math.abs(m.c) > 0.05;
    }).length,
    style: first
      ? {
          userSelect: first.userSelect || first.webkitUserSelect,
          color: first.color,
          pointerEvents: first.pointerEvents,
          layerPointerEvents: getComputedStyle(layer).pointerEvents,
          layerZ: getComputedStyle(layer).zIndex,
        }
      : null,
    spans: all.slice(0, 60).map(geo),
    stage: { left: box.left, top: box.top, w: box.width, h: box.height },
    natural: (() => {
      const im = document.querySelector('#view');
      return im ? { w: im.naturalWidth, h: im.naturalHeight } : null;
    })(),
    regionItems: document.querySelectorAll('#regionList .region-item').length,
    zoom: (document.querySelector('#zoomLbl') || {}).textContent || null,
    /* the box the page fitted the image into, in case the fit itself is the thing going wrong */
    env: (() => {
      const d = document.querySelector('.drop');
      const w = document.querySelector('#stageWrap');
      const z = document.querySelector('#zoomIn');
      return {
        drop: d ? { w: Math.round(d.clientWidth), h: Math.round(d.clientHeight) } : null,
        wrap: w ? { w: w.clientWidth, h: w.clientHeight } : null,
        zoomInHidden: !z || z.hidden === true,
        zoomInDisabled: !!z && z.disabled === true,
      };
    })(),
    toasts: [...document.querySelectorAll('#toasts .toast')].map((t) => t.textContent),
  };
})())`;

/* A drag can only prove selection over a target the browser can actually hit. At 适应窗口 scale a
 * 3200px-wide sheet renders characters 1-3px tall, and Blink places the caret from the hit point,
 * so a scripted drag there would measure the test, not the layer. This asks the page for one run of
 * characters that are legibly sized and consecutive in their own line, **scrolls that run into view
 * the way a reader would** (`#stageWrap` is the page's own scroll container, so this is a normal
 * interaction and not a style poke), and only then asks the browser what is actually under the two
 * end points. Both the scroll and the hit test happen inside one evaluate, so no layout change can
 * stale the coordinates between measuring and dispatching. */
const DRAG_TARGET = `JSON.stringify((() => {
  const lines = [...document.querySelectorAll('#textLayer .tl-line')];
  const inView = (x, y) => x > 2 && y > 2 && x < innerWidth - 2 && y < innerHeight - 2;
  const name = (e) => (e ? e.tagName + '.' + (e.className || '?') : 'null');
  const meas = (l) => [...l.querySelectorAll('.tl-word')].map((s) => {
    const r = s.getBoundingClientRect();
    return { el: s, text: s.textContent, x: r.left + r.width / 2, y: r.top + r.height / 2, w: r.width, h: r.height };
  });
  /* consecutive runs, so the selection is one slice of one line and not a jump across the page */
  const bestRun = (sp, hit) => {
    let run = [];
    let best = [];
    for (const s of sp) {
      const ok =
        s.w >= 3 && s.h >= 3 && (!hit || (inView(s.x, s.y) && document.elementFromPoint(s.x, s.y) === s.el));
      run = ok ? run.concat([s]) : [];
      if (run.length > best.length) best = run;
    }
    return best;
  };
  let longest = 0;
  let longestLine = '';
  let maxH = 0;
  const cands = [];
  for (const l of lines) {
    const sp = meas(l);
    const best = bestRun(sp, false);
    if (best.length > longest) {
      longest = best.length;
      longestLine = sp.map((s) => s.text).join('');
    }
    if (sp.length) maxH = Math.max(maxH, ...sp.map((s) => s.h));
    if (best.length >= 6) cands.push({ l, best });
  }
  for (const c of cands) {
    c.best[Math.floor(c.best.length / 2)].el.scrollIntoView({ block: 'center', inline: 'center' });
    const best = bestRun(meas(c.l), true);
    if (best.length < 6) continue;
    const from = best[1];
    const to = best[best.length - 2];
    return {
      found: true,
      ri: Number(c.l.getAttribute('data-ri')),
      line: meas(c.l).map((s) => s.text).join(''),
      want: best.map((s) => s.text).join('').slice(1, -1),
      scrolled: true,
      from: { x: from.x, y: from.y, text: from.text, hit: name(document.elementFromPoint(from.x, from.y)) },
      to: { x: to.x, y: to.y, text: to.text, hit: name(document.elementFromPoint(to.x, to.y)) },
      run: best.length,
      zoom: (document.querySelector('#zoomLbl') || {}).textContent || null,
    };
  }
  return { found: false, lines: lines.length, longest, longestLine, maxH, zoom: (document.querySelector('#zoomLbl') || {}).textContent || null };
})())`;

/** Run the page against a live server and return the same shape as a recorded capture. */
async function record() {
  if (!base || !cdpPort || !imagePath) {
    throw new Error('live mode needs --base <url> --cdp <port> --image <path>');
  }
  mkdirSync(outDir, { recursive: true });

  async function targetWs() {
    for (let i = 0; i < 80; i += 1) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
        const page = list.find((t) => t.type === 'page');
        if (page && page.webSocketDebuggerUrl) return page.webSocketDebuggerUrl;
      } catch {}
      await sleep(250);
    }
    throw new Error('CDP target not found');
  }

  const ws = new WebSocket(await targetWs());
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = () => rej(new Error('ws error')); });
  let id = 0;
  const pending = new Map();
  const requests = [];
  const responses = [];
  const exceptions = [];
  ws.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id); return; }
    if (message.method === 'Network.requestWillBeSent') {
      requests.push({ id: message.params.requestId, method: message.params.request.method, url: message.params.request.url });
    } else if (message.method === 'Network.responseReceived') {
      responses.push({ id: message.params.requestId, status: message.params.response.status, url: message.params.response.url });
    } else if (message.method === 'Runtime.exceptionThrown') {
      exceptions.push(message.params.exceptionDetails?.exception?.description || message.params.exceptionDetails?.text);
    }
  };
  const send = (method, params) => {
    const mid = ++id;
    ws.send(JSON.stringify({ id: mid, method, params: params || {} }));
    return new Promise((res) => pending.set(mid, res));
  };
  const evaluate = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, returnByValue: true });
    const d = r.result || {};
    if (d.exceptionDetails) throw new Error(d.exceptionDetails.exception?.description || d.exceptionDetails.text);
    return d.result ? d.result.value : null;
  };

  await send('Runtime.enable');
  await send('Page.enable');
  await send('Network.enable');
  /* A real window first, then the same emulated viewport on top of it.
   *
   * The page's left column is a flex column whose image box is `flex:1; min-height:0`, so every row
   * the run adds (toolbar, file-meta, job line, toasts) is taken out of that box: at a 1000px-tall
   * window `.drop` measured 685x0 once a recognition had rendered, and the fit scale that follows is
   * a property of the harness, not of the layer. A 1400px window leaves the box ~600px, which is
   * what a reader on a normal desktop actually gets, and is the size the drag is checked at. */
  try {
    const win = await send('Browser.getWindowForTarget');
    const windowId = win.result && win.result.windowId;
    if (windowId != null) {
      await send('Browser.setWindowBounds', { windowId, bounds: { width: 1600, height: 1400 } });
    }
  } catch (error) {
    console.log(`  (window resize unavailable: ${error.message})`);
  }
  await send('Emulation.setDeviceMetricsOverride', { width: 1600, height: 1300, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `${base}/` });
  await sleep(2500);

  const document = await send('DOM.getDocument', { depth: -1 });
  const input = await send('DOM.querySelector', { nodeId: document.result.root.nodeId, selector: '#fileInput' });
  const set = await send('DOM.setFileInputFiles', { files: [imagePath], nodeId: input.result.nodeId });
  if (set.error) throw new Error(`setFileInputFiles: ${JSON.stringify(set.error)}`);
  await sleep(1200);

  /* The reporter's action: press 开始识别.
   *
   * The page refuses to submit before `/api/models` has been read — it will not guess model
   * readiness — and on a first run that GET itself can take seconds (model verification happens on
   * first use). So the click is retried until it demonstrably produced a POST /api/ocr, instead of
   * the run silently timing out on a page that was simply not ready yet. */
  const samples = [];
  let after = null;
  const sample = async () => {
    await sleep(500);
    const probe = JSON.parse(await evaluate(`JSON.stringify({
      stageClass: (document.querySelector('.stage')||{}).className || null,
      jobHidden: (document.querySelector('#jobLine')||{}).hidden,
      jobLine: (document.querySelector('#jobLine')||{}).textContent || null,
      regionItems: document.querySelectorAll('#regionList .region-item').length,
      toasts: [...document.querySelectorAll('#toasts .toast')].map(t => t.textContent)
    })`));
    samples.push(probe);
    after = probe;
    return probe;
  };
  const rendered = (p) => p.regionItems > 0 && !String(p.stageClass).includes('scanning');
  const posted = () => requests.some((r) => r.method === 'POST' && r.url.includes('/api/ocr'));

  for (let round = 0; round < 12 && !posted() && !rendered(after || {}); round += 1) {
    await evaluate(`document.querySelector('#ocrBtn').click(); 'clicked'`);
    for (let i = 0; i < 6 && !posted(); i += 1) await sample();
  }
  /* A POST is not a result: keep watching until the region list actually renders. */
  for (let i = 0; i < 120 && !rendered(after || {}); i += 1) await sample();
  const firstRegions = JSON.parse(await evaluate(
    `JSON.stringify([...document.querySelectorAll('#regionList .region-item')].slice(0,3).map(e => e.textContent.trim().slice(0,80)))`,
  ));

  /* ---- Step 2: the transparent text layer, driven for real -------------------------------
   * `?words=` is fixed at submit time (serve keeps it in the job's params), so the layer needs a
   * **second** recognition with the `chars` tier chosen through the page's own selector.
   *
   * The second job costs as much wall time as the first (the whole image is re-recognized), so the
   * wait is keyed on *that* job's `GET .../result` appearing — not on a fixed number of beats, and
   * not on the first job's result, which would silently compare the layer against the wrong body. */
  const isResult = (r) => r.method === 'GET' && /\/api\/jobs\/[^/]+\/result$/.test(r.url);
  const resultsBefore = requests.filter(isResult).length;
  await evaluate(
    `(() => { const b = document.querySelector('#wordsOpts [data-words="chars"]');`
    + ` if (!b) throw new Error('the page has no chars tier to select'); b.click(); return 'tier'; })()`,
  );
  const postedChars = () =>
    requests.some((r) => r.method === 'POST' && r.url.includes('words=chars'));
  for (let round = 0; round < 12 && !postedChars(); round += 1) {
    await evaluate(`document.querySelector('#ocrBtn').click(); 're-recognize'`);
    for (let i = 0; i < 6 && !postedChars(); i += 1) await sleep(500);
  }

  let layer = null;
  for (let i = 0; i < 300; i += 1) {
    await sleep(500);
    if (requests.filter(isResult).length <= resultsBefore) continue;
    layer = JSON.parse(await evaluate(LAYER_PROBE));
    if (layer.spanCount > 0 && layer.hidden === false) break;
  }

  /* The same body the page just rendered, fetched here so the layer is compared against the wire
   * rather than against a value this script invented. */
  const token = await evaluate(`(window.__RROCR__ || {}).token || ''`);
  const resultUrl = requests.filter(isResult).slice(resultsBefore).reverse()[0] || null;
  let wire = null;
  if (token && resultUrl) {
    const res = await fetch(resultUrl.url, {
      headers: { 'X-RapidStructure-Token': token, Origin: base },
    });
    if (res.status === 200) {
      const body = await res.json();
      wire = {
        size: body.image && body.image.original_size,
        regions: (body.regions || []).map((r) => ({
          kind: r.kind,
          text: r.recognition && r.recognition.text,
          words: (r.recognition && r.recognition.words) || null,
        })),
      };
    }
  }

  /* A real drag, and it is done at a zoom the browser can hit.
   *
   * `#textLayer` is laid out for whatever scale the page is at, so at 适应窗口 a 3200px sheet gives
   * characters ~2px tall with a share of boxes collapsing under the page's own sub-pixel guard —
   * caret placement there is a property of the pointer resolution, not of the layer. The page's own
   * 放大 button is used (never a synthetic style poke), and the drag runs over the first line whose
   * characters hit-test back to themselves. The target is re-evaluated immediately before the
   * dispatch, so a layout change in between cannot stale the coordinates. */
  let target = null;
  const zoomTries = [];
  for (let z = 0; z < 16 && !(target && target.found); z += 1) {
    await evaluate(`(() => { const b = document.querySelector('#zoomIn'); if (!b) throw new Error('the page has no zoom-in control'); b.click(); return 'zoom'; })()`);
    await sleep(300);
    target = JSON.parse(await evaluate(DRAG_TARGET));
    zoomTries.push({ z, zoom: target.zoom, found: target.found, longest: target.longest, maxH: target.maxH });
  }

  let drag = null;
  if (target && target.found) {
    target = JSON.parse(await evaluate(DRAG_TARGET));
    const move = (type, p) =>
      send('Input.dispatchMouseEvent', {
        type, x: p.x, y: p.y, button: 'left', clickCount: 1,
        buttons: type === 'mouseReleased' ? 0 : 1,
      });
    await evaluate(`(document.getSelection() || {}).removeAllRanges(); 'clear'`);
    await move('mousePressed', target.from);
    for (let i = 1; i <= 4; i += 1) {
      const t = i / 5;
      await move('mouseMoved', { x: target.from.x + (target.to.x - target.from.x) * t, y: target.from.y + (target.to.y - target.from.y) * t });
    }
    await move('mouseReleased', target.to);
    await sleep(200);
    const sel = await evaluate(`(() => { const g = document.getSelection();
      if (!g || !g.rangeCount) return null;
      const d = (n) => { while (n && n.nodeType === 3) n = n.parentElement; return n ? n.tagName + '.' + (n.className || '?') : 'null'; };
      return { text: g.toString(), collapsed: g.isCollapsed, ranges: g.rangeCount, anchor: d(g.anchorNode), focus: d(g.focusNode) }; })()`);
    drag = { ...target, selection: sel ? sel.text : null, range: sel };
    await evaluate(`(document.getSelection() || {}).removeAllRanges(); 'cleared'`);
  }

  /* The layer must follow the zoom the same way the overlay boxes do — same layout tick, same scale. */
  const layerZoomed = target && target.found ? JSON.parse(await evaluate(LAYER_PROBE)) : null;

  const capture = {
    baseUrl: base,
    assertTilt,
    charsRun: { posted: postedChars(), results: requests.filter(isResult).length - resultsBefore },
    apiResponses: responses
      .map((r) => ({ ...r, ...(requests.find((q) => q.id === r.id) || {}) }))
      .filter((r) => r.url.includes('/api/')),
    after: { ...after, firstRegions },
    samples,
    exceptions,
    layer,
    layerZoomed,
    zoomTries,
    wire,
    drag,
  };
  writeFileSync(join(outDir, 'cdp-flow.json'), JSON.stringify(capture, null, 2));
  ws.close();
  return capture;
}

const capture = capturePath ? JSON.parse(readFileSync(capturePath, 'utf8')) : await record();
const calls = capture.apiResponses || [];
const label = capture.baseUrl || '(recorded capture)';

const problems = [];
const check = (ok, message) => { if (!ok) problems.push(message); };

const polls = calls.filter((c) => /\/api\/jobs\/[^/]+$/.test(c.url) && c.method === 'GET');
const results = calls.filter((c) => /\/api\/jobs\/[^/]+\/result$/.test(c.url));
const submitted = calls.find((c) => c.url.includes('/api/ocr') && c.method === 'POST');

check(!!submitted, 'the page must POST /api/ocr');
check(!!submitted && submitted.status === 202, `POST /api/ocr must be accepted (202), got ${submitted && submitted.status}`);
check(polls.length > 0, 'the page must poll GET /api/jobs/{id} at least once — this is exactly what the pre-M5 page never did');
check(results.length > 0, 'the page must fetch GET /api/jobs/{id}/result');
check(results.every((r) => r.status === 200), `every /result fetch must be 200, got ${results.map((r) => r.status).join(',')}`);
check(!String(capture.after.stageClass).includes('scanning'), `the scanning animation must be cleared, stage class was \`${capture.after.stageClass}\``);
check(capture.after.regionItems > 0, `the region list must render at least one region, got ${capture.after.regionItems}`);
check((capture.exceptions || []).length === 0, `the page must raise no exception, got ${JSON.stringify(capture.exceptions)}`);
const firstText = (capture.after.firstRegions || [])[0] || '';
check(/[\p{L}\p{N}]/u.test(firstText), `the first rendered region must carry text, got ${JSON.stringify(firstText)}`);

/* ---- Step 2: the transparent text layer, measured in the browser ---- */
const layer = capture.layer;
const wire = capture.wire;
const tiltRequired = assertTilt || capture.assertTilt === true;

check(!!layer, 'the capture must carry the text-layer probe — an older recording cannot prove Step 2');
check(!!wire, 'the check must re-fetch the very /result body the page rendered, or it is asserting nothing about the wire');
if (capture.charsRun) {
  check(
    capture.charsRun.posted === true,
    'the page must re-submit with `words=chars` through its own tier selector — the layer cannot be proven against the default tier',
  );
  check(
    capture.charsRun.results > 0,
    `the second recognition must deliver its own /result (got ${capture.charsRun.results} new result fetch(es))`,
  );
}

if (layer && wire) {
  check(
    layer.hidden === false,
    `#textLayer must be visible once words=chars came back, hidden=${JSON.stringify(layer.hidden)}`,
  );
  check(layer.spanCount > 0, `the text layer must hold at least one span, got ${layer.spanCount}`);
  check(
    layer.kinds.length === 1 && layer.kinds[0] === 'char',
    `every span must carry the label the server sent (char), got ${JSON.stringify(layer.kinds)}`,
  );
  const st = layer.style || {};
  check(
    st.userSelect === 'text',
    `the spans must be selectable (computed user-select: text), got ${JSON.stringify(st.userSelect)}`,
  );
  check(
    /rgba\(0, 0, 0, 0\)|transparent/.test(String(st.color)),
    `the glyphs must stay invisible over the image (color: transparent), got ${JSON.stringify(st.color)}`,
  );
  check(
    st.layerPointerEvents === 'none',
    `the layer must not swallow clicks that belong to the overlay boxes, got pointer-events=${JSON.stringify(st.layerPointerEvents)}`,
  );

  /* (6) wire → DOM: byte-identical text, in the response's own order. */
  const expectedText = wire.regions
    .filter((r) => r.kind === 'text' && Array.isArray(r.words))
    .map((r) => r.words.map((w) => w.text).join(''))
    .join('');
  check(
    layer.layerText === expectedText,
    `the layer's text must be the wire's words concatenated in order, got ${JSON.stringify(String(layer.layerText).slice(0, 40))}`
    + ` expected ${JSON.stringify(String(expectedText).slice(0, 40))}`,
  );

  /* (7) each span's matrix must land on its own word polygon. An axis-aligned substitute would
   * agree on a straight row and differ everywhere else, which is why the tolerance is tight. */
  const size = wire.size || {};
  const sx = layer.stage.w / size.width;
  const sy = layer.stage.h / size.height;
  check(
    Number.isFinite(sx) && sx > 0 && Number.isFinite(sy) && sy > 0,
    `the display scale must be derivable from image.original_size, got sx=${sx} sy=${sy}`,
  );
  check(
    !layer.natural || (layer.natural.w === size.width && layer.natural.h === size.height),
    `the <img> the user sees must be the size the polygons are in, got ${JSON.stringify(layer.natural)}`
    + ` vs original_size ${JSON.stringify(size)}`,
  );

  const cos = (x, y) => {
    const n = Math.hypot(x[0], x[1]) * Math.hypot(y[0], y[1]);
    return n ? (x[0] * y[0] + x[1] * y[1]) / n : 0;
  };
  const geoBad = [];
  for (const s of layer.spans) {
    const region = wire.regions[s.ri];
    if (!region || region.kind !== 'text' || !Array.isArray(region.words)) {
      geoBad.push(`span ${s.ri}/${s.k} has no text region behind it on the wire`);
      continue;
    }
    const word = region.words[s.k];
    if (!word) {
      geoBad.push(`region ${s.ri} has no word ${s.k}: the DOM order drifted from the wire`);
      continue;
    }
    if (word.text !== s.text) {
      geoBad.push(`span ${s.ri}/${s.k} shows ${JSON.stringify(s.text)}, the wire says ${JSON.stringify(word.text)}`);
      continue;
    }
    if (s.gone) {
      /* The page declines to place a box that degenerates below half a display pixel. Such a span
       * must then be out of the layout entirely — keeping its text in the layer (so nothing is
       * silently dropped) while drawing an empty box elsewhere would be the worst of both. */
      if (s.box.w || s.box.h) {
        geoBad.push(`span ${s.ri}/${s.k} is hidden by the sub-pixel guard yet still occupies ${s.box.w}×${s.box.h}px`);
      }
      continue;
    }
    const q = word.polygon.points;
    /* The matrix is resolved against the span's containing block (`.tl-line` overlays `.stage` at
     * 0,0, and `.stage` has no border/padding), so `e`/`f` are **stage-relative** display pixels —
     * adding getBoundingClientRect()'s left/top here would double-count the viewport offset. */
    const p0 = [q[0][0] * sx, q[0][1] * sy];
    const u = [(q[1][0] - q[0][0]) * sx, (q[1][1] - q[0][1]) * sy];
    const d = [(q[3][0] - q[0][0]) * sx, (q[3][1] - q[0][1]) * sy];
    const [a, b, c, dd, e, f] = s.m;
    const lenD = Math.hypot(d[0], d[1]);
    if (Math.abs(e - p0[0]) > 2 || Math.abs(f - p0[1]) > 2) {
      geoBad.push(`span ${s.ri}/${s.k} starts at (${e.toFixed(1)},${f.toFixed(1)}), its polygon's p0 is (${p0[0].toFixed(1)},${p0[1].toFixed(1)})`);
      continue;
    }
    if (cos([a, b], u) < 0.9998) geoBad.push(`span ${s.ri}/${s.k} does not run along p0→p1 (cos=${cos([a, b], u).toFixed(5)})`);
    if (cos([c, dd], d) < 0.9998) geoBad.push(`span ${s.ri}/${s.k} does not run along p0→p3 (cos=${cos([c, dd], d).toFixed(5)})`);
    if (Math.abs(s.font - lenD) > Math.max(0.5, 0.02 * lenD)) {
      geoBad.push(`span ${s.ri}/${s.k} is ${s.font.toFixed(1)}px tall but its polygon is ${lenD.toFixed(1)}px`);
    }
  }
  check(
    geoBad.length === 0,
    `every span must sit on its own word polygon (affine, not an axis-aligned box): ${geoBad.slice(0, 3).join('; ')}`,
  );
  if (tiltRequired) {
    check(
      layer.tilted > 0,
      `--assert-tilt: at least one span must be measurably off-axis, got ${layer.tilted} of ${layer.spanCount}`,
    );
  }

  /* (8) the drag actually selected recognized characters, over a target the browser could hit. */
  const drag = capture.drag;
  check(
    !!drag && drag.found === true,
    `zooming in must produce a line of >=6 hit-testable character spans to drag across, got ${JSON.stringify(drag || { found: false })}`,
  );
  if (drag && drag.found) {
    const sel = String(drag.selection || '');
    const want = String(drag.want || '');
    check(
      /tl-word/.test(String(drag.from.hit)) && /tl-word/.test(String(drag.to.hit)),
      `the drag must start and end on the spans it names, got ${JSON.stringify(drag.from.hit)} / ${JSON.stringify(drag.to.hit)}`,
    );
    const wireLine = (wire.regions[drag.ri] || {}).words;
    check(
      Array.isArray(wireLine) && wireLine.map((w) => w.text).join('') === drag.line,
      `the dragged line must be the wire's words for region ${drag.ri}, got ${JSON.stringify(String(drag.line).slice(0, 40))}`,
    );
    check(sel.length > 0, `a real mouse drag across the layer must select something, got "${sel}"`);
    check(
      drag.line.includes(sel),
      `the selection must be a contiguous slice of that line's recognized text: selected ${JSON.stringify(sel)} in ${JSON.stringify(drag.line)}`,
    );
    /* the two characters under the press and release points are indeterminate (Blink puts the caret
     * at the nearest boundary), so the floor is the run's inner characters minus one at each end */
    check(
      sel.length >= Math.max(2, want.length - 2),
      `the drag run was ${want.length} characters (${JSON.stringify(want)}) but the selection held ${sel.length} (${JSON.stringify(sel)})`,
    );
  }

  /* (9) the layer tracks the page's own zoom: same spans, same text, fewer collapsed boxes. */
  const zoomed = capture.layerZoomed;
  if (zoomed) {
    check(
      zoomed.spanCount === layer.spanCount && zoomed.layerText === layer.layerText,
      `zooming must re-lay-out the same layer, not rebuild a different one: fit ${layer.spanCount}/${String(layer.layerText).length} chars, zoomed ${zoomed.spanCount}/${String(zoomed.layerText).length}`,
    );
    check(
      zoomed.collapsed <= layer.collapsed,
      `a larger display scale must collapse fewer boxes, got ${zoomed.collapsed} vs ${layer.collapsed}`,
    );
    check(
      zoomed.hidden === false,
      `the layer must stay visible across a zoom, hidden=${JSON.stringify(zoomed.hidden)}`,
    );
    check(
      String(zoomed.zoom).trim() !== String(layer.zoom).trim(),
      `the page's own zoom readout must have changed, stayed at ${JSON.stringify(layer.zoom)}`,
    );
    /* An enlarged stage is a scrollable box, so "same text" is not enough — every placed span must
     * still sit inside the displayed image (and a hidden one must hold no box at all, otherwise a
     * caller that ignored the guard would click the page chrome at the document origin). */
    const view = zoomed.stage;
    const stray = zoomed.spans.filter((p) => {
      if (p.gone) return p.box.w !== 0 || p.box.h !== 0;
      const b = p.box;
      return (
        b.left < view.left - 2 ||
        b.right > view.left + view.w + 2 ||
        b.top < view.top - 2 ||
        b.bottom > view.top + view.h + 2
      );
    });
    check(
      stray.length === 0,
      `after the zoom every placed span must still be inside the displayed image, ${stray.length} strayed (first ${JSON.stringify(stray[0] || null)})`,
    );
  }
}

console.log(`capture: ${label}`);
console.log(`  POST /api/ocr           -> ${submitted ? submitted.status : 'MISSING'}`);
console.log(`  GET /api/jobs/{id}      -> ${polls.length} poll(s) ${polls.map((p) => p.status).join(',')}`);
console.log(`  GET /api/jobs/{id}/result -> ${results.length} fetch(es) ${results.map((r) => r.status).join(',')}`);
console.log(`  .stage class            -> ${capture.after.stageClass}`);
console.log(`  rendered regions        -> ${capture.after.regionItems}`);
console.log(`  first region            -> ${JSON.stringify(String(firstText).replace(/\s+/g, ' ').trim().slice(0, 60))}`);
if (layer) {
  console.log(`  text layer (fit)        -> ${layer.spanCount} span(s), kinds ${JSON.stringify(layer.kinds)}, off-axis ${layer.tilted}, collapsed ${layer.collapsed}, zoom ${JSON.stringify(String(layer.zoom || '').trim())}`);
  console.log(`  layer text              -> ${layer.layerText.length} char(s), matches the wire's words${layer.layerText === ((wire || {}).regions || []).filter((r) => r.kind === 'text' && Array.isArray(r.words)).map((r) => r.words.map((w) => w.text).join('')).join('') ? ' (yes)' : ' (NO)'}`);
  const drag = capture.drag || {};
  console.log(`  drag selection          -> ${JSON.stringify(String(drag.selection || '').slice(0, 40))} of run ${JSON.stringify(String(drag.want || '').slice(0, 40))} (region ${drag.ri}, zoomed scale ${((capture.layerZoomed || {}).zoom || '?')})`);
  console.log(`  drag targets / range    -> ${JSON.stringify(drag.from && drag.from.hit)} ${JSON.stringify(drag.to && drag.to.hit)} ${JSON.stringify(drag.range)} run=${JSON.stringify(drag.run)} scrolled=${JSON.stringify(!!drag.scrolled)}`);
  if (capture.zoomTries) console.log(`  zoom attempts           -> ${JSON.stringify(capture.zoomTries.slice(-4))}`);
  if (layer.env) console.log(`  fit box / zoom control  -> ${JSON.stringify(layer.env)}`);
}

if (problems.length) {
  for (const problem of problems) console.error(`FAIL ${problem}`);
  console.error(`\npage flow check FAILED (${problems.length} problem(s))`);
  process.exit(1);
}
console.log('\npage flow check PASSED: one upload polls its job, fetches its result, clears the animation, renders regions, and the transparent text layer is selectable text that equals the wire, sits on its own polygons, follows the zoom, and yields to a real mouse drag');
