// GGUF Chatbox — main script

// ── Tauri bridge helpers ──────────────────────────────────────────────────

async function invoke(cmd, args) {
  return window.__TAURI__.core.invoke(cmd, args || {});
}
function listen(event, handler) {
  return window.__TAURI__.event.listen(event, handler);
}

// ── ExpansionSlots manager ────────────────────────────────────────────────

window.ExpansionSlots = (function () {
  const slots = {};
  let openSlot = null;

  function tab(n) { return document.querySelector(`.exp-tab[data-slot="${n}"]`); }
  function tray(n) { return document.getElementById(`exp-tray-${n}`); }
  function content(n) { return document.getElementById(`exp-slot-${n}-content`); }

  function open(n) {
    if (openSlot !== null && openSlot !== n) {
      closeTray(openSlot);
    }
    if (openSlot === n) { closeTray(n); return; }

    const t = tray(n);
    if (!t) return;
    t.style.display = 'flex';
    if (tab(n)) tab(n).classList.add('active');
    openSlot = n;

    const s = slots[n];
    if (s && !s._initialised) {
      const c = content(n);
      if (c && s.init) s.init(c);
      s._initialised = true;
    }
  }

  function closeTray(n) {
    const t = tray(n);
    if (t) t.style.display = 'none';
    if (tab(n)) tab(n).classList.remove('active');
    if (openSlot === n) openSlot = null;
  }

  function register(n, config) {
    slots[n] = Object.assign({ _initialised: false }, config);
    const tb = tab(n);
    if (tb) {
      if (config.icon) tb.querySelector('.exp-tab-icon').textContent = config.icon;
      if (config.label) tb.setAttribute('title', config.label);
    }
    const t = tray(n);
    if (t && config.label) {
      const headerSpan = t.querySelector('.exp-tray-header span');
      if (headerSpan) headerSpan.textContent = config.label;
    }
  }

  function isOpen(n) { return openSlot === n; }

  // Wire tab buttons
  document.querySelectorAll('.exp-tab').forEach(btn => {
    btn.addEventListener('click', () => open(Number(btn.dataset.slot)));
  });
  document.querySelectorAll('.exp-tray-close').forEach(btn => {
    btn.addEventListener('click', () => closeTray(Number(btn.dataset.slot)));
  });

  return { register, open, close: closeTray, isOpen };
})();

// ── ChatEngine ────────────────────────────────────────────────────────────

window.ChatEngine = (function () {
  const history = [];
  const callbacks = [];
  let generating = false;

  function getHistory() { return history.slice(); }

  function onMessage(cb) { callbacks.push(cb); }

  function currentTokenCount() {
    const allText = history.map(m => m.content).join(' ');
    return Math.floor(allText.length / 2);
  }

  function sendSystemMessage(text) {
    appendMessage('system', text, true);
  }

  function appendMessage(role, content, invisible) {
    const msg = { role, content };
    history.push(msg);
    callbacks.forEach(cb => cb(msg));
    if (!invisible) renderMessage(role, content);
  }

  function renderMessage(role, text) {
    const messagesDiv = document.getElementById('messages');
    const div = document.createElement('div');
    div.className = `message ${role}`;
    div.innerHTML = `
      <div class="message-role">${role === 'user' ? 'You' : 'Assistant'}</div>
      <div class="message-bubble">${escHtml(text)}</div>`;
    if (role !== 'system') addCopyBtn(div, () => text);
    messagesDiv.appendChild(div);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    return div;
  }

  // Per-message one-click copy button (self-styled; copies the raw message text).
  function addCopyBtn(div, getText) {
    const btn = document.createElement('button');
    btn.textContent = '📋 copy';
    btn.title = 'Copy this message';
    btn.style.cssText = 'display:block;margin-top:4px;font-size:11px;padding:2px 6px;'
      + 'cursor:pointer;background:transparent;border:1px solid rgba(255,255,255,0.2);'
      + 'border-radius:4px;color:inherit;opacity:0.55;';
    btn.onmouseenter = () => { btn.style.opacity = '1'; };
    btn.onmouseleave = () => { btn.style.opacity = '0.55'; };
    btn.addEventListener('click', (e) => {
      e.stopPropagation();
      navigator.clipboard.writeText(getText() || '').then(() => {
        btn.textContent = '✓ copied';
        setTimeout(() => { btn.textContent = '📋 copy'; }, 1200);
      }).catch(() => {});
    });
    div.appendChild(btn);
  }

  function escHtml(s) {
    return s.replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
  }

  // Start streaming assistant bubble
  let activeBubble = null;
  let activeBubbleDiv = null;
  let activeTokens = '';

  function startAssistantBubble() {
    const messagesDiv = document.getElementById('messages');
    const div = document.createElement('div');
    div.className = 'message assistant';
    div.innerHTML = `<div class="message-role">Assistant</div><div class="message-bubble"></div>`;
    messagesDiv.appendChild(div);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    activeBubble = div.querySelector('.message-bubble');
    activeBubbleDiv = div;
    activeTokens = '';
  }

  function appendToken(token, raw) {
    if (!activeBubble) return;
    // CLI path emits whole LINES (needs the newline back); server path emits
    // raw mid-word fragments (must be appended verbatim).
    activeTokens += raw ? token : token + '\n';
    activeBubble.textContent = activeTokens;
    document.getElementById('messages').scrollTop = document.getElementById('messages').scrollHeight;
  }

  // Per-bubble model attribution: retitle the streaming bubble's role line
  // with the identity of the brain that is answering (e.g. "cd2 · gemma").
  function labelActiveBubble(label) {
    if (!activeBubbleDiv || !label) return;
    const roleEl = activeBubbleDiv.querySelector('.message-role');
    if (roleEl) roleEl.textContent = label;
  }

  function finaliseAssistantBubble() {
    if (activeBubble) {
      history.push({ role: 'assistant', content: activeTokens });
      const finalText = activeTokens;
      if (activeBubbleDiv) addCopyBtn(activeBubbleDiv, () => finalText);
      activeBubble = null;
      activeBubbleDiv = null;
    }
  }

  async function sendMessage(text) {
    if (generating || !text.trim()) return;
    generating = true;

    const sendBtn = document.getElementById('btn-send');
    const stopBtn = document.getElementById('btn-stop');
    const input   = document.getElementById('chat-input');

    sendBtn.disabled = true;
    stopBtn.disabled = false;
    input.disabled = true;

    appendMessage('user', text, false);

    startAssistantBubble();

    try {
      await invoke('cmd_send_message', { text });
    } catch (err) {
      if (activeBubble) activeBubble.textContent = '[Error: ' + err + ']';
    }

    // done event finalises the bubble
    generating = false;
    sendBtn.disabled = false;
    stopBtn.disabled = true;
    input.disabled = false;
    input.value = '';
    input.focus();
  }

  async function stopGeneration() {
    try { await invoke('cmd_stop_generation'); } catch (_) {}
  }

  return {
    getHistory, onMessage, currentTokenCount,
    sendSystemMessage, sendMessage, stopGeneration,
    appendToken, finaliseAssistantBubble, labelActiveBubble,
    renderStored: renderMessage,   // repaint one stored {role,content} (layer viewer)
  };
})();

// ── CD-Changer chat layers ────────────────────────────────────────────────
// Three persistent conversations, one per magazine slot (cd1/cd2/cd3). The
// ACTIVE layer (the loaded disk) is where live turns land; the buttons bring any
// layer's transcript forward. The active layer is steady green and pulses yellow
// only while inferring — an activity lightboard. Because each layer keeps its own
// history, a CD-changer swap never wipes another layer's turns (this retires the
// hop-hygiene window-clear). (An orchestrator like SOC maps cd1/2/3 to its own
// agents, but the chatbox stays SOC-agnostic — it only knows cd1/cd2/cd3.)
(function () {
  let activeLayer = 0;
  let viewedLayer = 0;
  let inferring = false;
  const btns = Array.from(document.querySelectorAll('.cd-layer-btn'));
  if (!btns.length) return;

  function updateButtons() {
    btns.forEach(b => {
      const i = parseInt(b.dataset.layer, 10);
      b.classList.toggle('cd-active', i === activeLayer);          // steady green = current layer
      b.classList.toggle('cd-viewed', i === viewedLayer);          // outline = currently shown
      b.classList.toggle('cd-inferring', inferring && i === activeLayer); // yellow pulse = working now
    });
  }

  // Called from the streaming listener: pulse the active layer only WHILE it
  // generates. Idle = calm green, no animation.
  function setInferring(active) {
    inferring = !!active;
    updateButtons();
  }

  async function renderLayer(index) {
    try {
      const data = await invoke('cmd_get_layer', { index });
      const msgs = (data && data.messages) || [];
      const pane = document.getElementById('messages');
      if (!pane) return;
      pane.innerHTML = '';
      msgs.forEach(m => ChatEngine.renderStored(m.role, m.content));
      pane.scrollTop = pane.scrollHeight;
    } catch (e) { /* backend not ready yet — ignore */ }
  }

  function viewLayer(index) {
    viewedLayer = index;
    updateButtons();
    renderLayer(index);
  }

  btns.forEach(b => b.addEventListener('click',
    () => viewLayer(parseInt(b.dataset.layer, 10))));

  // Backend announces the active layer on every load / swap / server-mode send.
  listen('layer-changed', (event) => {
    const i = (event.payload && typeof event.payload.index === 'number')
      ? event.payload.index : 0;
    activeLayer = i;
    inferring = false;   // a swap ended the old generation; next token re-arms it
    // Follow live: snap the view to the layer that just became active so the
    // operator always watches the working agent. They can click any button to
    // read another layer between swaps.
    if (viewedLayer !== i) { viewedLayer = i; renderLayer(i); }
    updateButtons();
  });

  updateButtons();
  window.ChatLayers = { viewLayer, setInferring };
})();

// ── Streaming listener ────────────────────────────────────────────────────


// Message tag filter state
let filterMsgTags = true;
invoke('cmd_get_settings').then(s => {
  filterMsgTags = (s && typeof s.filter_msg_tags !== 'undefined') ? !!s.filter_msg_tags : true;
});
window.addEventListener('focus', () => {
  invoke('cmd_get_settings').then(s => {
    filterMsgTags = (s && typeof s.filter_msg_tags !== 'undefined') ? !!s.filter_msg_tags : true;
  });
});

listen('chat-token', event => {
  const { token, done, raw: rawFrag, error } = event.payload;
  if (done) {
    // Surface backend errors in the bubble instead of leaving it empty
    // (e.g. "server chat failed — is the server running on :8080?").
    if (error) ChatEngine.appendToken('[' + error + ']', true);
    ChatEngine.finaliseAssistantBubble();
    if (window.ChatLayers) window.ChatLayers.setInferring(false);   // pulse off
    return;
  }
  // A token is arriving → the active layer is generating: pulse it.
  if (window.ChatLayers) window.ChatLayers.setInferring(true);
  if (rawFrag) {
    // Server-streamed delta: verbatim fragment, and the CLI-debris tag filter
    // must not eat prose that happens to contain "end of"/"eof".
    ChatEngine.appendToken(token, true);
    return;
  }
  // Filter out tag messages if enabled (robust matching)
  const rawTok = token || '';
  // Remove leading non-alphanumeric characters (quotes, >, punctuation)
  const cleaned = rawTok.replace(/^[^a-zA-Z0-9]+/, '').toLowerCase().trim();
  const isInterrupted = cleaned.includes('interrupted by user') || cleaned.includes('interrupted');
  const isEof = cleaned.includes('eof') || cleaned.includes('end of') || cleaned.includes('end-of') || cleaned.includes('end of input') || cleaned.includes('end of generation');
  const isTag = isInterrupted || isEof;
  if (filterMsgTags && isTag) return;
  ChatEngine.appendToken(token);
});

// Per-bubble attribution: the backend announces which model is answering
// (local path = the GUI-loaded model; server path = whatever llama-server is
// actually serving, i.e. the CD-changer disk). NOTHING is hardcoded — the
// label is derived from the live path every reply, so swapping a new model
// into a slot relabels bubbles automatically.
listen('chat-model', event => {
  const p = (event.payload && event.payload.model) || '';
  if (!p) return;
  const label = (window.ModelTray && window.ModelTray.attributionFor)
    ? window.ModelTray.attributionFor(p) : p;
  ChatEngine.labelActiveBubble(label);
});

// Remote New-Chat (SOC POSTs :8086/chat/clear between CD-changer hops): the
// backend already wiped the history — mirror it in the pane.
listen('chat-cleared', () => {
  const m = document.getElementById('messages');
  if (m) m.innerHTML = '';
});

// ── Model Tray engine ─────────────────────────────────────────────────────

// Debug probe: confirm UI script load
try { console.log('[ui] frontend/main.js loaded'); invoke('cmd_log', { line: '[ui] frontend/main.js loaded' }).catch(()=>{}); } catch(e) {}

(function () {
  // ── CD-Changer Magazine loader ──────────────────────────────────────────
  // Data-driven: MAG_SIZE slots, each = one "disk" {model_path, mmproj_path}.
  // 3 disks now; scaling to 10 later is this ONE number (slots list scrolls).
  // The magazine array persists in settings.json — SOC Ultralight reads it as
  // the CD-changer disk registry for the :8086/swap control endpoint.
  const MAG_SIZE = 3;

  const trayEl     = document.getElementById('model-tray');
  const btnToggle  = document.getElementById('btn-model-tray');
  const slotsEl    = document.getElementById('magazine-slots');
  const btnEject   = document.getElementById('btn-mag-eject');
  const statusEl   = document.getElementById('model-tray-status');
  const indicator  = document.getElementById('model-indicator');
  const modelLabel = document.getElementById('chat-model-label');
  const warningEl  = document.getElementById('no-model-warning');

  let magazine    = [];    // [{model_path, mmproj_path, label}] — the disks
  let loadedSlot  = -1;    // index of the currently-loaded disk (-1 = none)
  let modelLoaded = false;
  let standalone  = true;  // classic single-model mode: slots 2+ greyed out
  let chatViaServer = false; // chat routes through :8080 (one brain, both hemispheres)

  // ── Adaptive per-bubble identity ─────────────────────────────────────────
  // Layer name = slot POSITION (slot 1 → cd1, slot 2 → cd2, …) — the CHATBOX's
  // own labels, so a standalone user sees "cd2 · gemma" (not SOC's A6 noise).
  // Model name = first word of whatever file is loaded RIGHT NOW (or the slot's
  // label if one was typed). Nothing is hardcoded to any model — drop a new
  // distill into a slot and every bubble picks up its name automatically.
  function firstWord(p) {
    const stem = baseName(p).replace(/\.gguf$/i, '');
    const w = stem.split(/[-_.\s]/).filter(Boolean)[0];
    return w || stem || 'model';
  }

  function attributionFor(path) {
    const i = slotOfPath(path);
    const disk = i >= 0 ? magazine[i] : null;
    const name = (disk && disk.label) ? disk.label : firstWord(path);
    return i >= 0 ? ('cd' + (i + 1) + ' · ' + name) : name;
  }

  function openTray()  { trayEl.style.display = 'flex'; refreshFromSettings(); }
  function closeTray() { trayEl.style.display = 'none'; }
  window.ModelTray = { openTray, closeTray, attributionFor };

  btnToggle.addEventListener('click', () => {
    trayEl.style.display === 'none' ? openTray() : closeTray();
  });

  // Standalone toggle: classic single-model behavior; persists in settings.
  const chkStandalone = document.getElementById('chk-standalone');
  if (chkStandalone) {
    chkStandalone.addEventListener('change', () => {
      standalone = chkStandalone.checked;
      invoke('cmd_update_settings', { standaloneMode: standalone }).catch(() => {});
      renderSlots();
    });
  }

  // Chat-via-server toggle: this window's chat goes through :8080, so the
  // CD-changer swap changes the chat's brain too (not just the HTTP layer).
  const chkViaServer = document.getElementById('chk-via-server');
  if (chkViaServer) {
    chkViaServer.addEventListener('change', () => {
      chatViaServer = chkViaServer.checked;
      invoke('cmd_update_settings', { chatViaServer }).catch(() => {});
      renderSlots();
    });
  }

  function baseName(p) { return (p || '').replace(/\\/g, '/').split('/').pop(); }
  function normPath(p) { return (p || '').replace(/\\/g, '/').toLowerCase(); }

  function normalizeMagazine(m) {
    const mag = Array.isArray(m) ? m.slice(0, MAG_SIZE) : [];
    while (mag.length < MAG_SIZE) mag.push({ model_path: '', mmproj_path: '', label: '' });
    return mag.map(d => ({
      model_path:  (d && d.model_path)  || '',
      mmproj_path: (d && d.mmproj_path) || '',
      label:       (d && d.label)       || ''
    }));
  }

  function persist() {
    invoke('cmd_update_settings', { magazine }).catch(() => {});
  }

  function slotOfPath(path) {
    const n = normPath(path);
    return magazine.findIndex(d => d.model_path && normPath(d.model_path) === n);
  }

  function applyLoadedMeta(meta) {
    modelLabel.textContent = baseName(meta.path) + '  ·  ctx ' + meta.context_length + '  ·  ' + meta.gpu_layers + ' GPU layers';
    indicator.className = 'model-indicator on';
    indicator.title = 'Model loaded';
    warningEl.classList.add('hidden');
    modelLoaded = true;
    btnEject.disabled = false;
    loadedSlot = slotOfPath(meta.path);
  }

  // ── Slot rendering ─────────────────────────────────────────────────────────
  function renderSlots() {
    slotsEl.innerHTML = '';
    magazine.forEach((disk, i) => slotsEl.appendChild(buildSlot(disk, i)));
    const chk = document.getElementById('chk-standalone');
    if (chk) chk.checked = standalone;
    const chkVs = document.getElementById('chk-via-server');
    if (chkVs) chkVs.checked = chatViaServer;
    const hint = document.getElementById('mag-hint');
    if (hint) hint.textContent = standalone
      ? 'single model · classic mode (slots 2–3 off)'
      : (chatViaServer
          ? 'load up to 3 models · chat + agents share the server brain'
          : 'load up to 3 models · SOC swaps between them');
  }

  function buildSlot(disk, i) {
    // Standalone (classic single-model) mode: only MODEL 1 is usable — no
    // CD-changer orchestrator means multi-disk loading would just mislead.
    const off = standalone && i > 0;
    const el = document.createElement('div');
    el.className = 'mag-slot' + (i === loadedSlot ? ' loaded' : '')
                              + (off ? ' mag-off' : '');

    const head = document.createElement('div');
    head.className = 'mag-slot-head';
    head.innerHTML =
      '<span class="mag-dot">●</span>' +
      '<span class="mag-slot-name">MODEL ' + (i + 1) + '</span>' +
      '<span class="mag-slot-status">' + (i === loadedSlot ? '● loaded' : (disk.model_path ? 'ready' : 'empty')) + '</span>';
    el.appendChild(head);

    // Model file row
    const q1 = document.createElement('div');
    q1.className = 'mag-q';
    q1.textContent = 'Select the model here →';
    el.appendChild(q1);
    const row1 = document.createElement('div');
    row1.className = 'mag-row';
    const inp1 = document.createElement('input');
    inp1.readOnly = true;
    inp1.placeholder = 'no model selected';
    inp1.value = disk.model_path;
    inp1.title = disk.model_path;
    const b1 = document.createElement('button');
    b1.className = 'mag-browse';
    b1.textContent = 'Browse';
    b1.addEventListener('click', async () => {
      try {
        const p = await invoke('cmd_browse_gguf_file', { title: 'Select Model for MODEL ' + (i + 1) + ' (.gguf)' });
        magazine[i].model_path = p;
        persist(); renderSlots();
      } catch (e) { if (e !== 'cancelled') statusEl.textContent = 'Browse error: ' + e; }
    });
    row1.appendChild(inp1); row1.appendChild(b1);
    el.appendChild(row1);

    // mmproj row
    const q2 = document.createElement('div');
    q2.className = 'mag-q';
    q2.textContent = 'Does this model have an mmproj? If so, select it here →';
    el.appendChild(q2);
    const row2 = document.createElement('div');
    row2.className = 'mag-row';
    const inp2 = document.createElement('input');
    inp2.readOnly = true;
    inp2.placeholder = '(none — text-only model)';
    inp2.value = disk.mmproj_path;
    inp2.title = disk.mmproj_path;
    const b2 = document.createElement('button');
    b2.className = 'mag-browse';
    b2.textContent = 'Browse';
    b2.addEventListener('click', async () => {
      try {
        const p = await invoke('cmd_browse_gguf_file', { title: 'Select mmproj for MODEL ' + (i + 1) + ' (.gguf)' });
        magazine[i].mmproj_path = p;
        persist(); renderSlots();
      } catch (e) { if (e !== 'cancelled') statusEl.textContent = 'Browse error: ' + e; }
    });
    const b2c = document.createElement('button');
    b2c.className = 'mag-clear';
    b2c.title = 'Clear mmproj';
    b2c.textContent = '✕';
    b2c.addEventListener('click', () => {
      magazine[i].mmproj_path = '';
      persist(); renderSlots();
    });
    row2.appendChild(inp2); row2.appendChild(b2); row2.appendChild(b2c);
    el.appendChild(row2);

    // Load row
    const q3 = document.createElement('div');
    q3.className = 'mag-q';
    q3.textContent = 'When model + mmproj (if any) are selected, click here →';
    el.appendChild(q3);
    const row3 = document.createElement('div');
    row3.className = 'mag-load-row';
    const bl = document.createElement('button');
    bl.className = 'mag-load-btn';
    bl.textContent = '▶ Load Model';
    bl.disabled = !disk.model_path;
    bl.addEventListener('click', () => loadSlot(i));
    row3.appendChild(bl);
    el.appendChild(row3);

    if (off) {
      el.querySelectorAll('button, input').forEach(c => { c.disabled = true; });
    }
    return el;
  }

  // ── Load / eject ──────────────────────────────────────────────────────────
  async function loadSlot(i) {
    const disk = magazine[i];
    if (!disk || !disk.model_path) return;
    statusEl.textContent = 'Loading MODEL ' + (i + 1) + '…';
    trayEl.classList.add('loading');
    try {
      // Persist this disk's mmproj as the main mmproj BEFORE loading, so the
      // server / use-main-for-vision attach the right projector for this disk.
      await invoke('cmd_update_settings', { mainMmprojPath: disk.mmproj_path || '' }).catch(() => {});
      if (chatViaServer) {
        // Server mode: a slot load IS a CD-changer swap — same code path as
        // SOC's :8086 POST /swap. No GUI-side model is loaded at all.
        const already = await invoke('cmd_swap_server', {
          modelPath: disk.model_path,
          mmprojPath: disk.mmproj_path || null,
        });
        if (!already) await waitServerRunning(i);
        applyServerLoaded(disk, i);
        statusEl.textContent = '';
        invoke('cmd_update_settings', { lastModelPath: disk.model_path }).catch(() => {});
      } else {
        const meta = await invoke('cmd_load_model', { path: disk.model_path });
        applyLoadedMeta(meta);
        statusEl.textContent = '';
        invoke('cmd_update_settings', { lastModelPath: meta.path, lastGpuLayers: meta.gpu_layers }).catch(() => {});
        if (window.HardwareSlot) window.HardwareSlot.onModelLoaded(meta);
      }
      renderSlots();
      closeTray();
    } catch (e) {
      statusEl.textContent = 'Load failed: ' + e;
    } finally {
      trayEl.classList.remove('loading');
    }
  }

  // Patient-wait for a server swap: big FP models take minutes to load — show
  // live elapsed status so it never looks stalled (graceful-wait rule).
  async function waitServerRunning(i) {
    const start = Date.now();
    await new Promise(r => setTimeout(r, 2000)); // let the old server die first
    while (Date.now() - start < 300000) {
      try {
        const st = await invoke('cmd_server_status');
        if (st && st.status === 'running') return;
        statusEl.textContent = 'MODEL ' + (i + 1) + ' loading on server… '
          + Math.round((Date.now() - start) / 1000) + 's (' + (st ? st.status : '?') + ')';
      } catch (_) {}
      await new Promise(r => setTimeout(r, 2000));
    }
    throw 'server did not reach running within 300s';
  }

  function applyServerLoaded(disk, i) {
    modelLabel.textContent = baseName(disk.model_path) + '  ·  via server :8080';
    indicator.className = 'model-indicator on';
    indicator.title = 'Serving via :8080';
    warningEl.classList.add('hidden');
    modelLoaded = true;
    btnEject.disabled = false;
    loadedSlot = i;
  }

  btnEject.addEventListener('click', async () => {
    // Server mode: the loaded disk lives in llama-server, so an honest eject
    // stops the server (otherwise the header would say "no model" while the
    // server keeps answering agents with the old disk).
    if (chatViaServer) { try { await invoke('cmd_stop_server'); } catch (_) {} }
    try { await invoke('cmd_unload_model'); } catch (_) {}
    document.getElementById('messages').innerHTML = '';
    indicator.className = 'model-indicator off';
    indicator.title = 'No model loaded';
    modelLabel.textContent = 'No model loaded';
    warningEl.classList.remove('hidden');
    modelLoaded = false;
    btnEject.disabled = true;
    loadedSlot = -1;
    statusEl.textContent = 'Model ejected.';
    if (window.HardwareSlot) window.HardwareSlot.onModelEjected();
    invoke('cmd_update_settings', { lastModelPath: '' }).catch(() => {});
    renderSlots();
  });

  // ── Settings sync + startup restore ───────────────────────────────────────
  function seedFromLegacy(s) {
    // Migration: if the magazine is empty but a single model was configured the
    // old way, seed MODEL 1 with it so the current setup appears automatically.
    if (!magazine.some(d => d.model_path) && s.last_model_path) {
      magazine[0] = { model_path: s.last_model_path, mmproj_path: s.main_mmproj_path || '', label: '' };
      persist();
    }
  }

  function refreshFromSettings() {
    invoke('cmd_get_settings').then(s => {
      magazine = normalizeMagazine(s.magazine);
      standalone = (s.standalone_mode !== false);
      chatViaServer = !!s.chat_via_server;
      seedFromLegacy(s);
      renderSlots();
    }).catch(() => {});
  }

  invoke('cmd_get_settings').then(s => {
    magazine = normalizeMagazine(s.magazine);
    standalone = (s.standalone_mode !== false);
    chatViaServer = !!s.chat_via_server;
    seedFromLegacy(s);
    renderSlots();

    // Chat-via-server mode: the server (not the GUI) owns the loaded model, so
    // skip the GUI auto-restore — start the server stack instead if a last
    // disk is known, and reflect it in the header once it's up.
    if (chatViaServer) {
      if (s.last_model_path) {
        statusEl.textContent = 'Starting server with last disk…';
        invoke('cmd_swap_server', {
          modelPath: s.last_model_path,
          mmprojPath: s.main_mmproj_path || null,
        }).then(async () => {
          const i = Math.max(0, slotOfPath(s.last_model_path));
          try { await waitServerRunning(i); } catch (_) {}
          applyServerLoaded({ model_path: s.last_model_path }, slotOfPath(s.last_model_path));
          statusEl.textContent = '';
          renderSlots();
        }).catch(err => {
          statusEl.textContent = 'Server start failed: ' + err;
        });
      }
      return;
    }

    // Auto-restore the last model on startup (same behavior as before).
    if (s.last_model_path) {
      const savedGpu = (s.last_gpu_layers != null) ? s.last_gpu_layers : null;
      statusEl.textContent = 'Restoring last model…';
      invoke('cmd_load_model', { path: s.last_model_path }).then(meta => {
        if (savedGpu != null && savedGpu !== meta.gpu_layers) {
          invoke('cmd_reload_gpu_layers', { gpuLayers: savedGpu }).then(meta2 => {
            applyLoadedMeta(meta2);
            statusEl.textContent = '';
            if (window.HardwareSlot) window.HardwareSlot.onModelRestored(meta2);
            renderSlots();
          }).catch(() => {
            applyLoadedMeta(meta);
            statusEl.textContent = '';
            if (window.HardwareSlot) window.HardwareSlot.onModelRestored(meta);
            renderSlots();
          });
        } else {
          applyLoadedMeta(meta);
          statusEl.textContent = '';
          if (window.HardwareSlot) window.HardwareSlot.onModelRestored(meta);
          renderSlots();
        }
      }).catch(err => {
        statusEl.textContent = 'Auto-load failed: ' + err;
      });
    }
  }).catch(() => {});
})();

// ── Chat input / buttons ──────────────────────────────────────────────────

(function () {
  const input   = document.getElementById('chat-input');
  const sendBtn = document.getElementById('btn-send');
  const stopBtn = document.getElementById('btn-stop');

  sendBtn.addEventListener('click', () => {
    ChatEngine.sendMessage(input.value);
  });

  input.addEventListener('keydown', e => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      ChatEngine.sendMessage(input.value);
    }
  });

  stopBtn.addEventListener('click', () => {
    ChatEngine.stopGeneration();
  });

  // ↺ New Chat — clear history (backend + pane) without touching the model.
  const newChatBtn = document.getElementById('btn-new-chat');
  if (newChatBtn) {
    newChatBtn.addEventListener('click', async () => {
      try { await invoke('cmd_stop_generation'); } catch (_) {}
      try { await invoke('cmd_clear_chat'); } catch (_) {}
      document.getElementById('messages').innerHTML = '';
    });
  }

  // Advanced Settings button — opens standalone window. It now lives INSIDE the
  // ⚙ Settings tray (menus consolidated; the ⚙+ top-bar button was removed in
  // the accidental-close reorg).
  const advBtn = document.getElementById('btn-open-advanced');
  if (advBtn) {
    advBtn.addEventListener('click', () => {
      invoke('cmd_show_advanced_settings').catch(() => {});
    });
  }

  // Window control buttons (custom titlebar)
  const btnMinimize = document.getElementById('btn-win-minimize');
  const btnClose = document.getElementById('btn-win-close');
  if (btnMinimize && window.__TAURI__) {
    btnMinimize.addEventListener('click', async () => {
      const win = window.__TAURI__.window.getCurrentWindow();
      await win.minimize();
    });
  }
  if (btnClose && window.__TAURI__) {
    btnClose.addEventListener('click', async () => {
      // Kill every child server BEFORE exiting — a bare process.exit orphans
      // llama-server children, which then squat on ports 8081/8082/8083 and
      // VRAM invisibly (the recurring "zombie server" plague). Best-effort,
      // fast: each stop is a kill+wait on an already-tracked child.
      for (const cmd of ['cmd_stop_vision_server', 'cmd_stop_server',
                         'cmd_stop_listening_server']) {
        try { await invoke(cmd); } catch (_) {}
      }
      // Use process exit to guarantee app closes
      if (window.__TAURI__.process) {
        await window.__TAURI__.process.exit(0);
      } else {
        const win = window.__TAURI__.window.getCurrentWindow();
        await win.destroy();
      }
    });
  }
})();

// ── Expansion Slot 1: Context Echo ────────────────────────────────────────

(function () {
  function buildEchoControlsHTML() {
    return `
      <div class="echo-ctrl">
        <label class="echo-toggle-row">
          <input type="checkbox" id="echo-enabled" checked />
          <span>Enable Context Echo</span>
        </label>
        <hr class="echo-sep" />
        <div class="echo-section-label">Fire at token milestones:</div>
        <label class="echo-ms-row">
          <input type="checkbox" class="echo-ms" data-pct="0.25" checked /> 25%
        </label>
        <label class="echo-ms-row">
          <input type="checkbox" class="echo-ms" data-pct="0.5" checked /> 50%
        </label>
        <label class="echo-ms-row">
          <input type="checkbox" class="echo-ms" data-pct="0.75" checked /> 75%
        </label>
        <hr class="echo-sep" />
        <div class="echo-section-label">System prompt (used in echo):</div>
        <textarea id="echo-system-prompt" rows="4" placeholder="Optional system context to echo back…"></textarea>
        <button id="echo-save-prompt">Save prompt</button>
        <hr class="echo-sep" />
        <div class="echo-section-label">Status:</div>
        <div id="echo-status" class="echo-status">Context Echo is active.</div>
      </div>`;
  }

  function wireEchoEvents(div) {
    // Load current state from backend
    invoke('cmd_get_echo_state').then(s => {
      const enabledBox = div.querySelector('#echo-enabled');
      if (enabledBox) enabledBox.checked = s.enabled;
      const promptEl = div.querySelector('#echo-system-prompt');
      if (promptEl) promptEl.value = s.system_prompt || '';
      // Sync milestone checkboxes
      div.querySelectorAll('.echo-ms').forEach(cb => {
        const pct = parseFloat(cb.dataset.pct);
        cb.checked = s.milestones.includes(pct);
      });
    }).catch(() => {});

    // Toggle enable/disable
    const enabledBox = div.querySelector('#echo-enabled');
    if (enabledBox) {
      enabledBox.addEventListener('change', () => {
        invoke('cmd_set_echo_enabled', { enabled: enabledBox.checked }).catch(() => {});
        const status = div.querySelector('#echo-status');
        if (status) status.textContent = enabledBox.checked ? 'Context Echo is active.' : 'Context Echo is disabled.';
      });
    }

    // Milestone checkboxes
    div.querySelectorAll('.echo-ms').forEach(cb => {
      cb.addEventListener('change', () => {
        const selected = Array.from(div.querySelectorAll('.echo-ms'))
          .filter(c => c.checked)
          .map(c => parseFloat(c.dataset.pct));
        invoke('cmd_set_echo_config', { milestones: selected, maxEchoPct: 0.01 }).catch(() => {});
      });
    });

    // Save system prompt
    const saveBtn = div.querySelector('#echo-save-prompt');
    const promptEl = div.querySelector('#echo-system-prompt');
    if (saveBtn && promptEl) {
      saveBtn.addEventListener('click', () => {
        invoke('cmd_set_system_prompt', { prompt: promptEl.value }).then(() => {
          const status = div.querySelector('#echo-status');
          if (status) status.textContent = 'System prompt saved.';
          setTimeout(() => { if (status) status.textContent = 'Context Echo is active.'; }, 2000);
        }).catch(() => {});
      });
    }
  }

  ExpansionSlots.register(1, {
    label: 'Context Echo',
    icon: '\uD83D\uDCE1',
    init(div) {
      div.innerHTML = buildEchoControlsHTML();
      wireEchoEvents(div);
    },
    destroy() {},
  });
})();

// ── Expansion Slot 2: Model Info ──────────────────────────────────────────
(function () {
  let _unlisten = null;

  function buildStatsHTML() {
    return `<div class="model-stat-grid">
  <span class="model-stat-label">Tokens</span><span class="model-stat-value" id="ms-tokens">—</span>
  <span class="model-stat-label">Speed</span><span class="model-stat-value" id="ms-toks">—</span>
  <span class="model-stat-label">VRAM</span><span class="model-stat-value" id="ms-vram">—</span>
  <span class="model-stat-label">Temp</span><span class="model-stat-value" id="ms-temp">—</span>
</div>`;
  }

  function applyStats(s) {
    const el = (id) => document.getElementById(id);
    if (!el('ms-tokens')) return;
    el('ms-tokens').textContent = s.ctx_size
      ? `${s.tokens_used} / ${s.ctx_size}`
      : `${s.tokens_used}`;
    el('ms-toks').textContent = s.done ? '—' : `${s.tok_s.toFixed(1)} tok/s`;
    el('ms-vram').textContent = `${s.vram_mb} MB`;
    el('ms-temp').textContent = s.temperature.toFixed(2);
  }

  ExpansionSlots.register(2, {
    label: 'Model Info',
    icon: '📊',
    async init(div) {
      div.innerHTML = buildStatsHTML();
      try {
        const stats = await invoke('cmd_get_model_stats');
        applyStats(stats);
      } catch (_) {}
      _unlisten = await listen('model-stats', (event) => applyStats(event.payload));
    },
    destroy() {
      if (_unlisten) { _unlisten(); _unlisten = null; }
    },
  });
})();

// ── CWL4 sidebar engine ───────────────────────────────────────────────────

window.CWL4 = (function () {
  let openPanel = null;
  let msgCount = 0;

  const recalib = { text: '', cadence: 1, onSpillway: false };
  // spillways[0] = Spillway 1, spillways[1] = Spillway 2, spillways[2] = Spillway 3
  const spillways = [
    { text: '', triggerWord: '', fired: false },
    { text: '', triggerWord: '', fired: false },
    { text: '', triggerWord: '', fired: false },
  ];

  // ── Panel open / close ──────────────────────────────────────────────────

  function openPanelFn(n) {
    if (openPanel !== null && openPanel !== n) closePanelFn(openPanel);
    if (openPanel === n) { closePanelFn(n); return; }
    const p = document.getElementById('cwl4-panel-' + n);
    if (!p) return;
    p.style.display = 'flex';
    const btn = document.querySelector('.cwl4-btn[data-cwl="' + n + '"]');
    if (btn) btn.classList.add('active');
    openPanel = n;
    const contentDiv = document.getElementById('cwl4-slot-' + n + '-content');
    if (contentDiv && !contentDiv.dataset.init) {
      if (n === 1) initRecalib(contentDiv);
      else initSpillway(n - 2, contentDiv); // panel 2→idx 0, panel 3→idx 1, panel 4→idx 2
      contentDiv.dataset.init = '1';
    }
  }

  function closePanelFn(n) {
    const p = document.getElementById('cwl4-panel-' + n);
    if (p) p.style.display = 'none';
    const btn = document.querySelector('.cwl4-btn[data-cwl="' + n + '"]');
    if (btn) btn.classList.remove('active');
    if (openPanel === n) openPanel = null;
  }

  // Wire sidebar buttons and close buttons
  document.querySelectorAll('.cwl4-btn').forEach(btn => {
    btn.addEventListener('click', () => openPanelFn(Number(btn.dataset.cwl)));
  });
  document.querySelectorAll('.cwl4-panel-close').forEach(btn => {
    btn.addEventListener('click', () => closePanelFn(Number(btn.dataset.cwl)));
  });

  // ── Recalibration panel init ────────────────────────────────────────────

  function initRecalib(div) {
    div.innerHTML = `<div class="cwl4-ctrl">
      <div class="cwl4-section-label">Behavioral prompt:</div>
      <textarea id="cwl4-recalib-text" rows="5" placeholder="Enter recalibration message\u2026"></textarea>
      <div class="cwl4-section-label">Fire every N messages:</div>
      <div class="cwl4-cadence-row">
        <button class="cwl4-cadence-btn selected" data-cadence="1">1</button>
        <button class="cwl4-cadence-btn" data-cadence="2">2</button>
        <button class="cwl4-cadence-btn" data-cadence="3">3</button>
        <button class="cwl4-cadence-btn" data-cadence="5">5</button>
      </div>
      <hr class="cwl4-sep" />
      <label class="cwl4-toggle-row">
        <input type="checkbox" id="cwl4-recalib-on-spillway" />
        <span>Also fire on any spillway</span>
      </label>
      <hr class="cwl4-sep" />
      <div id="cwl4-recalib-status" style="font-size:12px;color:var(--text-dim);">Ready.</div>
    </div>`;

    const textEl     = div.querySelector('#cwl4-recalib-text');
    const onSpillEl  = div.querySelector('#cwl4-recalib-on-spillway');
    const statusEl   = div.querySelector('#cwl4-recalib-status');

    textEl.value = recalib.text;
    onSpillEl.checked = recalib.onSpillway;

    textEl.addEventListener('input', () => { recalib.text = textEl.value; });
    onSpillEl.addEventListener('change', () => { recalib.onSpillway = onSpillEl.checked; });

    div.querySelectorAll('.cwl4-cadence-btn').forEach(btn => {
      if (Number(btn.dataset.cadence) === recalib.cadence) btn.classList.add('selected');
      else btn.classList.remove('selected');
      btn.addEventListener('click', () => {
        div.querySelectorAll('.cwl4-cadence-btn').forEach(b => b.classList.remove('selected'));
        btn.classList.add('selected');
        recalib.cadence = Number(btn.dataset.cadence);
        if (statusEl) statusEl.textContent = 'Cadence set to every ' + recalib.cadence + ' message(s).';
      });
    });
  }

  // ── Spillway panel init ─────────────────────────────────────────────────

  function isGateOpen(idx) {
    return idx === 0 || spillways[idx - 1].fired;
  }

  function initSpillway(idx, div) {
    const n = idx + 1;
    const gateOpen = isGateOpen(idx);
    div.innerHTML = `<div class="cwl4-ctrl">
      <div class="cwl4-gate-status ${gateOpen ? 'ready' : 'locked'}" id="cwl4-gate-${idx}">
        ${gateOpen ? 'Ready' : 'Locked &mdash; Spillway ' + idx + ' not yet fired'}
      </div>
      <hr class="cwl4-sep" />
      <div class="cwl4-section-label">Message to inject:</div>
      <textarea id="cwl4-spill-text-${idx}" rows="5" placeholder="Spillway ${n} message\u2026"></textarea>
      <button id="cwl4-spill-send-${idx}">&#x1F6AA; Send as next message</button>
      <hr class="cwl4-sep" />
      <div class="cwl4-section-label">Auto-trigger word (AI response):</div>
      <input type="text" id="cwl4-spill-trigger-${idx}" placeholder="Leave blank to disable" />
    </div>`;

    const textEl    = div.querySelector('#cwl4-spill-text-' + idx);
    const sendBtn   = div.querySelector('#cwl4-spill-send-' + idx);
    const triggerEl = div.querySelector('#cwl4-spill-trigger-' + idx);

    textEl.value    = spillways[idx].text;
    triggerEl.value = spillways[idx].triggerWord;

    textEl.addEventListener('input',    () => { spillways[idx].text        = textEl.value; });
    triggerEl.addEventListener('input', () => { spillways[idx].triggerWord = triggerEl.value.trim(); });
    sendBtn.addEventListener('click',   () => { fireSpillway(idx); });
  }

  function refreshGateStatuses() {
    spillways.forEach((_s, idx) => {
      const el = document.getElementById('cwl4-gate-' + idx);
      if (!el) return;
      if (isGateOpen(idx)) {
        el.className = 'cwl4-gate-status ready';
        el.textContent = 'Ready';
      } else {
        el.className = 'cwl4-gate-status locked';
        el.textContent = 'Locked \u2014 Spillway ' + idx + ' not yet fired';
      }
    });
  }

  // ── Recalibration fire ──────────────────────────────────────────────────

  function fireRecalib() {
    if (!recalib.text.trim()) return;
    // Read current system prompt from backend, then prepend recalib text.
    invoke('cmd_get_echo_state').then(s => {
      const base     = (s && s.system_prompt) ? s.system_prompt : '';
      const combined = recalib.text.trim() + (base ? '\n' + base : '');
      return invoke('cmd_set_system_prompt', { prompt: combined });
    }).catch(() => {});
    const statusEl = document.getElementById('cwl4-recalib-status');
    if (statusEl) statusEl.textContent = 'Fired at message ' + msgCount + '.';
  }

  // ── Spillway fire ───────────────────────────────────────────────────────

  function escHtml(s) {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  function fireSpillway(idx) {
    const s = spillways[idx];
    if (!s.text.trim()) return;
    if (!isGateOpen(idx)) {
      refreshGateStatuses();
      return;
    }
    // Render a visible spillway indicator in the chat message area.
    const messagesDiv = document.getElementById('messages');
    if (messagesDiv) {
      const indicator = document.createElement('div');
      indicator.className = 'cwl4-spill-indicator';
      indicator.textContent = '\uD83D\uDEAA Spillway ' + (idx + 1) + ' fired';
      messagesDiv.appendChild(indicator);
      messagesDiv.scrollTop = messagesDiv.scrollHeight;
    }
    // Send the spillway text through normal chat flow (user message → AI).
    ChatEngine.sendMessage(s.text);
    s.fired = true;
    // If recalib "on spillway" toggle is active, fire recalibration too.
    if (recalib.onSpillway) fireRecalib();
    refreshGateStatuses();
  }

  // ── Message hooks ───────────────────────────────────────────────────────

  ChatEngine.onMessage(function (msg) {
    if (msg.role === 'user') {
      msgCount++;
      if (recalib.text.trim() && recalib.cadence > 0 && msgCount % recalib.cadence === 0) {
        fireRecalib();
      }
    }
    if (msg.role === 'assistant') {
      const lower = msg.content.toLowerCase();
      spillways.forEach((_s, idx) => {
        const tw = spillways[idx].triggerWord;
        if (tw && lower.includes(tw.toLowerCase())) {
          // Defer by one tick to let generating=false settle.
          setTimeout(() => fireSpillway(idx), 100);
        }
      });
    }
  });

  return {};
})();

// ── Server tray engine ────────────────────────────────────────────────────

(function () {
  const trayEl            = document.getElementById('server-tray');
  const btnToggle         = document.getElementById('btn-server-tray');
  const btnClose          = document.getElementById('btn-server-tray-close');
  const beaconEl          = document.getElementById('server-beacon');
  const statusText        = document.getElementById('srv-status-text');
  const modelNameEl       = document.getElementById('srv-model-name');
  const btnStart          = document.getElementById('btn-srv-start');
  const btnStop           = document.getElementById('btn-srv-stop');
  const btnCopyEp         = document.getElementById('btn-srv-copy-endpoint');
  const wsPathInput       = document.getElementById('srv-workspace-path');
  const btnWriteCfg       = document.getElementById('btn-srv-write-config');
  const cfgStatus         = document.getElementById('srv-config-status');
  let pollTimer = null;

  // Open/close only toggle VISIBILITY. Status polling runs ALWAYS (started at
  // init below) so the beacon — which lives on the tray BUTTON, visible even when
  // the panel is closed — goes green the moment a model finishes loading, without
  // the operator having to open the panel first (the old code only polled while
  // the tray was open, so the beacon stayed stale until opened).
  function openTray()  { trayEl.style.display = 'flex'; pollStatus(); }
  function closeTray() { trayEl.style.display = 'none'; }

  btnToggle.addEventListener('click', () => {
    trayEl.style.display === 'none' ? openTray() : closeTray();
  });
  btnClose.addEventListener('click', closeTray);
  startPolling();   // always-on background poll → live beacon

  function applyStatus(status, modelName) {
    const s = status || 'stopped';
    const beaconClass = {
      running: 'beacon-running',
      starting: 'beacon-starting',
      stopped: 'beacon-stopped',
      crashed: 'beacon-crashed',
      down: 'beacon-starting',
    }[s] || 'beacon-stopped';

    beaconEl.className = 'server-beacon ' + beaconClass;
    statusText.className = 'srv-status-text ' + s;
    statusText.textContent = s.charAt(0).toUpperCase() + s.slice(1);
    if (modelNameEl && modelName) modelNameEl.textContent = modelName || '—';

    const isRunning = s === 'running';
    btnStart.disabled = isRunning || s === 'starting';
    btnStop.disabled  = !isRunning && s !== 'starting';
  }

  async function pollStatus() {
    try {
      const s = await invoke('cmd_server_status');
      applyStatus(s.status, s.model_name);
    } catch (_) {
      applyStatus('stopped', '');
    }
  }

  function startPolling() {
    pollStatus();
    if (!pollTimer) pollTimer = setInterval(pollStatus, 2000);
  }
  function stopPolling() {
    if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
  }

  btnStart.addEventListener('click', async () => {
    btnStart.disabled = true;
    applyStatus('starting', '');
    try {
      await invoke('cmd_start_server');
    } catch (e) {
      cfgStatus.textContent = 'Start failed: ' + e;
    }
    pollStatus();
  });

  btnStop.addEventListener('click', async () => {
    btnStop.disabled = true;
    try {
      await invoke('cmd_stop_server');
    } catch (e) {
      cfgStatus.textContent = 'Stop failed: ' + e;
    }
    pollStatus();
  });

  btnCopyEp.addEventListener('click', () => {
    navigator.clipboard.writeText('http://127.0.0.1:8080/v1').catch(() => {});
    btnCopyEp.textContent = 'Copied!';
    setTimeout(() => { btnCopyEp.textContent = 'Copy'; }, 1500);
  });

  btnWriteCfg.addEventListener('click', async () => {
    const ws = wsPathInput.value.trim();
    if (!ws) { cfgStatus.textContent = 'Enter a workspace folder path first.'; return; }
    try {
      await invoke('cmd_write_continue_config', { workspacePath: ws });
      cfgStatus.textContent = 'Written to .continue/config.json';
      setTimeout(() => { cfgStatus.textContent = ''; }, 3000);
    } catch (e) {
      cfgStatus.textContent = 'Error: ' + e;
    }
  });

  // ── Vision Server (inside server tray) ───────────────────────────────────
  const visionModelInput  = document.getElementById('srv-vision-model');
  const mmprojInput       = document.getElementById('srv-mmproj');
  const btnBrowseVision   = document.getElementById('btn-srv-browse-vision');
  const btnBrowseMmproj   = document.getElementById('btn-srv-browse-mmproj');
  const btnVisionStart    = document.getElementById('btn-vision-start');
  const btnVisionStop     = document.getElementById('btn-vision-stop');
  const visionBeacon      = document.getElementById('srv-vision-beacon');
  const visionStatusEl    = document.getElementById('srv-vision-status');

  // Load saved paths when tray opens.
  async function loadVisionPaths() {
    try {
      const s = await invoke('cmd_get_settings');
      if (s.vision_model_path) visionModelInput.value = s.vision_model_path;
      if (s.mmproj_path)       mmprojInput.value      = s.mmproj_path;
    } catch (_) {}
  }

  btnBrowseVision.addEventListener('click', async () => {
    try {
      const path = await invoke('cmd_browse_vision_model');
      visionModelInput.value = path;
      await invoke('cmd_update_settings', { visionModelPath: path });
    } catch (e) { if (e !== 'cancelled') visionStatusEl.textContent = 'Error: ' + e; }
  });

  btnBrowseMmproj.addEventListener('click', async () => {
    try {
      const path = await invoke('cmd_browse_mmproj');
      mmprojInput.value = path;
      await invoke('cmd_update_settings', { mmprojPath: path });
    } catch (e) { if (e !== 'cancelled') visionStatusEl.textContent = 'Error: ' + e; }
  });

  function applyVisionStatus(status) {
    const beaconClass = status === 'running'  ? 'beacon-running'
                      : status === 'starting' ? 'beacon-starting'
                      : 'beacon-stopped';
    visionBeacon.className = 'server-beacon ' + beaconClass;
    visionStatusEl.textContent = status.charAt(0).toUpperCase() + status.slice(1);
    visionStatusEl.style.color = status === 'running'  ? '#4ade80'
                                : status === 'starting' ? '#facc15'
                                : 'var(--text-dim)';
    btnVisionStart.disabled = status === 'running' || status === 'starting';
    btnVisionStop.disabled  = status === 'stopped';
  }

  let visionPollTimer = null;
  async function pollVisionStatus() {
    try {
      const s = await invoke('cmd_vision_server_status');
      applyVisionStatus(s.status);
    } catch (_) { applyVisionStatus('stopped'); }
  }

  // Extend openTray / closeTray to also manage vision polling and path load.
  const _origOpen  = openTray;
  const _origClose = closeTray;
  openTray = function () {
    _origOpen();
    loadVisionPaths();
    pollVisionStatus();
    if (!visionPollTimer) visionPollTimer = setInterval(pollVisionStatus, 2000);
  };
  closeTray = function () {
    _origClose();
    if (visionPollTimer) { clearInterval(visionPollTimer); visionPollTimer = null; }
  };

  btnVisionStart.addEventListener('click', async () => {
    // Guard: disable immediately and check live status before spawning.
    btnVisionStart.disabled = true;
    try {
      const cur = await invoke('cmd_vision_server_status');
      if (cur.status === 'running' || cur.status === 'starting') {
        applyVisionStatus(cur.status);
        return;
      }
    } catch (_) {}
    applyVisionStatus('starting');
    try {
      await invoke('cmd_start_vision_server');
    } catch (e) {
      visionStatusEl.textContent = 'Error: ' + e;
      visionStatusEl.style.color = '#f87171';
    }
    pollVisionStatus();
  });

  btnVisionStop.addEventListener('click', async () => {
    try { await invoke('cmd_stop_vision_server'); } catch (_) {}
    applyVisionStatus('stopped');
  });

  applyVisionStatus('stopped');
})();

// ── Listening Server (inside server tray) ────────────────────────────────────────
(function () {
  const listenModelInput = document.getElementById('srv-listening-model');
  const btnBrowseListen  = document.getElementById('btn-srv-browse-listening');
  const btnListenStart   = document.getElementById('btn-listening-start');
  const btnListenStop    = document.getElementById('btn-listening-stop');
  const listenBeacon     = document.getElementById('srv-listening-beacon');
  const listenStatusEl   = document.getElementById('srv-listening-status');

  if (!listenModelInput) return; // guard for older HTML

  function applyListenStatus(status) {
    const cls = status === 'running'  ? 'beacon-running'
              : status === 'starting' ? 'beacon-starting'
              : 'beacon-stopped';
    listenBeacon.className = 'server-beacon ' + cls;
    listenStatusEl.textContent = status.charAt(0).toUpperCase() + status.slice(1);
    listenStatusEl.style.color = status === 'running'  ? '#4ade80'
                                : status === 'starting' ? '#facc15'
                                : 'var(--text-dim)';
    btnListenStart.disabled = status === 'running' || status === 'starting';
    btnListenStop.disabled  = status === 'stopped';
  }

  async function pollListenStatus() {
    try {
      const s = await invoke('cmd_listening_server_status');
      applyListenStatus(s.status || 'stopped');
    } catch (_) { applyListenStatus('stopped'); }
  }

  btnBrowseListen && btnBrowseListen.addEventListener('click', async () => {
    try {
      const path = await invoke('cmd_browse_listening_model');
      listenModelInput.value = path;
      await invoke('cmd_update_settings', { listeningModelPath: path });
    } catch (e) { if (e !== 'cancelled') listenStatusEl.textContent = 'Error: ' + e; }
  });

  btnListenStart && btnListenStart.addEventListener('click', async () => {
    btnListenStart.disabled = true;
    applyListenStatus('starting');
    try {
      await invoke('cmd_start_listening_server', { modelPath: listenModelInput.value });
    } catch (e) {
      listenStatusEl.textContent = 'Error: ' + e;
      listenStatusEl.style.color = '#f87171';
    }
    pollListenStatus();
  });

  btnListenStop && btnListenStop.addEventListener('click', async () => {
    try { await invoke('cmd_stop_listening_server'); } catch (_) {}
    applyListenStatus('stopped');
  });

  applyListenStatus('stopped');

  // Refresh when server tray opens (piggybacked via same pattern as vision)
  const _origOpenSrv  = window._serverTrayOpen  || null;
  window._listenPollTimer = null;
  document.getElementById('btn-server-tray') && document.getElementById('btn-server-tray').addEventListener('click', () => {
    pollListenStatus();
    if (!window._listenPollTimer)
      window._listenPollTimer = setInterval(pollListenStatus, 2000);
  });
  document.getElementById('btn-server-tray-close') && document.getElementById('btn-server-tray-close').addEventListener('click', () => {
    if (window._listenPollTimer) { clearInterval(window._listenPollTimer); window._listenPollTimer = null; }
  });
})();

// ── Settings tray engine ──────────────────────────────────────────────────

(function () {
  const trayEl      = document.getElementById('settings-tray');
  const btnToggle   = document.getElementById('btn-settings');
  const btnClose    = document.getElementById('btn-settings-close');
  const slider      = document.getElementById('stg-throttle');
  const sliderVal   = document.getElementById('stg-throttle-val');
  const sysPrompt   = document.getElementById('stg-sys-prompt');
  const btnSave     = document.getElementById('btn-stg-save');
  const statusEl    = document.getElementById('stg-status');

  function openTray() {
    trayEl.style.display = 'flex';
    loadSettings();
  }
  function closeTray() { trayEl.style.display = 'none'; }

  btnToggle.addEventListener('click', () => {
    trayEl.style.display === 'none' ? openTray() : closeTray();
  });
  btnClose.addEventListener('click', closeTray);

  slider.addEventListener('input', () => {
    sliderVal.textContent = slider.value + '%';
  });

  async function loadSettings() {
    try {
      const s = await invoke('cmd_get_settings');
      slider.value = s.throttle_pct;
      sliderVal.textContent = s.throttle_pct + '%';
      sysPrompt.value = s.default_system_prompt;
      // Load max context override
      const maxCtxEl = document.getElementById('stg-max-ctx');
      const maxCtxWarn = document.getElementById('stg-max-ctx-warn');
      if (maxCtxEl) {
        const v = s.max_context_override || 0;
        maxCtxEl.value = String(v);
        if (v >= 131072) {
          if (maxCtxWarn) { maxCtxWarn.textContent = 'Higher contexts may reduce response quality and increase memory use.'; maxCtxWarn.style.color = '#f87171'; }
        } else {
          if (maxCtxWarn) { maxCtxWarn.textContent = 'Auto chooses the best context based on model & hardware.'; maxCtxWarn.style.color = ''; }
        }
        maxCtxEl.addEventListener('change', () => {
          const sel = parseInt(maxCtxEl.value, 10) || 0;
          if (sel >= 131072) {
            if (maxCtxWarn) { maxCtxWarn.textContent = 'Higher contexts may reduce response quality and increase memory use.'; maxCtxWarn.style.color = '#f87171'; }
          } else {
            if (maxCtxWarn) { maxCtxWarn.textContent = 'Auto chooses the best context based on model & hardware.'; maxCtxWarn.style.color = ''; }
          }
        });
      }
    } catch (_) {}
  }

  btnSave.addEventListener('click', async () => {
    btnSave.disabled = true;
    try {
      const maxCtxEl = document.getElementById('stg-max-ctx');
      const maxCtxVal = maxCtxEl ? (parseInt(maxCtxEl.value, 10) || 0) : 0;
      await invoke('cmd_update_settings', {
        throttlePct: parseInt(slider.value, 10),
        defaultSystemPrompt: sysPrompt.value,
        maxContextOverride: maxCtxVal,
      });
      statusEl.textContent = 'Settings saved.';
      setTimeout(() => { statusEl.textContent = ''; }, 2500);
    } catch (e) {
      statusEl.textContent = 'Error: ' + e;
    } finally {
      btnSave.disabled = false;
    }
  });
})();

// ── Expansion Slot 3: Debug Terminal ──────────────────────────────────────

(function () {
  ExpansionSlots.register(3, {
    label: 'Debug Terminal',
    icon: '\uD83D\uDCBB',
    init(div) {
      div.innerHTML = `
        <div style="display:flex;flex-direction:column;gap:12px;font-size:13px;">
          <p style="color:var(--text-dim);">
            Opens a standalone debug terminal window for troubleshooting.
            Errors, warnings, and system events are logged here.
          </p>
          <button id="btn-show-terminal" style="align-self:flex-start;">Show Debug Terminal</button>
          <div id="term-slot-status" style="font-size:12px;color:var(--text-dim);min-height:16px;"></div>
        </div>`;
      var btn = div.querySelector('#btn-show-terminal');
      var status = div.querySelector('#term-slot-status');
      btn.addEventListener('click', function () {
        invoke('cmd_show_debug_terminal').then(function () {
          status.textContent = 'Terminal window opened.';
          setTimeout(function () { status.textContent = ''; }, 2000);
        }).catch(function (e) {
          status.textContent = 'Error: ' + e;
        });
      });
    },
    destroy() {},
  });
})();

// ── Expansion Slot 4: HF Model Fetcher ───────────────────────────────────

(function () {
  let _unlisten = null;

  function buildFetcherHTML() {
    return `<div style="display:flex;flex-direction:column;gap:10px;padding:6px;max-width:440px;">

  <div style="font-size:11px;color:var(--text-dim);line-height:1.5">
    Paste a direct link to a <code>.gguf</code> file and click <b>Download</b>.
    Optionally add the model-card link and an mmproj link — all three are tracked
    together with the model (as long as the files aren't moved).
    Partial downloads are saved as <code>.part</code> files and resumed automatically.
  </div>

  <!-- Model .gguf URL + Download button -->
  <div style="display:flex;gap:6px;align-items:center;">
    <input type="text" id="hf-url"
      placeholder="model .gguf URL  (…/resolve/main/model.gguf)"
      style="flex:1;background:var(--input-bg);color:var(--text);border:1px solid var(--border);
             border-radius:4px;padding:5px 8px;font-size:11px;min-width:0;" />
    <button id="hf-download-btn" style="white-space:nowrap;">⬇ Download</button>
  </div>

  <!-- Optional: model-card link + mmproj link (tracked with the model) -->
  <input type="text" id="hf-card-url"
    placeholder="model card link  (optional — the HF model page URL)"
    style="background:var(--input-bg);color:var(--text);border:1px solid var(--border);
           border-radius:4px;padding:5px 8px;font-size:11px;min-width:0;" />
  <input type="text" id="hf-mmproj-url"
    placeholder="mmproj .gguf URL  (optional — for vision models)"
    style="background:var(--input-bg);color:var(--text);border:1px solid var(--border);
           border-radius:4px;padding:5px 8px;font-size:11px;min-width:0;" />

  <!-- Resume / cancel row -->
  <div style="display:flex;gap:8px;align-items:center;">
    <button id="hf-resume-btn" disabled style="font-size:11px;">↻ Resume</button>
    <span id="hf-status" style="font-size:11px;color:var(--text-dim);flex:1;"></span>
  </div>

  <!-- Terminal-style log -->
  <pre id="hf-terminal"
    style="background:var(--input-bg);border:1px solid var(--border);border-radius:4px;
           padding:6px 8px;font-size:11px;line-height:1.6;white-space:pre-wrap;
           word-break:break-all;max-height:240px;overflow-y:auto;margin:0;
           font-family:monospace;color:var(--text);"></pre>

</div>`;
  }

  function wireEvents(div) {
    const urlInput  = div.querySelector('#hf-url');
    const cardInput = div.querySelector('#hf-card-url');
    const mmInput   = div.querySelector('#hf-mmproj-url');
    const dlBtn     = div.querySelector('#hf-download-btn');
    const resumeBtn = div.querySelector('#hf-resume-btn');
    const terminal  = div.querySelector('#hf-terminal');
    const statusEl  = div.querySelector('#hf-status');

    let currentUrl = null;
    let currentCard = null;
    let currentMmproj = null;
    let downloading = false;

    function appendLine(text, isError) {
      const line = document.createElement('span');
      line.style.display = 'block';
      line.style.color   = isError ? '#f87171' : 'var(--text)';
      line.textContent   = text;
      terminal.appendChild(line);
      terminal.scrollTop = terminal.scrollHeight;
    }

    function setDownloading(active) {
      downloading = active;
      dlBtn.disabled    = active;
      resumeBtn.disabled = !active; // enable Resume only when a download is in progress or failed
    }

    async function startDownload(url, cardUrl, mmprojUrl) {
      terminal.innerHTML = '';
      statusEl.textContent = '';
      currentUrl = url;
      currentCard = cardUrl || null;
      currentMmproj = mmprojUrl || null;
      setDownloading(true);
      statusEl.textContent = 'Connecting…';
      try {
        await invoke('cmd_download_hf_model', {
          url,
          cardUrl: cardUrl || null,
          mmprojUrl: mmprojUrl || null,
        });
        // done event fires through hf-download-log; nothing extra needed here
      } catch (e) {
        appendLine('✗ ' + e, true);
        statusEl.textContent = 'Failed — paste URL and click Resume to retry.';
        setDownloading(false);
        resumeBtn.disabled = false;
      }
    }

    dlBtn.addEventListener('click', () => {
      const url = urlInput.value.trim();
      if (!url) { statusEl.textContent = 'Enter a model URL first.'; return; }
      resumeBtn.disabled = true;
      startDownload(url, cardInput.value.trim(), mmInput.value.trim());
    });

    resumeBtn.addEventListener('click', () => {
      if (!currentUrl) { statusEl.textContent = 'Paste the original URL above first.'; return; }
      startDownload(currentUrl, currentCard, currentMmproj);
    });

    // Listen for streaming log lines from Rust
    listen('hf-download-log', (event) => {
      const d = event.payload;
      if (!d) return;
      appendLine(d.line, d.error);
      if (d.done) {
        setDownloading(false);
        resumeBtn.disabled = true;
        statusEl.textContent = d.error ? 'Failed — see log above.' : 'Done!';
        statusEl.style.color  = d.error ? '#f87171' : '#4ade80';
      } else {
        // Extract percent from lines like "  42% — …"
        const m = d.line.match(/^\s*(\d+)%/);
        if (m) statusEl.textContent = m[1] + '% …';
      }
    }).then(u => { _unlisten = u; }).catch(() => {});
  }

  ExpansionSlots.register(4, {
    label: 'HF Model Fetcher',
    icon: '\u2B07\uFE0F',
    init(div) {
      div.innerHTML = buildFetcherHTML();
      wireEvents(div);
    },
    destroy() {
      if (_unlisten) { _unlisten(); _unlisten = null; }
    },
  });
})();

// ── Expansion Slot 5: Hardware (CPU/GPU) ──────────────────────────────────

(function () {
  var currentMeta = null;
  var hwWarning = document.getElementById('hw-warning');
  var slot5Tab = document.querySelector('.exp-tab[data-slot="5"]');

  function buildHardwareHTML(meta) {
    var mode = (meta && meta.gpu_layers > 0) ? 'GPU (' + meta.gpu_layers + ' layers)' : 'CPU only';
    var maxLayers = (meta && meta.layers) ? meta.layers : 0;
    var curLayers = (meta && meta.gpu_layers) ? meta.gpu_layers : 0;
    return '<div style="padding:4px;">' +
      '<div style="font-size:13px;font-weight:600;color:var(--text);margin-bottom:8px;">\u2699 Hardware Configuration</div>' +
      '<div style="display:flex;align-items:center;gap:8px;margin-bottom:6px;">' +
        '<span style="font-size:12px;color:var(--text-dim);">Mode:</span>' +
        '<span id="hw-mode-label" style="font-size:12px;font-weight:600;color:' + (curLayers > 0 ? '#4ade80' : '#f87171') + ';">' + mode + '</span>' +
      '</div>' +
      '<div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">' +
        '<label style="font-size:12px;color:var(--text-dim);white-space:nowrap;">GPU Layers</label>' +
        '<input type="range" id="hw-gpu-slider" min="0" max="' + maxLayers + '" step="1" value="' + curLayers + '" style="flex:1;" />' +
        '<span id="hw-gpu-val" style="font-size:12px;color:var(--text);min-width:30px;">' + curLayers + '/' + maxLayers + '</span>' +
      '</div>' +
      '<div style="display:flex;gap:8px;align-items:center;">' +
        '<button id="hw-all-cpu" style="font-size:11px;padding:4px 10px;">All CPU</button>' +
        '<button id="hw-all-gpu" style="font-size:11px;padding:4px 10px;">All GPU</button>' +
        '<button id="hw-apply-btn" style="font-size:12px;padding:5px 14px;background:#6d28d9;color:#fff;font-weight:600;border:none;border-radius:4px;cursor:pointer;">Apply &amp; Reload</button>' +
      '</div>' +
      '<div id="hw-status" style="font-size:12px;color:var(--text-dim);min-height:16px;margin-top:6px;"></div>' +
    '</div>';
  }

  function wireEvents(div) {
    var slider = div.querySelector('#hw-gpu-slider');
    var valEl = div.querySelector('#hw-gpu-val');
    var applyBtn = div.querySelector('#hw-apply-btn');
    var statusEl = div.querySelector('#hw-status');
    var allCpu = div.querySelector('#hw-all-cpu');
    var allGpu = div.querySelector('#hw-all-gpu');
    var max = parseInt(slider.max, 10);

    slider.addEventListener('input', function () {
      valEl.textContent = slider.value + '/' + max;
    });

    allCpu.addEventListener('click', function () {
      slider.value = 0;
      valEl.textContent = '0/' + max;
    });
    allGpu.addEventListener('click', function () {
      slider.value = max;
      valEl.textContent = max + '/' + max;
    });

    applyBtn.addEventListener('click', async function () {
      applyBtn.disabled = true;
      statusEl.textContent = 'Reloading model\u2026';
      try {
        var meta = await invoke('cmd_reload_gpu_layers', { gpuLayers: parseInt(slider.value, 10) });
        currentMeta = meta;
        statusEl.style.color = '#4ade80';
        statusEl.textContent = 'Reloaded with ' + meta.gpu_layers + ' GPU layers.';
        updateWarning(meta);
        // Persist user's GPU layer choice
        invoke('cmd_update_settings', { lastGpuLayers: meta.gpu_layers }).catch(function () {});
        // Update the model label in titlebar
        var name = meta.path.replace(/\\\\/g, '/').split('/').pop();
        var label = document.getElementById('chat-model-label');
        if (label) label.textContent = name + '  \u00b7  ctx ' + meta.context_length + '  \u00b7  ' + meta.gpu_layers + ' GPU layers';
        // Refresh the tray content
        var content = document.getElementById('exp-slot-5-content');
        if (content) { content.innerHTML = buildHardwareHTML(meta); wireEvents(content); }
      } catch (e) {
        statusEl.style.color = '#f87171';
        statusEl.textContent = 'Error: ' + e;
      } finally {
        applyBtn.disabled = false;
      }
    });
  }

  function updateWarning(meta) {
    if (meta && meta.gpu_layers === 0 && meta.layers > 0) {
      hwWarning.classList.remove('hidden');
      slot5Tab.classList.add('hw-flash');
    } else {
      hwWarning.classList.add('hidden');
      slot5Tab.classList.remove('hw-flash');
    }
  }

  // Called by Model Tray after a model loads
  window.HardwareSlot = {
    onModelLoaded: function (meta) {
      currentMeta = meta;
      updateWarning(meta);
      // Refresh content if tray is open
      var content = document.getElementById('exp-slot-5-content');
      if (content && content.children.length > 0) {
        content.innerHTML = buildHardwareHTML(meta);
        wireEvents(content);
      }
    },
    // Restored from saved settings — no warning flash
    onModelRestored: function (meta) {
      currentMeta = meta;
      // Suppress warning — user already configured this
      hwWarning.classList.add('hidden');
      slot5Tab.classList.remove('hw-flash');
      var content = document.getElementById('exp-slot-5-content');
      if (content && content.children.length > 0) {
        content.innerHTML = buildHardwareHTML(meta);
        wireEvents(content);
      }
    },
    onModelEjected: function () {
      currentMeta = null;
      hwWarning.classList.add('hidden');
      slot5Tab.classList.remove('hw-flash');
    }
  };

  // Clicking the titlebar warning opens the Hardware slot
  hwWarning.addEventListener('click', function () {
    var tab = document.querySelector('.exp-tab[data-slot="5"]');
    if (tab) tab.click();
  });

  ExpansionSlots.register(5, {
    label: 'Hardware',
    icon: '\uD83D\uDD27',
    init(div) {
      if (currentMeta) {
        div.innerHTML = buildHardwareHTML(currentMeta);
        wireEvents(div);
      } else {
        div.innerHTML = '<p style="padding:8px;font-size:12px;color:var(--text-dim);">Load a model first to configure hardware.</p>';
      }
    },
    destroy() {},
  });
})();

// ── Expansion Slot 6: Configurator (launch external Python GUI) ─────────
(function () {
  ExpansionSlots.register(6, {
    label: 'Configurator',
    icon: '\u2699\uFE0F',
    init(div) {
      div.innerHTML = `<div style="padding:8px"><button id="open-configurator">Open Configurator GUI</button><div id="config-status" style="margin-top:8px;color:var(--text-dim)"></div></div>`;
      const btn = div.querySelector('#open-configurator');
      const status = div.querySelector('#config-status');
      if (btn) btn.addEventListener('click', async () => {
        try {
          status.textContent = 'Starting configurator...';
          await invoke('cmd_open_configurator');
          status.textContent = 'Configurator started.';
        } catch (e) {
          status.textContent = 'Failed to start: ' + e;
        }
      });
    },
    destroy() {},
  });
})();

// ── Expansion Slot 7: Vision Adapter ─────────────────────────────────────
(function () {
  ExpansionSlots.register(7, {
    label: 'Vision',
    icon: '🖼️',
    init(div) {
      div.innerHTML = `<div style="padding:8px;max-width:420px">
        <input id="vision-file" type="file" accept="image/*"><br/>
        <img id="vision-preview" style="max-width:100%;margin-top:8px;display:none" />
        <div id="vision-toolbar" style="margin-top:8px;display:flex;gap:8px">
          <button id="vision-annotate" disabled>Annotate</button>
          <button id="vision-ocr" disabled>OCR</button>
          <button id="vision-custom" disabled>Custom</button>
        </div>
        <div style="margin-top:8px"><button id="vision-analyze" disabled>Analyze Image</button></div>
        <div class="mm-toggle-row" style="margin-top:8px">
          <label class="mm-toggle-label">
            <input type="checkbox" id="vision-use-main-chk" />
            Use main multimodal model
          </label>
          <span id="vision-use-main-warn" class="mm-warning" style="display:none;">&#x26A0; Main model does not support vision input</span>
        </div>
        <div id="vision-status" style="margin-top:8px;color:var(--text-dim)"></div>
        <pre id="vision-result" style="margin-top:8px;white-space:pre-wrap;word-break:break-word;font-size:12px"></pre>
      </div>`;

      const fileEl = div.querySelector('#vision-file');
      const imgEl = div.querySelector('#vision-preview');
      const btn = div.querySelector('#vision-analyze');
      const btnAnnotate = div.querySelector('#vision-annotate');
      const btnOcr = div.querySelector('#vision-ocr');
      const btnCustom = div.querySelector('#vision-custom');
      const status = div.querySelector('#vision-status');
      const result = div.querySelector('#vision-result');
      const useMainChk  = div.querySelector('#vision-use-main-chk');
      const useMainWarn = div.querySelector('#vision-use-main-warn');

      // Load persisted toggle state and run capability check.
      async function refreshVisionToggleState() {
        try {
          const s = await invoke('cmd_get_settings');
          useMainChk.checked = s.use_main_for_vision ?? false;
        } catch (_) {}
        try {
          const capable = await invoke('cmd_check_main_model_has_vision');
          if (!capable) {
            useMainChk.disabled = true;
            useMainChk.checked  = false;
            useMainWarn.style.display = 'inline';
          } else {
            useMainChk.disabled = false;
            useMainWarn.style.display = 'none';
          }
        } catch (_) {
          useMainChk.disabled = true;
          useMainWarn.style.display = 'inline';
        }
      }
      refreshVisionToggleState();

      useMainChk.addEventListener('change', async () => {
        try { await invoke('cmd_update_settings', { useMainForVision: useMainChk.checked }); } catch (_) {}
      });

      fileEl.addEventListener('change', () => {
        const f = fileEl.files[0];
        if (!f) { imgEl.style.display='none'; btn.disabled = true; return; }
        const reader = new FileReader();
        reader.onload = () => {
          imgEl.src = reader.result;
          imgEl.style.display = 'block';
          btn.disabled = false;
          result.textContent = '';
        };
        reader.readAsDataURL(f);
      });

      btn.addEventListener('click', async () => {
        const f = fileEl.files[0];
        if (!f) return;
        status.textContent = 'Uploading…';
        btn.disabled = true;
        try {
          const reader = new FileReader();
          reader.onload = async () => {
            const dataUrl = reader.result;
              const resp = await invoke('cmd_process_image', { filename: f.name || 'upload.png', data_url: dataUrl });
              status.textContent = 'Saved image.';
              result.textContent = JSON.stringify(resp, null, 2);
              // Enable action buttons and wire default handlers
              if (btnAnnotate) btnAnnotate.disabled = false;
              if (btnOcr) btnOcr.disabled = false;
              if (btnCustom) btnCustom.disabled = false;
              const savedPath = resp.saved_as;
              async function doVisionAction(action, label) {
                status.textContent = 'Running ' + label + '…';
                try {
                  const useMain = useMainChk && useMainChk.checked && !useMainChk.disabled;
                  const cmd = useMain ? 'cmd_vision_action_main' : 'cmd_vision_action';
                  const r = await invoke(cmd, { action, image_path: savedPath });
                  result.textContent = JSON.stringify(r, null, 2);
                  status.textContent = label + ' complete.';
                } catch (e) { status.textContent = label + ' failed: ' + e; }
              }
              if (btnAnnotate) btnAnnotate.onclick = () => doVisionAction('annotate', 'Annotate');
              if (btnOcr)      btnOcr.onclick      = () => doVisionAction('ocr',      'OCR');
              if (btnCustom)   btnCustom.onclick   = () => doVisionAction('custom',   'Custom');
              // Show Open in BSR button
              let openBtn = div.querySelector('#vision-open-bsr');
              if (!openBtn) {
                const wrap = document.createElement('div');
                wrap.style.marginTop = '8px';
                wrap.innerHTML = '<button id="vision-open-bsr">Open in BSR</button> <span id="vision-bsr-status" style="margin-left:8px;color:var(--text-dim)"></span>';
                div.querySelector('div').appendChild(wrap);
                openBtn = div.querySelector('#vision-open-bsr');
                const bsrStatus = div.querySelector('#vision-bsr-status');
                openBtn.addEventListener('click', async () => {
                  bsrStatus.textContent = 'Launching BSR...';
                  try {
                    const launch = await invoke('cmd_launch_bsr', { image_path: resp.saved_as, bsr_path: null });
                    bsrStatus.textContent = launch;
                  } catch (e) {
                    bsrStatus.textContent = 'BSR launch failed: ' + e;
                  }
                });
              }
          };
          reader.readAsDataURL(f);
        } catch (e) {
          status.textContent = 'Error: ' + e;
        } finally {
          btn.disabled = false;
        }
      });
    },
    destroy() {},
  });
})();

// ── Expansion Slot 8: Listening — Audio Intelligence ─────────────────────
(function () {
  ExpansionSlots.register(8, {
    label: '\uD83D\uDC42)) Listening',
    icon: '\uD83D\uDC42))',
    init(div) {
      div.innerHTML = `
<div style="padding:8px;max-width:440px;display:flex;flex-direction:column;gap:10px">

  <div style="font-size:11px;color:var(--text-dim);line-height:1.5">
    Load an audio file and run genre tagging, transcription, or a full
    review via the Listening Server (port 8083).
  </div>

  <!-- File picker row -->
  <div style="display:flex;gap:6px;align-items:center">
    <input type="text" id="ls-file-path" readonly placeholder="No file selected…"
      style="flex:1;background:var(--input-bg);color:var(--text);border:1px solid var(--border);
             border-radius:4px;padding:4px 6px;font-size:11px;min-width:0;" />
    <button id="ls-browse">Browse</button>
  </div>

  <!-- Action buttons -->
  <div style="display:flex;gap:8px;flex-wrap:wrap">
    <button id="ls-gen-tags"  disabled>\uD83C\uDFF7\uFE0F Generate Tags</button>
    <button id="ls-transcribe" disabled>\uD83D\uDCDD Transcribe</button>
    <button id="ls-review"    disabled>\uD83D\uDD0D Review Audio File</button>
  </div>

  <!-- Multimodal routing toggle -->
  <div class="mm-toggle-row">
    <label class="mm-toggle-label">
      <input type="checkbox" id="ls-use-main-chk" />
      Use main multimodal model
    </label>
    <span id="ls-use-main-warn" class="mm-warning" style="display:none;">&#x26A0; Whisper not configured \u2014 ASR required</span>
  </div>

  <!-- Server status hint -->
  <div id="ls-server-hint" style="font-size:10px;color:var(--text-dim)">
    Listening Server: checking…
  </div>

  <!-- Progress / status -->
  <div id="ls-status" style="font-size:11px;color:var(--text-dim);min-height:16px"></div>

  <!-- Result -->
  <pre id="ls-result"
    style="background:var(--input-bg);border:1px solid var(--border);border-radius:4px;
           padding:6px;font-size:11px;white-space:pre-wrap;word-break:break-word;
           max-height:220px;overflow-y:auto;margin:0"></pre>

</div>`;

      const filePathEl   = div.querySelector('#ls-file-path');
      const browseBtn    = div.querySelector('#ls-browse');
      const genTagsBtn   = div.querySelector('#ls-gen-tags');
      const transBtn     = div.querySelector('#ls-transcribe');
      const reviewBtn    = div.querySelector('#ls-review');
      const serverHint   = div.querySelector('#ls-server-hint');
      const statusEl     = div.querySelector('#ls-status');
      const resultEl     = div.querySelector('#ls-result');
      const lsUseMainChk  = div.querySelector('#ls-use-main-chk');
      const lsUseMainWarn = div.querySelector('#ls-use-main-warn');
      let currentFile    = null;

      // Load persisted toggle state and check Whisper availability.
      async function refreshAudioToggleState() {
        try {
          const s = await invoke('cmd_get_settings');
          lsUseMainChk.checked = s.use_main_for_audio ?? false;
          const whisperReady = s.whisper_exe_path && s.whisper_exe_path.length > 0
                            && s.whisper_model_path && s.whisper_model_path.length > 0;
          if (!whisperReady) {
            lsUseMainChk.disabled = true;
            lsUseMainChk.checked  = false;
            lsUseMainWarn.style.display = 'inline';
          } else {
            lsUseMainChk.disabled = false;
            lsUseMainWarn.style.display = 'none';
          }
        } catch (_) {
          lsUseMainChk.disabled = true;
          lsUseMainWarn.style.display = 'inline';
        }
      }
      refreshAudioToggleState();

      lsUseMainChk.addEventListener('change', async () => {
        try { await invoke('cmd_update_settings', { useMainForAudio: lsUseMainChk.checked }); } catch (_) {}
        // Hide/show server hint based on routing mode.
        serverHint.style.display = lsUseMainChk.checked ? 'none' : '';
      });

      // Check listening server status
      async function checkServer() {
        try {
          const s = await invoke('cmd_listening_server_status');
          const up = s.status === 'running';
          serverHint.textContent = up ? '\u2022 Listening Server: Online (port 8083)'
                                      : '\u25E6 Listening Server: Offline — start it in the SERVER tray';
          serverHint.style.color = up ? '#4ade80' : 'var(--text-dim)';
          return up;
        } catch (_) {
          serverHint.textContent = '\u25E6 Listening Server: Offline';
          serverHint.style.color = 'var(--text-dim)';
          return false;
        }
      }

      function setFile(path) {
        currentFile = path;
        filePathEl.value = path;
        genTagsBtn.disabled  = false;
        transBtn.disabled    = false;
        reviewBtn.disabled   = false;
        resultEl.textContent = '';
        statusEl.textContent = '';
      }

      browseBtn.addEventListener('click', async () => {
        try {
          const path = await invoke('cmd_browse_audio_file');
          if (path) setFile(path);
        } catch (e) { if (e !== 'cancelled') statusEl.textContent = 'Browse error: ' + e; }
      });

      async function runAction(action) {
        if (!currentFile) return;
        statusEl.textContent = 'Running ' + action + '…';
        statusEl.style.color = 'var(--text-dim)';
        resultEl.textContent = '';
        genTagsBtn.disabled = transBtn.disabled = reviewBtn.disabled = true;
        try {
          const useMain = lsUseMainChk && lsUseMainChk.checked && !lsUseMainChk.disabled;
          if (useMain) {
            // Route: whisper-cli → transcript → main model (port 8080)
            const r = await invoke('cmd_audio_transcribe_then_reason', { filePath: currentFile, action });
            resultEl.textContent = typeof r === 'string' ? r : JSON.stringify(r, null, 2);
            statusEl.textContent = action + ' complete (via main model).';
          } else {
            // Existing path: dedicated Listening Server (port 8083)
            const up = await checkServer();
            if (!up) { statusEl.textContent = 'Start the Listening Server first (SERVER tray).'; return; }
            const r = await invoke('cmd_listening_action', { action, filePath: currentFile });
            resultEl.textContent = typeof r === 'string' ? r : JSON.stringify(r, null, 2);
            statusEl.textContent = action + ' complete.';
          }
        } catch (e) {
          statusEl.textContent = 'Error: ' + e;
          statusEl.style.color = '#f87171';
        } finally {
          genTagsBtn.disabled = transBtn.disabled = reviewBtn.disabled = false;
        }
      }

      genTagsBtn.addEventListener('click',  () => runAction('generate_tags'));
      transBtn.addEventListener('click',    () => runAction('transcribe'));
      reviewBtn.addEventListener('click',   () => runAction('review'));

      checkServer();
    },
    destroy() {},
  });
})();

// ── HRT auto-link (background retry) ─────────────────────────────────────

(function () {
  var hrtLinked = false;
  var hrtRetryTimer = null;

  function autoLinkHrt() {
    if (hrtLinked) return;
    if (!window.__TAURI__ || !window.__TAURI__.core) return;
    window.__TAURI__.core.invoke('cmd_link_hrt', { port: 8090 })
      .then(function () {
        hrtLinked = true;
        stopRetry();
      })
      .catch(function () {});
  }

  function startRetry() {
    if (hrtRetryTimer) return;
    hrtRetryTimer = setInterval(function () {
      if (hrtLinked) { stopRetry(); return; }
      autoLinkHrt();
    }, 30000);
  }

  function stopRetry() {
    if (hrtRetryTimer) { clearInterval(hrtRetryTimer); hrtRetryTimer = null; }
  }

  autoLinkHrt();
  startRetry();
})();

// ── TTS engine (Voicebox) ─────────────────────────────────────────────────

(function () {
  let ttsEnabled = false;
  const ttsBtn = document.getElementById('btn-tts');

  // Toggle TTS on/off
  ttsBtn.addEventListener('click', () => {
    ttsEnabled = !ttsEnabled;
    ttsBtn.classList.toggle('active', ttsEnabled);
    ttsBtn.title = ttsEnabled ? 'TTS ON — will speak responses' : 'Text to Speech (requires Voicebox)';
  });

  // Load saved TTS preference
  invoke('cmd_get_settings').then(s => {
    if (s.voice_enabled) {
      ttsEnabled = true;
      ttsBtn.classList.add('active');
    }
  }).catch(() => {});

  // When assistant finishes a message and TTS is on, speak it
  ChatEngine.onMessage(async function (msg) {
    if (msg.role !== 'assistant' || !ttsEnabled) return;
    if (!msg.content || !msg.content.trim()) return;
    try {
      const settings = await invoke('cmd_get_settings');
      const url = settings.voicebox_url || 'http://localhost:17493';
      const profileId = settings.voice_profile_id || '';
      await invoke('cmd_voicebox_speak', {
        url: url,
        text: msg.content,
        profileId: profileId,
      });
    } catch (e) {
      console.warn('TTS failed:', e);
    }
  });

  // Save TTS state when toggling
  ttsBtn.addEventListener('click', () => {
    invoke('cmd_update_settings', { voiceEnabled: ttsEnabled }).catch(() => {});
  });
})();

// ── STT engine (Whisper) ──────────────────────────────────────────────────

(function () {
  const micBtn = document.getElementById('btn-mic');
  const chatInput = document.getElementById('chat-input');
  let recording = false;
  let mediaStream = null;
  let audioContext = null;
  let processor = null;
  let recordedChunks = [];
  let silenceTimeoutSec = 5;
  let silenceStart = null;
  const SILENCE_THRESHOLD = 0.01;

  // Load silence timeout from settings
  invoke('cmd_get_settings').then(function (s) {
    if (s.whisper_silence_timeout) silenceTimeoutSec = s.whisper_silence_timeout;
  }).catch(function () {});

  micBtn.addEventListener('click', () => {
    if (recording) {
      stopRecording();
    } else {
      startRecording();
    }
  });

  async function startRecording() {
    try {
      mediaStream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      console.warn('Mic access denied:', e);
      return;
    }
    recording = true;
    micBtn.classList.add('active');
    micBtn.title = 'Recording… click to stop';
    recordedChunks = [];
    silenceStart = null;

    audioContext = new AudioContext({ sampleRate: 16000 });
    const source = audioContext.createMediaStreamSource(mediaStream);
    processor = audioContext.createScriptProcessor(4096, 1, 1);
    processor.onaudioprocess = function (e) {
      const data = e.inputBuffer.getChannelData(0);
      recordedChunks.push(new Float32Array(data));

      // Silence detection — compute RMS
      let sum = 0;
      for (let i = 0; i < data.length; i++) sum += data[i] * data[i];
      const rms = Math.sqrt(sum / data.length);
      if (rms < SILENCE_THRESHOLD) {
        if (!silenceStart) silenceStart = Date.now();
        else if (Date.now() - silenceStart >= silenceTimeoutSec * 1000) {
          stopRecording();
          return;
        }
      } else {
        silenceStart = null;
      }
    };
    source.connect(processor);
    processor.connect(audioContext.destination);
  }

  async function stopRecording() {
    recording = false;
    micBtn.classList.remove('active');
    micBtn.title = 'Voice input (requires Whisper)';

    if (processor) processor.disconnect();
    if (audioContext) audioContext.close();
    if (mediaStream) mediaStream.getTracks().forEach(t => t.stop());

    if (recordedChunks.length === 0) return;

    micBtn.classList.add('processing');
    micBtn.title = 'Transcribing…';

    // Combine chunks into one Float32Array
    const totalLen = recordedChunks.reduce((s, c) => s + c.length, 0);
    const pcm = new Float32Array(totalLen);
    let offset = 0;
    for (const chunk of recordedChunks) {
      pcm.set(chunk, offset);
      offset += chunk.length;
    }

    // Encode as 16-bit PCM WAV
    const wavBytes = encodeWav(pcm, 16000);

    try {
      const text = await invoke('cmd_transcribe_audio', { audioData: Array.from(new Uint8Array(wavBytes)) });
      if (text && text.trim()) {
        chatInput.value += (chatInput.value ? ' ' : '') + text.trim();
        chatInput.focus();
      }
    } catch (e) {
      console.warn('Transcription failed:', e);
      // Show error briefly in chat input placeholder
      chatInput.placeholder = 'STT error: ' + e;
      setTimeout(() => { chatInput.placeholder = 'Type a message…'; }, 4000);
    } finally {
      micBtn.classList.remove('processing');
      micBtn.title = 'Voice input (requires Whisper)';
    }
  }

  function encodeWav(samples, sampleRate) {
    const numChannels = 1;
    const bitsPerSample = 16;
    const byteRate = sampleRate * numChannels * bitsPerSample / 8;
    const blockAlign = numChannels * bitsPerSample / 8;
    const dataSize = samples.length * blockAlign;
    const buffer = new ArrayBuffer(44 + dataSize);
    const view = new DataView(buffer);

    // RIFF header
    writeString(view, 0, 'RIFF');
    view.setUint32(4, 36 + dataSize, true);
    writeString(view, 8, 'WAVE');
    // fmt chunk
    writeString(view, 12, 'fmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true); // PCM
    view.setUint16(22, numChannels, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, byteRate, true);
    view.setUint16(32, blockAlign, true);
    view.setUint16(34, bitsPerSample, true);
    // data chunk
    writeString(view, 36, 'data');
    view.setUint32(40, dataSize, true);

    // Convert float32 to int16
    let off = 44;
    for (let i = 0; i < samples.length; i++) {
      let s = Math.max(-1, Math.min(1, samples[i]));
      view.setInt16(off, s < 0 ? s * 0x8000 : s * 0x7FFF, true);
      off += 2;
    }
    return buffer;
  }

  function writeString(view, offset, str) {
    for (let i = 0; i < str.length; i++) {
      view.setUint8(offset + i, str.charCodeAt(i));
    }
  }
})();

// ── Expansion Slot 9: Model Card & App Profile ────────────────────────────
(function () {
  let currentCard = null;

  function renderCard(div, card, profiles, activeProfileId) {
    const p = card ? card.recommended_params : {};
    const profileOptions = profiles.map(pr =>
      `<option value="${pr.id}" ${pr.id === activeProfileId ? 'selected' : ''}>${pr.label}</option>`
    ).join('');

    div.innerHTML = `
<div style="padding:10px;max-width:480px;display:flex;flex-direction:column;gap:10px;font-size:12px">
  <div style="display:grid;grid-template-columns:auto 1fr;gap:4px 12px;color:var(--text)">
    <span style="color:var(--text-dim)">Name</span>     <span id="mc-name">${card ? card.name || '—' : '—'}</span>
    <span style="color:var(--text-dim)">Arch</span>      <span id="mc-arch">${card ? card.architecture || '—' : '—'}</span>
    <span style="color:var(--text-dim)">Author</span>    <span id="mc-author">${card ? card.author || '—' : '—'}</span>
    <span style="color:var(--text-dim)">Quant</span>     <span id="mc-quant">${card ? card.quant || '—' : '—'}</span>
    <span style="color:var(--text-dim)">Size</span>      <span id="mc-size">${card ? card.size_mb + ' MB' : '—'}</span>
    <span style="color:var(--text-dim)">Native Ctx</span><span id="mc-ctx">${card ? card.native_ctx : '—'}</span>
    <span style="color:var(--text-dim)">Layers</span>    <span id="mc-layers">${card ? card.layers : '—'}</span>
    <span style="color:var(--text-dim)">HF Repo</span>   <span id="mc-hf" style="word-break:break-all">${card ? card.hf_repo_id || '—' : '—'}</span>
  </div>
  ${card && card.description ? `<div id="mc-desc" style="color:var(--text-dim);font-style:italic;font-size:11px;line-height:1.4;max-height:60px;overflow-y:auto">${card.description}</div>` : ''}
  <div style="border-top:1px solid var(--border);padding-top:8px">
    <div style="font-size:11px;color:var(--text-dim);margin-bottom:4px">Suggested params from card</div>
    <div style="display:grid;grid-template-columns:auto 1fr;gap:3px 12px">
      <span style="color:var(--text-dim)">Temperature</span><span>${p.temperature != null ? p.temperature : '—'}</span>
      <span style="color:var(--text-dim)">Max tokens</span><span>${p.n_predict != null ? p.n_predict : '—'}</span>
      <span style="color:var(--text-dim)">Ctx size</span>  <span>${p.ctx_size != null ? p.ctx_size : '—'}</span>
    </div>
  </div>
  <div style="border-top:1px solid var(--border);padding-top:8px;display:flex;flex-direction:column;gap:6px">
    <div style="font-size:11px;color:var(--text-dim)">App Profile (applied on server start)</div>
    <select id="mc-profile-select" style="background:var(--input-bg);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:4px 6px">${profileOptions}</select>
    <div id="mc-profile-params" style="font-size:11px;color:var(--text-dim)"></div>
  </div>
  ${card ? `<button id="mc-fetch-hf" style="align-self:flex-start" ${!card.hf_repo_id ? '' : ''}>Fetch HF README</button>` : ''}
  <div id="mc-status" style="font-size:11px;color:var(--text-dim);min-height:14px"></div>
</div>`;

    function updateProfileParams() {
      const sel = div.querySelector('#mc-profile-select');
      if (!sel) return;
      const chosen = profiles.find(pr => pr.id === sel.value);
      const pDiv = div.querySelector('#mc-profile-params');
      if (!chosen || !pDiv) return;
      const parts = [];
      if (chosen.temperature_override != null) parts.push(`temp=${chosen.temperature_override}`);
      if (chosen.n_predict_override != null) parts.push(`n_predict=${chosen.n_predict_override}`);
      if (chosen.ctx_cap_override != null) parts.push(`ctx_cap=${chosen.ctx_cap_override}`);
      pDiv.textContent = parts.length ? parts.join('  |  ') : 'No overrides (model defaults)';
    }
    updateProfileParams();

    const sel = div.querySelector('#mc-profile-select');
    if (sel) sel.addEventListener('change', async () => {
      try {
        await invoke('cmd_set_active_profile', { profileId: sel.value });
        updateProfileParams();
      } catch (e) {
        const st = div.querySelector('#mc-status');
        if (st) st.textContent = 'Error: ' + e;
      }
    });

    const fetchBtn = div.querySelector('#mc-fetch-hf');
    if (fetchBtn && card) fetchBtn.addEventListener('click', async () => {
      const hfInput = prompt('HuggingFace repo ID (e.g. Qwen/Qwen3-8B):', card.hf_repo_id || '');
      if (!hfInput) return;
      const st = div.querySelector('#mc-status');
      if (st) st.textContent = 'Fetching README…';
      try {
        const settings = await invoke('cmd_get_settings');
        const modelPath = settings.last_model_path;
        if (!modelPath) throw new Error('No model loaded');
        const updated = await invoke('cmd_fetch_hf_card', { modelPath, hfRepoId: hfInput });
        currentCard = updated;
        if (st) st.textContent = 'README merged.';
        renderCard(div, updated, profiles, sel ? sel.value : activeProfileId);
      } catch (e) {
        if (st) st.textContent = 'Error: ' + e;
      }
    });
  }

  ExpansionSlots.register(9, {
    label: 'Model Card',
    icon: '📇',
    async init(div) {
      div.innerHTML = '<div style="padding:8px;color:var(--text-dim)">Loading…</div>';
      try {
        const [card, profiles, settings] = await Promise.all([
          invoke('cmd_get_model_card'),
          invoke('cmd_get_app_profiles'),
          invoke('cmd_get_settings'),
        ]);
        currentCard = card;
        renderCard(div, card, profiles, settings.active_app_profile || 'general');
      } catch (e) {
        div.innerHTML = `<div style="padding:8px;color:var(--text-dim)">Error: ${e}</div>`;
      }
    },
    destroy() {},
  });

  // Refresh when a new model is loaded.
  listen('model-card-loaded', async (evt) => {
    currentCard = evt.payload;
    const div = document.getElementById('exp-slot-9-content');
    if (!div || !ExpansionSlots.isOpen(9)) return;
    try {
      const [profiles, settings] = await Promise.all([
        invoke('cmd_get_app_profiles'),
        invoke('cmd_get_settings'),
      ]);
      renderCard(div, currentCard, profiles, settings.active_app_profile || 'general');
    } catch (_) {}
  });
})();

