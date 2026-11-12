// Headless browser inspection of the isolated synthetic DSH web fixture.
// The DSH access token stays inside the process; do not print URLs from web.out.
import { spawn } from 'node:child_process';
import { readFile, writeFile } from 'node:fs/promises';
import { randomUUID } from 'node:crypto';
import path from 'node:path';

const root = path.resolve(process.argv[2] || '');
if (!path.basename(root).startsWith('sic-web-view-ui-')) throw new Error('invalid fixture root');
const log = await readFile(path.join(root, 'web.out'), 'utf8');
const url = log.match(/http:\/\/127\.0\.0\.1:\d+\/\?token=[A-Za-z0-9_-]+/)?.[0];
if (!url) throw new Error('fixture web URL unavailable');
const chromeHome = path.join(root, `chrome-fixture-${randomUUID()}`);
const chrome = spawn('C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe', [
  '--headless=new', '--no-first-run', '--no-default-browser-check', '--disable-gpu',
  '--disable-extensions', '--disable-background-networking', '--metrics-recording-only',
  '--host-resolver-rules=MAP * 0.0.0.0, EXCLUDE 127.0.0.1',
  `--user-data-dir=${chromeHome}`, '--remote-debugging-port=0', 'about:blank',
], { windowsHide: true, stdio: 'ignore' });
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let ws;
let id = 0;
const waiting = new Map();
const errors = [];
const responses = [];
const writes = [];
const extraHeaders = new Map();
try {
  let port;
  for (let i = 0; i < 100 && !port; i++) {
    try { port = Number((await readFile(path.join(chromeHome, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); } catch {}
    if (!port) await sleep(100);
  }
  if (!port) throw new Error('Chrome debug port unavailable');
  const targets = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  const target = targets.find(entry => entry.type === 'page');
  if (!target) throw new Error('Chrome target unavailable');
  ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }); });
  ws.addEventListener('message', message => {
    const data = JSON.parse(message.data);
    if (data.method === 'Runtime.exceptionThrown') errors.push(data.params?.exceptionDetails?.text || 'browser exception');
    if (data.method === 'Network.requestWillBeSentExtraInfo') extraHeaders.set(data.params.requestId, data.params.headers);
    if (data.method === 'Network.requestWillBeSent' && data.params?.request?.url?.includes('/api/sic-core/decision-event')) {
      const request = data.params.request, headers = request.headers;
      writes.push({ requestId: data.params.requestId, origin: headers.Origin ?? headers.origin ?? null, referer: headers.Referer ? new URL(headers.Referer).origin : null,
        contentType: headers['Content-Type'] ?? headers['content-type'] ?? null, urlOrigin: new URL(request.url).origin });
    }
    if (data.method === 'Network.responseReceived' && data.params?.response?.url?.includes('/api/sic-core/')) {
      const response = data.params.response;
      responses.push({ path: new URL(response.url).pathname, status: response.status, requestId: data.params.requestId });
    }
    if (!data.id) return;
    const pair = waiting.get(data.id);
    if (!pair) return;
    waiting.delete(data.id);
    if (data.error) pair.reject(new Error(data.error.message)); else pair.resolve(data.result);
  });
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const requestId = ++id; waiting.set(requestId, { resolve, reject });
    ws.send(JSON.stringify({ id: requestId, method, params }));
  });
  const evaluate = async expression => {
    const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) throw new Error('browser evaluation failed');
    return result.result?.value;
  };
  await send('Page.enable'); await send('Runtime.enable'); await send('Network.enable');
  await send('Page.navigate', { url });
  let text = '';
  for (let i = 0; i < 100; i++) {
    await sleep(250);
    text = await evaluate('document.body?.innerText || ""');
    if (text.length > 20 && !text.includes('Loading plugins')) break;
  }
  // A fresh DSH home can render the preview notice after the shell itself.
  // Wait for and dismiss the actual notice, rather than racing its first paint.
  let previewDismissed = false;
  let previewClicked = false;
  for (let i = 0; i < 80 && !previewDismissed; i++) {
    const preview = await evaluate(`(() => {
      const button = [...document.querySelectorAll('button')].find(x => x.innerText.trim() === '继续');
      if (button) { button.click(); return { clicked: true }; }
      return { shown: document.body.innerText.includes('预览版说明'), workspace: document.body.innerText.includes('工作区') };
    })()`);
    if (preview.clicked) previewClicked = true;
    previewDismissed = !preview.clicked && !preview.shown && preview.workspace && (previewClicked || i >= 20);
    if (!previewDismissed) await sleep(250);
  }
  if (!previewDismissed) throw new Error('DSH preview notice could not be dismissed');
  await evaluate(`([...document.querySelectorAll('button')].find(x => (x.getAttribute('aria-label') || '') === '打开侧边栏')?.click(), true)`);
  await sleep(500);
  await evaluate(`([...document.querySelectorAll('*')].find(x => x.textContent?.trim() === 'SIC synthetic UI session, reply')?.click(), true)`);
  await sleep(1200);
  await evaluate(`([...document.querySelectorAll('[role="tab"]')].find(x => x.textContent.trim() === 'SIC')?.click(), true)`);
  await sleep(1200);
  text = await evaluate('document.body?.innerText || ""');
  const expected = ['SIC 会话', '该 DSH 会话最近一次发布', 'Editor / Worker 模型用量', 'openai/mock-model',
    'DSH 原生工具事件', 'read', '工具返回正常', '续接时状态'];
  const missing = expected.filter(value => !text.includes(value));
  if (missing.length || errors.length) {
    console.log(JSON.stringify({ missing, exceptions: errors.length,
      text: text.slice(0, 5000).replace(/token=[A-Za-z0-9_-]+/g, 'token=[redacted]') }));
    throw new Error('DSH web overview did not display the expected synthetic fixture');
  }
  const click = label => evaluate(`(() => { const el = [...document.querySelectorAll('button')].find(x => x.textContent.trim() === ${JSON.stringify(label)}); if (!el) return false; el.click(); return true; })()`);
  async function waitFor(needle) {
    for (let i = 0; i < 40; i++) {
      await sleep(200);
      if ((await evaluate('document.body?.innerText || ""')).includes(needle)) return true;
    }
    return false;
  }
  const overviewCount = responses.filter(row => row.path.endsWith('/overview') && row.status === 200).length;
  if (!(await click('刷新状态'))) throw new Error('Overview refresh control missing');
  for (let i = 0; i < 40 && responses.filter(row => row.path.endsWith('/overview') && row.status === 200).length <= overviewCount; i++) {
    await sleep(200);
  }
  if (responses.filter(row => row.path.endsWith('/overview') && row.status === 200).length <= overviewCount || !(await waitFor('Editor / Worker 模型用量'))) {
    throw new Error('Manual overview refresh did not reload');
  }
  if (!(await click('读取时间线')) || !(await waitFor('展开 #1'))) throw new Error('WB/CB timeline did not load');
  if (!(await click('展开 #1')) || !(await waitFor('第 1 层对应关系'))) throw new Error('WB/CB pair did not expand');
  if (!(await click('读取 CB')) || !(await waitFor('CB 内容'))) throw new Error('CB content page did not load');
  const contentVisible = await evaluate(`document.body.innerText.includes('SIC synthetic UI session') &&
    [...document.querySelectorAll('pre')].some(el => el.textContent.includes('SIC synthetic UI session'))`);
  if (!contentVisible) throw new Error('CB page did not expose its synthetic content');
  if (!(await click('读取决策树')) || !(await waitFor('当前版本'))) throw new Error('Decision workspace did not load');
  const inputReady = await evaluate(`(() => { const input = document.querySelector('textarea[aria-label="决策条目"]');
    if (!input) return false; Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(input, 'Synthetic decision item for browser acceptance');
    input.dispatchEvent(new Event('input', { bubbles: true })); return true; })()`);
  await sleep(100);
  if (!inputReady || !(await click('保存决策条目')) || !(await waitFor('条目已保存。'))) {
    const detail = await evaluate(`({text:document.body.innerText.slice(-600),
      input:document.querySelector('textarea[aria-label="决策条目"]')?.value,
      disabled:[...document.querySelectorAll('button')].find(x=>x.textContent.trim()==='保存决策条目')?.disabled})`);
    const latestWrite = responses.filter(row => row.path.endsWith('/decision-event')).at(-1);
    let errorBody;
    if (latestWrite) { try { errorBody = (await send('Network.getResponseBody', { requestId: latestWrite.requestId })).body?.slice(0, 500); } catch {} }
    const safeWrites = writes.map(({ requestId, ...item }) => { const headers = extraHeaders.get(requestId) || {};
      return { ...item, extraOrigin: headers.Origin ?? headers.origin ?? null,
        fetchSite: headers['Sec-Fetch-Site'] ?? headers['sec-fetch-site'] ?? null }; });
    console.log(JSON.stringify({ decisionFailure: detail, responses: responses.map(({ path, status }) => ({ path, status })), writes: safeWrites, errorBody }));
    throw new Error('Decision item was not saved');
  }
  if (!(await click('读取决策树')) || !(await waitFor('Synthetic decision item for browser acceptance'))) throw new Error('Saved decision did not survive reload');
  await evaluate(`document.querySelector('[aria-label="WB 与 CB 时间线"]')?.scrollIntoView({block:'start'})`);
  const screenshot = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(path.join(root, 'web-inspect.png'), Buffer.from(screenshot.data, 'base64'));
  console.log(JSON.stringify({ sessionOpened: true, sicTabOpened: true, wbCbPublicationVisible: true,
    editorWorkerUsageVisible: true, manualRefreshVerified: true, nativeReadOutcomeVisible: true, timelineExpanded: true,
    cbPageVisible: true, decisionEditedAndReloaded: true, runtimeExceptions: 0 }));
} finally {
  ws?.close();
  chrome.kill();
}
