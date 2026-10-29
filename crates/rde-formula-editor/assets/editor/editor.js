/* Окно визуального редактора формул RDE (P6, ADR-0022): MathLive + мост к RDE.
 *
 * Контракт с RDE (loopback-HTTP, токен в query страницы `?t=`):
 *   GET  /api/state  -> { ok, data: { title, latex, display, theme, zoom, project } }
 *   POST /api/apply   { latex }        — «Применить»: LaTeX уходит в RDE (запись в formula.toml)
 *   POST /api/cancel  {}              — «Отмена»: изменений нет
 *   POST /api/prefs   { zoom }        — зум сохраняется между открытиями
 * LaTeX — данные: страница не исполняет ничего из RDE и не ходит в сеть (CSP default-src 'self').
 */
(function () {
  'use strict';

  var params = new URLSearchParams(window.location.search);
  var TOKEN = params.get('t') || '';
  var api = function (path) { return path + '?t=' + encodeURIComponent(TOKEN); };

  var mf = document.getElementById('mf');
  var titleEl = document.getElementById('formula-title');
  var metaEl = document.getElementById('formula-meta');
  var zoomLabel = document.getElementById('zoom-label');
  var statusEl = document.getElementById('status');
  var bridgeEl = document.getElementById('bridge-state');
  var latexPanel = document.getElementById('latex-panel');
  var latexText = document.getElementById('latex-text');

  var state = { title: '', latex: '', display: true, theme: 'dark', zoom: 100, project: '' };
  var loaded = false;
  var finished = false;
  var zoom = 100;
  var offlinePolls = 0;
  var pollTimer = null;

  function setStatus(text, kind) {
    statusEl.textContent = text;
    statusEl.className = 'rde-status' + (kind ? ' ' + kind : '');
  }

  function setBridge(text, offline) {
    bridgeEl.textContent = text;
    bridgeEl.className = 'rde-bridge' + (offline ? ' offline' : '');
  }

  function currentLatex() {
    return (mf.value || '').trim();
  }

  function syncLatexPanel() {
    if (!latexPanel.hidden) latexText.value = mf.value;
  }

  function applyChrome() {
    document.documentElement.dataset.theme = state.theme === 'light' ? 'light' : 'dark';
    document.documentElement.style.setProperty('--rde-zoom', String(zoom / 100));
    zoomLabel.textContent = zoom + ' %';
    titleEl.textContent = state.title && state.title.trim() !== '' ? state.title : 'Новая формула';
    metaEl.innerHTML = '';
    var badge = document.createElement('span');
    badge.className = 'badge';
    badge.textContent = state.display ? 'display' : 'inline';
    metaEl.appendChild(badge);
    if (state.project) {
      var project = document.createElement('span');
      project.textContent = state.project;
      metaEl.appendChild(project);
    }
    document.title = 'Редактор формул — ' + (state.title || 'Новая формула');
  }

  function applyZoom(next, persist) {
    zoom = Math.max(100, Math.min(200, Math.round(next / 10) * 10));
    document.documentElement.style.setProperty('--rde-zoom', String(zoom / 100));
    zoomLabel.textContent = zoom + ' %';
    if (persist) {
      post('/api/prefs', { zoom: zoom }).catch(function () {});
    }
  }

  async function getState() {
    var response = await fetch(api('/api/state'), { cache: 'no-store' });
    if (!response.ok) throw new Error('state ' + response.status);
    return (await response.json()).data;
  }

  async function post(path, body) {
    var response = await fetch(api(path), {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body || {})
    });
    var reply = await response.json();
    if (!reply || reply.ok !== true) throw new Error((reply && reply.error) || ('HTTP ' + response.status));
    return reply;
  }

  function setBusy(busy) {
    document.getElementById('apply').disabled = busy || finished;
    document.getElementById('cancel').disabled = busy || finished;
  }

  async function applyFormula() {
    if (finished) return;
    var latex = currentLatex();
    if (!latex) {
      setStatus('Пустая формула — нечего применять', 'error');
      mf.focus();
      return;
    }
    setBusy(true);
    setStatus('Применяем…');
    var started = (window.performance && performance.now) ? performance.now() : Date.now();
    try {
      await post('/api/apply', { latex: latex });
      window.__rdeLastApplyMs = Math.round(((window.performance && performance.now) ? performance.now() : Date.now()) - started);
      finished = true;
      setStatus('Применено ✓', 'ok');
      setBridge('RDE получил формулу и закрывает окно');
      window.setTimeout(function () { try { window.close(); } catch (error) {} }, 250);
    } catch (error) {
      setBusy(false);
      setStatus('Не удалось применить: ' + error.message, 'error');
    }
  }

  async function cancelEdit() {
    if (finished) return;
    finished = true;
    setBusy(true);
    setStatus('Отмена — изменений нет');
    try {
      await post('/api/cancel', {});
    } catch (error) {
      /* Мост уже закрыт — окно всё равно закрываем. */
    }
    try { window.close(); } catch (error) {}
  }

  function toggleVirtualKeyboard(show) {
    try {
      var target = mf;
      if (window.mathVirtualKeyboard) {
        var visible = window.mathVirtualKeyboard.visible;
        var next = typeof show === 'boolean' ? show : !visible;
        if (next) {
          window.mathVirtualKeyboard.show({ animate: false });
        } else {
          window.mathVirtualKeyboard.hide({ animate: false });
        }
      } else if (target && target.executeCommand) {
        target.executeCommand('toggleVirtualKeyboard');
      }
      mf.focus();
    } catch (error) {
      /* Виртуальная клавиатура недоступна — не мешаем вводу. */
    }
  }

  function toggleLatexPanel(show) {
    var next = typeof show === 'boolean' ? show : latexPanel.hidden;
    latexPanel.hidden = !next;
    if (next) {
      latexText.value = mf.value;
      latexText.focus();
    } else {
      mf.focus();
    }
  }

  function mathLiveElement() {
    /* UMD-бандл MathLive кладёт класс в window.MathLive.MathfieldElement; на случай ESM — и глобально. */
    var bundle = window.MathLive || {};
    return bundle.MathfieldElement || window.MathfieldElement || null;
  }

  function installMathLive() {
    var element = mathLiveElement();
    if (element) {
      /* Офлайн-бандл без аудио: звуки не загружаем (никаких сетевых/локальных звуков). */
      try {
        element.soundsDirectory = null;
      } catch (error) {}
      try {
        element.fontsDirectory = './mathlive/fonts';
      } catch (error) {}
    }
    if (!mf) return;
    mf.addEventListener('input', function () {
      syncLatexPanel();
      if (!finished) setStatus('LaTeX: ' + currentLatex().length + ' символов');
    });
    mf.addEventListener('change', syncLatexPanel);
  }

  function installButtons() {
    document.getElementById('apply').addEventListener('click', applyFormula);
    document.getElementById('cancel').addEventListener('click', cancelEdit);
    document.getElementById('keyboard-toggle').addEventListener('click', function () { toggleVirtualKeyboard(); });
    document.getElementById('latex-toggle').addEventListener('click', function () { toggleLatexPanel(); });
    document.getElementById('clear').addEventListener('click', function () {
      if (finished) return;
      mf.value = '';
      mf.focus();
      syncLatexPanel();
      setStatus('Поле очищено');
    });
    document.getElementById('zoom-in').addEventListener('click', function () { applyZoom(zoom + 10, true); });
    document.getElementById('zoom-out').addEventListener('click', function () { applyZoom(zoom - 10, true); });
    document.getElementById('latex-from-field').addEventListener('click', function () {
      latexText.value = mf.value;
      setStatus('LaTeX скопирован из редактора');
    });
    document.getElementById('latex-to-field').addEventListener('click', function () {
      mf.value = latexText.value;
      mf.focus();
      setStatus('LaTeX-источник загружен в редактор');
    });
  }

  function installShortcuts() {
    document.addEventListener('keydown', function (event) {
      var ctrl = event.ctrlKey || event.metaKey;
      if (ctrl && event.key === 'Enter') {
        event.preventDefault();
        applyFormula();
      } else if (event.key === 'Escape' && !event.defaultPrevented) {
        if (window.mathVirtualKeyboard && window.mathVirtualKeyboard.visible) {
          event.preventDefault();
          toggleVirtualKeyboard(false);
        } else if (!latexPanel.hidden) {
          event.preventDefault();
          toggleLatexPanel(false);
        } else {
          event.preventDefault();
          cancelEdit();
        }
      } else if (ctrl && (event.key === 'k' || event.key === 'K')) {
        event.preventDefault();
        toggleVirtualKeyboard();
      } else if (ctrl && event.shiftKey && (event.key === 'l' || event.key === 'L')) {
        event.preventDefault();
        toggleLatexPanel();
      } else if (ctrl && (event.key === '=' || event.key === '+')) {
        event.preventDefault();
        applyZoom(zoom + 10, true);
      } else if (ctrl && event.key === '-') {
        event.preventDefault();
        applyZoom(zoom - 10, true);
      }
    });
    window.addEventListener('beforeunload', function () {
      /* Окно закрыли крестиком: RDE увидит завершение процесса и трактует как «Отмена». */
    });
  }

  function startPolling() {
    pollTimer = window.setInterval(async function () {
      try {
        var next = await getState();
        if (!next) return;
        offlinePolls = 0;
        setBridge('связь с RDE ✓');
        var chromeChanged = next.theme !== state.theme || next.title !== state.title || next.display !== state.display || next.project !== state.project;
        var zoomChanged = typeof next.zoom === 'number' && next.zoom !== zoom;
        var wasLoaded = loaded;
        state = next;
        if (!wasLoaded) {
          loaded = true;
          zoom = next.zoom || 100;
          mf.defaultMode = next.display ? 'math' : 'inline-math';
          mf.value = next.latex || '';
          applyChrome();
          setStatus('LaTeX: ' + currentLatex().length + ' символов');
          mf.focus();
        } else if (chromeChanged || zoomChanged) {
          if (zoomChanged) zoom = next.zoom;
          applyChrome();
        }
      } catch (error) {
        offlinePolls += 1;
        if (offlinePolls >= 3) {
          setBridge('мост закрыт — RDE завершил сеанс', true);
          if (pollTimer) window.clearInterval(pollTimer);
          pollTimer = null;
        }
      }
    }, 700);
  }

  installMathLive();
  installButtons();
  installShortcuts();
  setBusy(false);
  applyChrome();
  startPolling();
  /* Первый кадр: сразу тянем состояние, чтобы окно открылось с готовой формулой. */
  getState()
    .then(function (next) {
      if (next) state = next;
    })
    .catch(function () {})
    .then(function () {
      if (!mf.textContent && !mf.value) mf.focus();
    });
})();
