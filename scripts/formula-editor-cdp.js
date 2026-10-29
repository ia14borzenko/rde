// Headless-проверка окна визуального редактора формул RDE (P6, ADR-0022) через CDP.
//
// Это dev-инструмент (не часть продукта): продукт запускает браузер сам, а скрипт позволяет
// проверить реальную страницу MathLive + JS-мост без графической сессии:
//   node scripts/formula-editor-cdp.js <debug-port> <out.png> <mode>
// mode: `ui`    — дождаться загрузки, показать виртуальную клавиатуру, снять скриншот;
//       `apply` — ввести формулу визуально (команда MathLive) и нажать «Применить»: RDE-процесс
//                 `serve` должен напечатать EVENT=applied/LATEX=… (проверяет сквозной путь).
'use strict';

const fs = require('fs');

const DEBUG_PORT = process.argv[2] || '9333';
const OUT_PNG = process.argv[3] || '/tmp/rde-formula-editor.png';
const MODE = process.argv[4] || 'ui';
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function connect() {
  let pages = null;
  for (let i = 0; i < 120; i += 1) {
    try {
      const response = await fetch(`http://127.0.0.1:${DEBUG_PORT}/json/list`);
      pages = await response.json();
      if (pages && pages.length) return pages.find((page) => page.type === 'page');
    } catch (error) {
      /* браузер ещё поднимается */
    }
    await sleep(100);
  }
  throw new Error('CDP недоступен: браузер не поднялся');
}

async function main() {
  const page = await connect();
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  let nextId = 0;
  const pending = new Map();
  const errors = [];
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) {
      pending.get(message.id)(message);
      pending.delete(message.id);
      return;
    }
    if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
      errors.push(message.params.entry.text || 'log error');
    }
    if (message.method === 'Runtime.exceptionThrown') {
      errors.push(JSON.stringify(message.params.exceptionDetails).slice(0, 300));
    }
  };
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const id = ++nextId;
      pending.set(id, resolve);
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    const reply = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (reply.result && reply.result.exceptionDetails) {
      throw new Error('JS: ' + JSON.stringify(reply.result.exceptionDetails).slice(0, 400));
    }
    return reply.result && reply.result.result ? reply.result.result.value : undefined;
  };

  await send('Runtime.enable');
  await send('Page.enable');
  await send('Log.enable');

  // Готовность: MathLive определён, состояние из моста загружено (status ушёл с «Загрузка…»).
  let ready = false;
  for (let i = 0; i < 150; i += 1) {
    ready = await evaluate(
      '!!(window.MathLive && window.MathLive.MathfieldElement && document.getElementById("mf")' +
        ' && document.getElementById("status").textContent.indexOf("Загрузка") === -1)'
    );
    if (ready) break;
    await sleep(100);
  }
  if (!ready) throw new Error('страница редактора не загрузилась (MathLive/мост)');
  console.log('READY=true');
  console.log('TITLE=' + (await evaluate('document.title')));
  console.log('LATEX=' + JSON.stringify(await evaluate('document.getElementById("mf").value')));
  console.log('THEME=' + (await evaluate('document.documentElement.dataset.theme')));
  console.log('STATUS=' + (await evaluate('document.getElementById("status").textContent')));
  console.log('READY_MS=' + (await evaluate('Math.round(performance.now())')));

  // Виртуальная клавиатура (MathType-паритет: палитры/символы) — на скриншоте.
  await evaluate('document.getElementById("keyboard-toggle").click()');
  await sleep(600);
  const keyboardVisible = await evaluate('!!(window.mathVirtualKeyboard && window.mathVirtualKeyboard.visible)');
  console.log('VK_VISIBLE=' + keyboardVisible);

  if (MODE === 'apply') {
    // Визуальный ввод: вставка структуры командой MathLive (как клик по палитре).
    await evaluate('document.getElementById("mf").executeCommand(["insert", "\\\\frac{a+b}{c}"])');
    await sleep(300);
    console.log('LATEX_EDITED=' + JSON.stringify(await evaluate('document.getElementById("mf").value')));
    const applied = await evaluate('window.__rdeLastApplyMs = undefined; document.getElementById("apply").click(); true');
    console.log('APPLY_CLICKED=' + applied);
    let applyMs = -1;
    for (let i = 0; i < 80; i += 1) {
      try {
        const value = await evaluate('window.__rdeLastApplyMs');
        if (typeof value === 'number') {
          applyMs = value;
          break;
        }
      } catch (error) {
        break; // страница закрылась после «Применить» — нормально
      }
      await sleep(25);
    }
    console.log('APPLY_MS=' + applyMs);
  } else {
    // Скриншот: формула + виртуальная клавиатура + LaTeX-панель (доказательство MathType-класса).
    await evaluate('document.getElementById("latex-toggle").click()');
    await sleep(250);
    console.log('LATEX_PANEL=' + (await evaluate('!document.getElementById("latex-panel").hidden')));
    await evaluate('document.getElementById("latex-toggle").click()');
    await evaluate('document.getElementById("zoom-in").click()');
    await sleep(200);
    console.log('ZOOM=' + (await evaluate('document.getElementById("zoom-label").textContent')));
    const shot = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(OUT_PNG, Buffer.from(shot.result.data, 'base64'));
    console.log('SCREENSHOT=' + OUT_PNG + ' (' + Buffer.from(shot.result.data, 'base64').length + ' B)');
    // Тёплое переоткрытие: reload страницы в том же браузере (профиль/ассеты в кэше).
    await send('Page.reload', { ignoreCache: false });
    let warm = false;
    for (let i = 0; i < 150; i += 1) {
      warm = await evaluate(
        '!!(window.MathLive && window.MathLive.MathfieldElement && document.getElementById("status")' +
          ' && document.getElementById("status").textContent.indexOf("Загрузка") === -1)'
      );
      if (warm) break;
      await sleep(50);
    }
    console.log('WARM_READY=' + warm);
    console.log('WARM_READY_MS=' + (warm ? await evaluate('Math.round(performance.now())') : -1));
  }

  const csp = errors.filter((error) => error.toLowerCase().includes('content security policy'));
  console.log('JS_ERRORS=' + errors.length);
  console.log('CSP_VIOLATIONS=' + csp.length);
  for (const error of errors.slice(0, 5)) console.log('JS_ERROR: ' + JSON.stringify(error).slice(0, 300));
  socket.close();
}

main()
  .then(() => process.exit(0))
  .catch((error) => {
    console.error('FAIL: ' + (error && error.stack ? error.stack : error));
    process.exit(1);
  });
