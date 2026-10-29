'use strict';
const $ = s => document.querySelector(s), $$ = s => [...document.querySelectorAll(s)];
const SVG = 'http://www.w3.org/2000/svg';
const VIEWS = ['overview', 'search', 'activity', 'settings'];
const TITLES = {overview: 'Overview', search: 'Search', activity: 'Activity', settings: 'Settings'};
const KINDS = {rss: 'RSS', torznab: 'Torznab', unit3d: 'UNIT3D', json: 'JSON', html: 'HTML', cardigann: 'Preset'};
const KIND_ICONS = {rss: 'activity', torznab: 'plug', unit3d: 'server', json: 'layers', html: 'layers'};
const CATEGORY_NAMES = {1: 'Console', 2: 'Movies', 3: 'Audio', 4: 'PC', 5: 'TV', 6: 'XXX', 7: 'Books'};
const MODES = {search: 'Search', 'tv-search': 'TV', 'movie-search': 'Movies', 'music-search': 'Music', 'book-search': 'Books'};
const SORTS = {
  date: {label: 'Newest first', field: 'date', dir: -1},
  'date-asc': {label: 'Oldest first', field: 'date', dir: 1},
  seeders: {label: 'Most seeders', field: 'seeders', dir: -1},
  'seeders-asc': {label: 'Fewest seeders', field: 'seeders', dir: 1},
  size: {label: 'Largest first', field: 'size', dir: -1},
  'size-asc': {label: 'Smallest first', field: 'size', dir: 1},
  title: {label: 'Title A–Z', field: 'title', dir: 1},
  'title-desc': {label: 'Title Z–A', field: 'title', dir: -1},
};
const PAGE_SIZE = 20, RESULT_STEP = 100, CACHE_AGE = 30 * 60e3;
const collator = new Intl.Collator(undefined, {numeric: true, sensitivity: 'base'});
const state = {
  view: null, lastHash: null, entry: 0,
  loaded: false, loadError: null, dashboardAt: 0, indexers: [], stats: {}, activity: [],
  settings: null, settingsError: null,
  catalog: [], catalogStatus: 'idle', catalogError: null,
  tests: {}, testing: new Set(), page: 1,
  editing: null, preset: null, chosen: null, step: 'configure',
  search: {status: 'idle'}, searchCache: new Map(), searchSel: new Set(), searchHref: null, sort: 'date', lastSearchMs: null,
  activityFilter: 'all', activityStatus: 'idle', activityError: null, activityAt: 0,
};

/* Building blocks */
function el(tag, cls, text) {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text !== undefined && text !== null) n.textContent = text;
  return n;
}
function icon(name, cls = 'icon') {
  const s = document.createElementNS(SVG, 'svg'), u = document.createElementNS(SVG, 'use');
  s.setAttribute('class', cls);
  s.setAttribute('aria-hidden', 'true');
  s.setAttribute('focusable', 'false');
  u.setAttribute('href', '#i-' + name);
  s.append(u);
  return s;
}
function button(label, fn, {cls = 'btn', ic, aria, key} = {}) {
  const b = el('button', cls);
  b.type = 'button';
  if (ic) b.append(icon(ic));
  if (label) b.append(el('span', '', label));
  if (aria) b.setAttribute('aria-label', aria);
  if (key) b.dataset.focusKey = key;
  b.addEventListener('click', e => Promise.resolve(fn(b, e)).catch(report));
  return b;
}
function stateBox({tone = '', ic = 'info', title, text, code, actions = []}) {
  const box = el('div', 'state ' + tone), badge = el('span', 'state-icon');
  badge.append(ic === 'spinner' ? el('span', 'spinner lg') : icon(ic));
  box.append(badge, el('h3', '', title));
  if (text) box.append(el('p', '', text));
  if (code) box.append(el('span', 'code', code));
  if (actions.length) {
    const row = el('div', 'state-actions');
    for (const a of actions) row.append(button(a.label, a.fn, {cls: 'btn ' + (a.cls || ''), ic: a.ic}));
    box.append(row);
  }
  return box;
}
function panelWith(...nodes) { const p = el('div', 'panel'); p.append(...nodes); return p; }
function banner(tone, title, text) {
  const b = el('div', 'banner ' + tone), body = el('div', 'banner-body');
  b.append(icon(tone === 'bad' ? 'error' : tone === 'ok' ? 'ok' : tone === 'info' ? 'info' : 'alert'), body);
  if (title) body.append(el('p', 'banner-title', title));
  if (text) body.append(el('p', '', text));
  return b;
}
async function pending(b, fn, busy) {
  const kids = [...b.childNodes], focused = document.activeElement === b;
  b.disabled = true;
  b.setAttribute('aria-busy', 'true');
  if (busy) b.replaceChildren(el('span', 'spinner'), el('span', '', busy));
  try { return await fn(); } finally {
    b.disabled = false;
    b.removeAttribute('aria-busy');
    if (busy) b.replaceChildren(...kids);
    if (focused && b.isConnected && (document.activeElement === document.body || !document.activeElement)) b.focus();
  }
}

/* Formatting */
function num(n) { return Number(n || 0).toLocaleString(); }
function count(n, one, many = one + 's') { return `${num(n)} ${n === 1 ? one : many}`; }
function ms(n) { return n < 1000 ? `${n} ms` : `${(n / 1000).toFixed(n < 10000 ? 2 : 1)} s`; }
function size(n) { if (n === null || n === undefined) return '—'; if (n === 0) return '0 B'; const p = Math.min(Math.floor(Math.log(n) / Math.log(1024)), 4); return (n / 1024 ** p).toFixed(p ? 1 : 0) + ' ' + ['B', 'KB', 'MB', 'GB', 'TB'][p]; }
function timeOf(v) { if (!v) return null; const t = Date.parse(v); return Number.isNaN(t) ? null : t; }
function stamp(t) { return new Date(t).toLocaleString(undefined, {dateStyle: 'medium', timeStyle: 'short'}); }
function ago(t) {
  const s = (Date.now() - t) / 1000;
  if (s < 45) return 'just now';
  const [n, unit] = s < 3600 ? [s / 60, 'min'] : s < 86400 ? [s / 3600, 'h'] : s < 2592000 ? [s / 86400, 'd'] : s < 31536000 ? [s / 2592000, 'mo'] : [s / 31536000, 'y'];
  return Math.max(1, Math.round(n)) + ' ' + unit;
}
function uptime(s) { const m = Math.floor(s / 60), h = Math.floor(m / 60); return s < 60 ? `${s} s` : m < 60 ? `${m} min` : h < 48 ? `${h} h ${m % 60} min` : `${Math.floor(h / 24)} d ${h % 24} h`; }
function hostOf(u) { try { return new URL(u).host || u; } catch { return u; } }
function originOf(u) { try { const x = new URL(u); return /^https?:$/.test(x.protocol) && x.host ? x.origin : null; } catch { return null; } }
function categoryName(id) { return CATEGORY_NAMES[Math.floor(id / 1000)] || 'Other'; }
function safeUrl(value) { try { const u = new URL(value); return ['http:', 'https:', 'magnet:'].includes(u.protocol) ? u.href : null; } catch { return null; } }
function endpoint(id = 'all') { return location.origin + '/api/v2.0/indexers/' + encodeURIComponent(id) + '/results/torznab'; }
function richText(text) {
  const out = document.createDocumentFragment();
  if (!/[<&]/.test(text)) { out.append(text); return out; }
  const walk = node => {
    for (const n of node.childNodes) {
      if (n.nodeType === Node.TEXT_NODE) out.append(n.textContent);
      else if (n.nodeName === 'BR') out.append('\n');
      else if (n.nodeName === 'A') {
        const href = safeUrl(n.getAttribute('href') || '');
        if (href && !href.startsWith('magnet:')) { const a = el('a', '', n.textContent); a.href = href; a.target = '_blank'; a.rel = 'noopener noreferrer'; out.append(a); } else out.append(n.textContent);
      } else if (n.nodeType === Node.ELEMENT_NODE) walk(n);
    }
  };
  walk(new DOMParser().parseFromString(text, 'text/html').body);
  return out;
}

/* Server access */
class ApiError extends Error { constructor(message, status) { super(message); this.name = 'ApiError'; this.status = status; } }
function describe(e) { return e?.status ? `${e.message} (HTTP ${e.status})` : e?.message || String(e); }
function report(e) { if (e?.name !== 'AbortError') toast(describe(e), 'bad'); }
async function api(path, options = {}) {
  let r;
  try { r = await fetch(path, {...options, headers: {'Content-Type': 'application/json', ...(options.headers || {})}}); } catch (e) {
    if (e.name === 'AbortError') throw e;
    setConnection('offline');
    throw new ApiError('Rackett did not respond. Check that the server is running, then try again.', 0);
  }
  setConnection('online');
  const t = await r.text();
  let data;
  try { data = t ? JSON.parse(t) : null; } catch { data = t; }
  if (!r.ok) {
    const message = data && typeof data === 'object' ? data.error || data.message : typeof data === 'string' && data && !/^\s*</.test(data) ? data : '';
    throw new ApiError(String(message || r.statusText || 'Request failed').slice(0, 400), r.status);
  }
  return data;
}
function setConnection(s) {
  const n = $('#connection');
  if (n.dataset.state === s) return;
  n.dataset.state = s;
  n.querySelector('span').textContent = {connecting: 'Connecting…', online: 'Connected', offline: 'Server unreachable'}[s];
}
function toast(message, kind = 'ok') {
  const box = $('#toasts'), t = el('div', 'toast ' + kind), close = el('button');
  let timer;
  const dismiss = () => { clearTimeout(timer); t.remove(); }, arm = () => { clearTimeout(timer); timer = setTimeout(dismiss, kind === 'bad' ? 12000 : 5000); };
  if (kind === 'bad') t.setAttribute('role', 'alert');
  close.type = 'button';
  close.setAttribute('aria-label', 'Dismiss notification');
  close.append(icon('close'));
  close.addEventListener('click', dismiss);
  t.append(icon({ok: 'ok', bad: 'error', warn: 'alert'}[kind] || 'info'), el('p', '', message), close);
  t.addEventListener('mouseenter', () => clearTimeout(timer));
  t.addEventListener('mouseleave', arm);
  t.addEventListener('focusin', () => clearTimeout(timer));
  t.addEventListener('focusout', arm);
  box.append(t);
  while (box.children.length > 3) box.firstElementChild.remove();
  arm();
}
async function copy(text, done = 'Copied to clipboard') {
  if (!text) return toast('There is nothing to copy yet.', 'warn');
  try { await navigator.clipboard.writeText(text); toast(done); } catch { toast('Clipboard access is unavailable here. Select the value and copy it manually.', 'warn'); }
}

/* Navigation: hash routes, history-aware dialogs and menus */
function parseRoute() {
  const raw = location.hash.replace(/^#\/?/, ''), i = raw.indexOf('?'), name = i < 0 ? raw : raw.slice(0, i), valid = VIEWS.includes(name);
  return {name: valid ? name : 'overview', params: new URLSearchParams(valid && i >= 0 ? raw.slice(i + 1) : ''), valid: valid || !name};
}
function routeHref(name, params) { const q = params ? params.toString() : ''; return '#' + name + (q ? '?' + q : ''); }
function go(name, params, {replace = false} = {}) {
  const href = routeHref(name, params), open = $$('dialog[open]');
  if (href === location.hash) {
    open.forEach(d => { d._noRestore = true; d.close(); });
    return applyRoute({top: true, focus: true});
  }
  if (history.state?.dialog) {
    open.forEach(d => { d._keepHistory = true; d._noRestore = true; d.close(); });
    replace = true;
  }
  history[replace ? 'replaceState' : 'pushState']({entry: ++state.entry}, '', href);
  applyRoute({top: true, focus: true});
}
function applyRoute({top = false, focus = false} = {}) {
  const r = parseRoute();
  if (!r.valid) history.replaceState(history.state, '', '#overview');
  state.lastHash = location.hash;
  const entering = state.view !== r.name;
  if (entering && state.view === 'settings') maskKey();
  state.view = r.name;
  for (const v of $$('.view')) v.hidden = v.id !== 'view-' + r.name;
  for (const a of $$('.nav a')) a.dataset.nav === r.name ? a.setAttribute('aria-current', 'page') : a.removeAttribute('aria-current');
  document.title = TITLES[r.name] + ' · Rackett';
  closeMenu();
  if (r.name === 'search') syncSearch(r.params);
  else if (r.name === 'activity' && (entering || !state.activityAt)) loadActivity();
  else if (r.name === 'overview' && entering && state.loaded && Date.now() - state.dashboardAt > 10000) load({quiet: true});
  else if (r.name === 'settings') renderSettings();
  if (top) window.scrollTo(0, 0);
  if (focus) $(`#view-${r.name} h1`).focus({preventScroll: true});
}
window.addEventListener('popstate', () => {
  for (const d of $$('dialog[open]')) if (history.state?.dialog !== d.id) d.close();
  if (location.hash !== state.lastHash) applyRoute({focus: true});
});
window.addEventListener('hashchange', () => { if (location.hash !== state.lastHash) applyRoute({top: true, focus: true}); });
document.addEventListener('click', e => {
  const a = e.target.closest('a[href^="#"]');
  if (!a || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  e.preventDefault();
  const raw = a.getAttribute('href').slice(1), i = raw.indexOf('?');
  if (raw === 'main') return $('#main').focus();
  go(i < 0 ? raw : raw.slice(0, i), new URLSearchParams(i < 0 ? '' : raw.slice(i + 1)));
});
function openDialog(d, opener = document.activeElement) {
  closeMenu();
  Object.assign(d, {_opener: opener, _keepHistory: false, _noRestore: false});
  d.showModal();
  history.pushState({...(history.state || {}), dialog: d.id}, '', location.href);
}
function restoreFocus(o) {
  if (o?.isConnected && o.getClientRects().length) return o.focus();
  const key = o?.dataset?.focusKey, alt = key && $(`[data-focus-key="${CSS.escape(key)}"]`);
  if (alt && alt.getClientRects().length) return alt.focus();
  $(`#view-${state.view} h1`)?.focus({preventScroll: true});
}
for (const d of $$('dialog')) {
  d.addEventListener('close', () => {
    if (!d._keepHistory && history.state?.dialog === d.id) history.back();
    const o = d._opener;
    d._opener = null;
    if (!d._noRestore) restoreFocus(o);
  });
  d.addEventListener('click', e => {
    if (e.target !== d || d.id === 'indexer-dialog') return;
    const r = d.getBoundingClientRect();
    if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) d.close();
  });
}
$$('.close-dialog').forEach(b => b.addEventListener('click', () => b.closest('dialog').close()));
function confirmAction(title, text, label = 'Confirm') {
  return new Promise(resolve => {
    const d = $('#confirm-dialog');
    let result = false;
    $('#confirm-title').textContent = title;
    $('#confirm-text').textContent = text;
    $('#confirm-ok').textContent = label;
    $('#confirm-ok').onclick = () => { result = true; d.close(); };
    $('#confirm-cancel').onclick = () => d.close();
    d.addEventListener('close', () => resolve(result), {once: true});
    openDialog(d);
    $('#confirm-cancel').focus();
  });
}
let menuTrigger = null;
function openMenu(trigger, items) {
  if (menuTrigger === trigger) return closeMenu(true);
  closeMenu();
  const m = $('#menu');
  m.replaceChildren();
  for (const it of items) {
    if (!it) { m.append(el('div', 'menu-sep')); continue; }
    const b = el('button', 'menu-item' + (it.danger ? ' danger' : ''));
    b.type = 'button';
    b.tabIndex = -1;
    b.setAttribute('role', 'menuitem');
    b.append(icon(it.icon), el('span', '', it.label));
    b.addEventListener('click', () => { closeMenu(true); Promise.resolve(it.fn()).catch(report); });
    m.append(b);
  }
  m.hidden = false;
  menuTrigger = trigger;
  trigger.setAttribute('aria-expanded', 'true');
  positionMenu();
  m.querySelector('.menu-item').focus({preventScroll: true});
}
function positionMenu() {
  const m = $('#menu'), t = menuTrigger;
  if (!t?.isConnected) return closeMenu();
  const r = t.getBoundingClientRect(), w = m.offsetWidth, h = m.offsetHeight;
  if (r.bottom < 0 || r.top > innerHeight) return closeMenu();
  m.style.top = (r.bottom + 6 + h > innerHeight - 8 && r.top - h - 6 > 8 ? r.top - h - 6 : r.bottom + 6) + 'px';
  m.style.left = Math.max(8, Math.min(r.right - w, innerWidth - w - 8)) + 'px';
}
function closeMenu(refocus = false) {
  const m = $('#menu'), t = menuTrigger;
  if (m.hidden) return;
  m.hidden = true;
  menuTrigger = null;
  t?.setAttribute('aria-expanded', 'false');
  if (refocus && t?.isConnected) t.focus({preventScroll: true});
}
$('#menu').addEventListener('keydown', e => {
  const items = $$('#menu .menu-item'), i = items.indexOf(document.activeElement);
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); items[(i + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length].focus(); }
  else if (e.key === 'Home' || e.key === 'End') { e.preventDefault(); items[e.key === 'Home' ? 0 : items.length - 1].focus(); }
  else if (e.key === 'Escape') { e.preventDefault(); closeMenu(true); }
  else if (e.key === 'Tab') closeMenu(true);
});
document.addEventListener('pointerdown', e => { if (menuTrigger && !e.target.closest('#menu') && e.target.closest('button') !== menuTrigger) closeMenu(); }, true);
window.addEventListener('resize', () => { if (menuTrigger) positionMenu(); });
window.addEventListener('scroll', () => { if (menuTrigger) positionMenu(); }, true);
document.addEventListener('keydown', e => {
  if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey || e.defaultPrevented || menuTrigger || $('dialog[open]')) return;
  if (e.target.closest?.('input, textarea, select, [contenteditable="true"]')) return;
  const target = state.view === 'search' ? $('#search-query') : state.view === 'overview' ? $('#indexer-filter') : null;
  if (target) { e.preventDefault(); target.focus(); target.select(); }
});

/* Overview */
async function load({quiet = false} = {}) {
  const started = Date.now();
  try {
    const data = await api('/api/dashboard');
    state.indexers = data.indexers || [];
    state.activity = data.activity || [];
    state.stats = data.stats || {};
    state.dashboardAt = started;
    state.activityAt = Math.max(state.activityAt, started);
    state.loaded = true;
    state.loadError = null;
  } catch (e) {
    state.loadError = e;
    if (state.loaded && !quiet) toast('Could not refresh your sources: ' + describe(e), 'bad');
  }
  renderOverview();
  renderSourceChips();
  if (state.view === 'activity') renderActivity();
  renderCacheFacts();
  return !state.loadError;
}
function sourceHealth(x) {
  if (state.testing.has(x.id)) return {key: 'testing', label: 'Testing…'};
  const t = state.tests[x.id], server = x.status && x.status !== 'untested' ? {ok: x.status === 'healthy' || x.status === 'ok', message: x.last_error || x.status, latency_ms: x.latency_ms} : null;
  const r = t && (!server || t.at > state.dashboardAt) ? t : server;
  if (!r) return {key: 'untested', label: 'Not tested'};
  return r.ok ? {key: 'healthy', label: 'Healthy', latency: r.latency_ms} : {key: 'failing', label: 'Failing', message: r.message || 'The last check failed.', latency: r.latency_ms};
}
function renderOverview() { renderStats(); renderSources(); if (menuTrigger && !menuTrigger.isConnected) closeMenu(); }
function renderStats() {
  const all = state.indexers, enabled = all.filter(x => x.enabled), s = state.stats, h = {healthy: 0, failing: 0, untested: 0};
  if (!state.loaded) {
    for (const id of ['#stat-total', '#stat-health', '#stat-searches', '#stat-cache']) $(id).textContent = '—';
    $('#stat-total-sub').textContent = state.loadError ? 'Unavailable' : 'Loading…';
    return;
  }
  for (const x of enabled) { const k = sourceHealth(x).key; h[k === 'testing' ? 'untested' : k]++; }
  $('#stat-total').textContent = num(all.length);
  $('#stat-total-sub').textContent = all.length ? `${num(enabled.length)} enabled · ${num(all.length - enabled.length)} paused` : 'No sources yet';
  const health = $('#stat-health');
  health.className = 'stat-value' + (h.failing ? ' bad' : enabled.length && h.healthy === enabled.length ? ' ok' : '');
  health.textContent = !enabled.length ? '—' : h.failing ? `${num(h.failing)} failing` : h.healthy === enabled.length ? 'All healthy' : h.healthy ? `${num(h.healthy)} healthy` : 'Not tested';
  $('#stat-health-sub').textContent = !enabled.length ? (all.length ? 'Every source is paused' : 'Add a source to check it') : [h.healthy && `${num(h.healthy)} healthy`, h.failing && `${num(h.failing)} failing`, h.untested && `${num(h.untested)} not tested`].filter(Boolean).join(' · ');
  const meter = $('#stat-meter');
  meter.replaceChildren();
  for (const [n, cls] of [[h.healthy, 'm-ok'], [h.failing, 'm-bad'], [h.untested, 'm-idle'], [all.length - enabled.length, 'm-pause']]) if (n) { const seg = el('span', cls); seg.style.flexGrow = n; meter.append(seg); }
  $('#stat-searches').textContent = num(s.searches ?? 0);
  $('#stat-searches-sub').textContent = [s.uptime_seconds !== undefined && `Server up ${uptime(s.uptime_seconds)}`, state.lastSearchMs !== null && `last took ${ms(state.lastSearchMs)}`].filter(Boolean).join(' · ') || 'Since the server started';
  $('#stat-cache').textContent = num(s.cache_hits ?? 0);
  $('#stat-cache-sub').textContent = `${count(s.cache_entries ?? 0, 'stored result set')}` + (state.settings ? ` · ${state.settings.cache_ttl_seconds ? state.settings.cache_ttl_seconds + ' s lifetime' : 'caching off'}` : '');
}
function filtersActive() { return !!($('#indexer-filter').value.trim() || $('#kind-filter').value || $('#status-filter').value || $('#tag-filter').value); }
function sourceMatches(x) {
  const q = $('#indexer-filter').value.trim().toLowerCase(), kind = $('#kind-filter').value, status = $('#status-filter').value, tag = $('#tag-filter').value;
  if (q && !q.split(/\s+/).every(term => [x.name, x.url, x.id, KINDS[x.kind], ...(x.tags || [])].join(' ').toLowerCase().includes(term))) return false;
  if (kind && x.kind !== kind) return false;
  if (tag && !(x.tags || []).includes(tag)) return false;
  if (!status) return true;
  if (status === 'paused') return !x.enabled;
  const k = sourceHealth(x).key;
  return x.enabled && (k === status || (status === 'untested' && k === 'testing'));
}
function updateTagFilter() {
  const sel = $('#tag-filter'), current = sel.value, tags = [...new Set(state.indexers.flatMap(x => x.tags || []))].sort(collator.compare);
  sel.replaceChildren(new Option('All tags', ''), ...tags.map(t => new Option(t, t)));
  sel.value = tags.includes(current) ? current : '';
  sel.hidden = !tags.length;
}
function renderSources() {
  const list = $('#indexer-list'), all = state.indexers, pg = $('#pagination'), focusKey = document.activeElement?.dataset?.focusKey;
  list.setAttribute('aria-busy', String(!state.loaded && !state.loadError));
  $('#indexer-count').textContent = num(all.length);
  updateTagFilter();
  const rows = all.filter(sourceMatches), active = filtersActive(), pages = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  $('#clear-filters').hidden = !active;
  $('#source-toolbar').hidden = state.loaded && !all.length;
  state.page = Math.min(state.page, pages);
  $('#sources-summary').textContent = !state.loaded ? (state.loadError ? 'Your sources could not be loaded.' : 'Loading your sources…') : !all.length ? 'Add a source to start searching.' : active ? `Showing ${num(rows.length)} of ${count(all.length, 'source')}` : `${count(all.filter(x => x.enabled).length, 'source')} enabled for search`;
  pg.hidden = true;
  if (!state.loaded) {
    list.replaceChildren(state.loadError ? stateBox({tone: 'bad', ic: 'error', title: 'Sources could not be loaded', text: describe(state.loadError), actions: [{label: 'Try again', ic: 'refresh', fn: b => pending(b, () => Promise.all([load(), loadSettings()]), 'Retrying…')}]}) : stateBox({ic: 'spinner', title: 'Loading your sources…'}));
    return;
  }
  if (!all.length) {
    list.replaceChildren(stateBox({tone: 'ok', ic: 'layers', title: 'Every great search starts with a source', text: 'Add a bundled preset, an RSS or Torznab feed, a UNIT3D API or a custom JSON or HTML endpoint. Rackett then searches all of them at once.', actions: [{label: 'Add your first source', ic: 'plus', cls: 'primary', fn: b => openIndexer(null, b)}]}));
    return;
  }
  if (!rows.length) {
    list.replaceChildren(stateBox({ic: 'filter', title: 'No sources match these filters', text: 'Try another name, adapter, status or tag.', actions: [{label: 'Clear filters', fn: clearFilters}]}));
    return;
  }
  const ul = el('ul', 'source-list');
  ul.append(...rows.slice((state.page - 1) * PAGE_SIZE, state.page * PAGE_SIZE).map(sourceRow));
  list.replaceChildren(ul);
  if (rows.length > PAGE_SIZE) {
    const nav = el('div'), prev = button('Previous', () => { state.page--; renderSources(); }, {cls: 'btn sm', key: 'page:prev'}), next = button('Next', () => { state.page++; renderSources(); }, {cls: 'btn sm', key: 'page:next'});
    prev.disabled = state.page === 1;
    next.disabled = state.page === pages;
    nav.append(prev, next);
    pg.replaceChildren(el('span', '', `${num((state.page - 1) * PAGE_SIZE + 1)}–${num(Math.min(state.page * PAGE_SIZE, rows.length))} of ${num(rows.length)}`), nav);
    pg.hidden = false;
  }
  if (focusKey && document.activeElement === document.body) $(`[data-focus-key="${CSS.escape(focusKey)}"]:not(:disabled)`)?.focus();
}
function sourceRow(x) {
  const h = sourceHealth(x), li = el('li', 'source' + (x.enabled ? '' : ' paused')), avatar = el('span', 'avatar', (x.name.trim()[0] || '?').toUpperCase());
  const main = el('div', 'source-main'), title = el('div', 'source-title'), meta = el('p', 'source-meta');
  avatar.setAttribute('aria-hidden', 'true');
  title.append(el('h3', 'source-name', x.name), el('span', 'badge', KINDS[x.kind] || x.kind));
  meta.append(el('span', 'host', hostOf(x.url)));
  for (const t of x.tags || []) meta.append(el('span', 'tag', t));
  main.append(title, meta);
  if (h.key === 'failing') { const err = el('p', 'source-error'); err.append(icon('error'), el('span', '', h.message)); main.append(err); }
  const shown = x.enabled || h.key === 'testing' ? h : {key: 'paused', label: 'Paused'}, health = el('p', 'health ' + shown.key);
  health.append(shown.key === 'testing' ? el('span', 'spinner') : el('i'), el('span', '', shown.label));
  if (shown.latency !== undefined && shown.latency !== null) health.append(el('small', '', '· ' + ms(shown.latency)));
  const actions = el('div', 'source-actions');
  const test = button('Test', () => testIndexer(x), {cls: 'btn sm', ic: 'zap', aria: `Test ${x.name}`, key: 'test:' + x.id});
  if (state.testing.has(x.id)) test.setAttribute('aria-disabled', 'true');
  const more = button('', b => openMenu(b, [
    {label: x.enabled ? 'Pause' : 'Enable', icon: x.enabled ? 'pause' : 'play', fn: () => toggleSource(x)},
    {label: 'Copy Torznab URL', icon: 'link', fn: () => copy(endpoint(x.id), 'Torznab URL copied')},
    null,
    {label: 'Remove…', icon: 'trash', danger: true, fn: () => removeSource(x)},
  ]), {cls: 'btn sm icon-only', ic: 'more', aria: `More actions for ${x.name}`, key: 'more:' + x.id});
  more.setAttribute('aria-haspopup', 'menu');
  more.setAttribute('aria-expanded', 'false');
  more.setAttribute('aria-controls', 'menu');
  actions.append(test, button('Edit', b => openIndexer(x, b), {cls: 'btn sm', ic: 'edit', aria: `Edit ${x.name}`, key: 'edit:' + x.id}), more);
  li.append(avatar, main, health, actions);
  return li;
}
function clearFilters() {
  for (const id of ['#indexer-filter', '#kind-filter', '#status-filter', '#tag-filter']) $(id).value = '';
  state.page = 1;
  renderSources();
  $('#indexer-filter').focus();
}
['#indexer-filter', '#kind-filter', '#status-filter', '#tag-filter'].forEach(s => $(s).addEventListener('input', () => { state.page = 1; renderSources(); }));
$('#clear-filters').addEventListener('click', clearFilters);
async function testIndexer(x, {announce = true} = {}) {
  if (state.testing.has(x.id)) return null;
  state.testing.add(x.id);
  renderOverview();
  let result;
  try { result = await api('/api/indexers/' + encodeURIComponent(x.id) + '/test', {method: 'POST'}); } catch (e) { result = {ok: false, message: describe(e)}; }
  state.testing.delete(x.id);
  state.tests[x.id] = {...result, at: Date.now()};
  renderOverview();
  if (announce) toast(result.ok ? `${x.name} responded in ${ms(result.latency_ms ?? 0)}` : `${x.name} needs attention: ${result.message}`, result.ok ? 'ok' : 'bad');
  load({quiet: true});
  return result;
}
async function toggleSource(x) {
  try { await api('/api/indexers/' + encodeURIComponent(x.id), {method: 'PATCH', body: JSON.stringify({enabled: !x.enabled})}); } catch (e) {
    return toast(`Could not ${x.enabled ? 'pause' : 'enable'} ${x.name}: ${describe(e)}`, 'bad');
  }
  await load();
  toast(x.enabled ? `${x.name} paused. Searches skip it until you enable it again.` : `${x.name} enabled`);
}
async function removeSource(x) {
  if (!await confirmAction(`Remove ${x.name}?`, 'Its saved configuration, including any credentials, is deleted. You can add it again later.', 'Remove source')) return;
  try { await api('/api/indexers/' + encodeURIComponent(x.id), {method: 'DELETE'}); } catch (e) { return toast(`Could not remove ${x.name}: ${describe(e)}`, 'bad'); }
  delete state.tests[x.id];
  state.searchSel.delete(x.id);
  await load();
  toast(`${x.name} removed`);
  $('#sources-title').focus();
}

/* Source dialog */
function stockEntry(id) { return state.catalog.find(x => x.kind === 'cardigann' && x.id === id); }
function optionPairs(options) { return Array.isArray(options) ? options.map(x => x && typeof x === 'object' ? [x.value ?? x.id, x.label ?? x.name] : [x, x]) : Object.entries(options || {}); }
async function loadCatalog() {
  state.catalogStatus = 'loading';
  renderCatalog();
  try {
    const data = await api('/api/catalog');
    state.catalog = Array.isArray(data) ? data : data.indexers || data.catalog || [];
    state.catalogStatus = 'ready';
  } catch (e) {
    state.catalogStatus = 'failed';
    state.catalogError = e;
  }
  renderCatalog();
  if (state.editing?.kind === 'cardigann' && $('#indexer-form').elements.kind.value === 'cardigann') renderPreset(stockEntry(state.editing.definition?.builtin_id), state.editing.definition?.settings || {});
}
async function openIndexer(item, opener) {
  const f = $('#indexer-form');
  Object.assign(state, {editing: item || null, preset: null, chosen: null});
  f.reset();
  delete f.elements.definition.dataset.suggested;
  $('#advanced').open = false;
  f.querySelectorAll('[aria-invalid]').forEach(n => n.removeAttribute('aria-invalid'));
  $('#catalog-search').value = '';
  $('#catalog-blocked').checked = false;
  for (const key of ['name', 'kind', 'url', 'api_key', 'cookie', 'request_delay_ms', 'retries']) if (item && item[key] !== undefined) f.elements[key].value = item[key];
  f.elements.tags.value = (item?.tags || []).join(', ');
  f.elements.categories.value = (item?.categories || []).join(', ');
  f.elements.definition.value = item?.definition ? JSON.stringify(item.definition, null, 2) : '';
  f.elements.api_key.placeholder = item ? 'Leave blank to keep the saved key' : 'If required';
  f.elements.cookie.placeholder = item ? 'Leave blank to keep the saved cookie' : 'If required';
  $('#url-privacy-note').hidden = !item?.url_redacted;
  renderPreset(item?.kind === 'cardigann' ? stockEntry(item.definition?.builtin_id) : null, item?.definition?.settings || {});
  renderChosen();
  showStep(item ? 'configure' : 'choose');
  openDialog($('#indexer-dialog'), opener);
  (item ? f.elements.name : $('#catalog-search')).focus();
  if (state.catalogStatus !== 'ready') await loadCatalog(); else renderCatalog();
}
function showStep(step) {
  const choose = step === 'choose', item = state.editing;
  state.step = step;
  $('#step-choose').hidden = !choose;
  $('#step-configure').hidden = choose;
  $('#save-indexer').hidden = choose;
  $('#step-back').hidden = choose || !!item;
  $('#dialog-step').textContent = item ? (item.kind === 'cardigann' ? 'Bundled preset' : `${KINDS[item.kind] || item.kind} source`) : choose ? 'Step 1 of 2' : 'Step 2 of 2';
  $('#dialog-title').textContent = item ? `Edit ${item.name}` : choose ? 'Add a source' : 'Configure your source';
  formError('');
  $('#indexer-dialog .dialog-body').scrollTop = 0;
}
function renderChosen() {
  const box = $('#chosen'), x = state.editing ? null : state.chosen;
  box.replaceChildren();
  if (!x || x.kind === 'cardigann') return;
  const badge = el('span', 'option-icon'), text = el('span', 'option-text');
  badge.append(icon(KIND_ICONS[x.kind] || 'layers'));
  badge.setAttribute('aria-hidden', 'true');
  text.append(el('strong', '', x.name), el('small', '', `Custom adapter · ${KINDS[x.kind] || x.kind}`));
  box.append(badge, text);
}
function renderCatalog() {
  const list = $('#catalog-list'), blockedBox = $('#catalog-blocked-list'), summary = $('#catalog-summary');
  blockedBox.hidden = true;
  if (state.catalogStatus !== 'ready') {
    summary.textContent = '';
    list.replaceChildren(state.catalogStatus === 'failed' ? stateBox({tone: 'bad', ic: 'error', title: 'The source catalog could not load', text: `${describe(state.catalogError)}. You can still add a custom endpoint.`, actions: [{label: 'Try again', ic: 'refresh', fn: loadCatalog}, {label: 'Use a custom endpoint', cls: 'primary', fn: () => { state.chosen = null; renderChosen(); showStep('configure'); $('#indexer-form').elements.name.focus(); }}]}) : stateBox({ic: 'spinner', title: 'Loading the source catalog…'}));
    return;
  }
  const q = $('#catalog-search').value.trim().toLowerCase(), showBlocked = $('#catalog-blocked').checked;
  const matches = state.catalog.filter(x => [x.name, x.id, x.language, x.type].join(' ').toLowerCase().includes(q));
  const available = matches.filter(x => x.kind !== 'cardigann' || x.ready === true), blocked = matches.filter(x => x.kind === 'cardigann' && x.ready !== true);
  list.replaceChildren();
  for (const [label, items] of [['Bundled presets', available.filter(x => x.kind === 'cardigann')], ['Custom adapters', available.filter(x => x.kind !== 'cardigann')]]) {
    if (!items.length) continue;
    const group = el('div', 'catalog-group'), heading = el('h3', '', label);
    heading.id = 'catalog-' + label.split(' ')[0].toLowerCase();
    group.setAttribute('role', 'group');
    group.setAttribute('aria-labelledby', heading.id);
    group.append(heading, ...items.map(optionButton));
    list.append(group);
  }
  if (!available.length) list.append(stateBox({ic: 'search', title: 'No selectable sources match', text: blocked.length && !showBlocked ? `${count(blocked.length, 'unsupported definition')} match. Show unsupported definitions to see why they are unavailable.` : 'Try a different name, language or type.'}));
  blockedBox.hidden = !showBlocked;
  blockedBox.replaceChildren();
  if (showBlocked) {
    const shown = blocked.slice(0, 200);
    blockedBox.append(el('p', '', !blocked.length ? 'No unsupported definitions match.' : blocked.length > shown.length ? `Unsupported definitions · showing ${shown.length} of ${num(blocked.length)}. Refine your search to see others.` : `Unsupported definitions · ${num(blocked.length)}`));
    for (const x of shown) {
      const row = el('article', 'blocked-source');
      row.append(el('strong', '', x.name), el('small', '', [x.language, x.type].filter(Boolean).join(' · ')), el('p', '', x.blocked_reason || 'Runtime support is not available.'));
      blockedBox.append(row);
    }
  }
  summary.textContent = `${count(available.length, 'selectable source')} · ${count(blocked.length, 'unsupported definition')} ${showBlocked ? 'shown below' : 'hidden'}. Preset support is fixture tested, not a live availability check.`;
}
function optionButton(x) {
  const b = el('button', 'option' + (x.kind === 'cardigann' ? ' preset' : '')), badge = el('span', 'option-icon', x.kind === 'cardigann' ? (x.name || '?')[0].toUpperCase() : null), text = el('span', 'option-text');
  b.type = 'button';
  if (x.kind !== 'cardigann') badge.append(icon(KIND_ICONS[x.kind] || 'layers'));
  badge.setAttribute('aria-hidden', 'true');
  text.append(el('strong', '', x.name), el('small', '', x.kind === 'cardigann' ? ['Bundled preset', x.language, x.type].filter(Boolean).join(' · ') : `Custom adapter · ${KINDS[x.kind] || x.kind}`));
  if (x.description) text.append(el('span', 'option-desc', x.description));
  b.append(badge, text, icon('chevron-right'));
  b.addEventListener('click', () => chooseEntry(x));
  return b;
}
function chooseEntry(x) {
  const f = $('#indexer-form'), prev = state.chosen;
  for (const k of ['name', 'url']) { const field = f.elements[k], current = field.value.trim(); if (!current || (prev && current === (prev[k] || ''))) field.value = x[k] || ''; }
  f.elements.kind.value = x.kind;
  state.chosen = x;
  if (x.kind === 'cardigann') { f.elements.definition.value = ''; f.elements.retries.value = '0'; renderPreset(x); } else { renderPreset(null); suggestDefinition(); }
  renderChosen();
  showStep('configure');
  f.elements.name.focus();
  if (x.kind !== 'cardigann') f.elements.name.select();
}
function currentPreset() { return state.chosen?.kind === 'cardigann' ? state.chosen : state.editing?.kind === 'cardigann' ? stockEntry(state.editing.definition?.builtin_id) : null; }
function renderPreset(entry, values = {}) {
  state.preset = entry || null;
  const f = $('#indexer-form'), stock = f.elements.kind.value === 'cardigann', target = $('#preset-settings'), info = $('#preset-info');
  target.replaceChildren();
  info.replaceChildren();
  target.hidden = !stock || !entry?.settings?.length;
  info.hidden = !stock;
  $('#custom-credentials').hidden = stock;
  $('#definition-label').hidden = stock;
  $('#url-label').textContent = stock ? 'Mirror URL' : 'Endpoint URL';
  f.elements.url.placeholder = stock ? 'https://mirror.example/' : 'https://example.com/feed';
  if (stock && !entry) {
    const waiting = state.editing?.kind === 'cardigann' && state.catalogStatus !== 'ready';
    const note = banner(waiting && state.catalogStatus !== 'failed' ? 'info' : 'warn', waiting ? (state.catalogStatus === 'failed' ? 'Preset details could not load' : 'Loading preset details…') : 'Choose a supported bundled preset before saving.');
    if (!state.editing) note.querySelector('.banner-body').append(button('Choose a preset', () => showStep('choose'), {cls: 'btn link'}));
    info.append(note);
  } else if (stock) {
    const head = el('div', 'preset-head'), caps = entry.caps;
    head.append(el('strong', '', entry.name));
    for (const t of [entry.language, entry.type].filter(Boolean)) head.append(el('span', 'badge outline', t));
    head.append(el('span', 'badge mint', 'Fixture tested'));
    info.append(head);
    if (entry.description) info.append(el('p', '', entry.description));
    if (caps && (Array.isArray(caps) ? caps.length : Object.keys(caps).length)) {
      const row = el('div', 'caps');
      row.setAttribute('aria-label', 'Search capabilities');
      for (const [mode, params] of Array.isArray(caps) ? caps.map(c => [c, null]) : Object.entries(caps).filter(([, v]) => v !== false && v !== null)) row.append(el('span', 'badge', (MODES[mode] || mode) + (Array.isArray(params) && params.length ? `: ${params.join(', ')}` : '')));
      info.append(row);
    }
    if (entry.settings?.length) target.append(el('p', 'settings-legend', 'Preset settings'));
    for (const setting of entry.settings || []) target.append(presetField(setting, values));
  }
  updateOriginNote();
}
function presetField(setting, values) {
  const type = setting.type === 'multi-select' ? 'multiselect' : setting.type || 'text', name = setting.name, label = setting.label || setting.name, id = 'ps-' + String(name).replace(/[^\w-]/g, '_');
  const value = Object.hasOwn(values, name) ? values[name] : setting.default;
  if (type === 'info' || type === 'info_flaresolverr' || type === 'info_cookie') {
    const box = el('div', 'setting-info'), text = String(setting.default || (type === 'info_cookie' ? 'Paste the Cookie request header from your signed-in tracker session. It stays hidden after saving. If your session expires, replace it here.' : type === 'info_flaresolverr' ? 'Challenge solving is not supported. This preset only works when the source responds directly.' : ''));
    box.append(el('strong', '', type === 'info_flaresolverr' && label === 'Information' ? 'Challenge solving' : label));
    if (text) { const p = el('p'); p.append(richText(text)); box.append(p); }
    return box;
  }
  if (type === 'checkbox') {
    const l = el('label', 'check'), c = el('input');
    c.type = 'checkbox';
    c.checked = value === true || value === 'true';
    Object.assign(c.dataset, {setting: name, settingType: type});
    l.append(c, document.createTextNode(label));
    return l;
  }
  if (type === 'multiselect') {
    const fs = el('fieldset', 'field'), grid = el('div', 'check-grid'), chosen = (Array.isArray(value) ? value : String(value ?? '').split(',')).map(String);
    Object.assign(fs.dataset, {setting: name, settingType: type});
    fs.append(el('legend', '', label), grid);
    for (const [key, text] of optionPairs(setting.options)) {
      const l = el('label', 'check'), c = el('input');
      c.type = 'checkbox';
      c.value = String(key);
      c.checked = chosen.includes(String(key));
      l.append(c, document.createTextNode(String(text)));
      grid.append(l);
    }
    return fs;
  }
  const field = el('div', 'field'), l = el('label', '', label);
  let input;
  l.htmlFor = id;
  if (type === 'select') {
    input = el('select');
    for (const [key, text] of optionPairs(setting.options)) { const o = new Option(String(text), String(key)); o.selected = String(value ?? '') === String(key); input.append(o); }
  } else {
    input = el('input');
    const secret = setting.secret || type === 'password';
    input.type = secret ? 'password' : 'text';
    input.autocomplete = secret ? 'new-password' : 'off';
    input.spellcheck = false;
    input.value = secret ? '' : String(value ?? '');
    if (secret) input.dataset.secret = '1';
  }
  input.id = id;
  Object.assign(input.dataset, {setting: name, settingType: type});
  field.append(l, input);
  if (input.dataset.secret) { const hint = el('p', 'hint secret-hint'); hint.id = id + '-hint'; input.setAttribute('aria-describedby', hint.id); field.append(hint); }
  return field;
}
function presetValues() {
  const values = {};
  $$('#preset-settings [data-setting]').forEach(n => {
    values[n.dataset.setting] = n.dataset.settingType === 'multiselect' ? [...n.querySelectorAll('input:checked')].map(c => c.value) : n.type === 'checkbox' ? n.checked : n.value;
  });
  return values;
}
function updateOriginNote() {
  const f = $('#indexer-form'), item = state.editing, note = $('#origin-note'), url = f.elements.url, stock = f.elements.kind.value === 'cardigann';
  const before = item && originOf(item.url), after = originOf(url.value.trim()), changed = !!(before && after && before !== after);
  note.replaceChildren();
  note.hidden = !changed;
  if (changed) note.append(icon('alert'), el('span', '', stock ? `This changes the origin from ${before} to ${after}. Saved preset secrets stay with the original origin, so enter them again below. Saving without them removes the saved values.` : `This changes the origin from ${before} to ${after}. A saved API key or cookie stays with this source and will be sent to the new origin. Replace it if it belongs to the old site.`));
  url.setAttribute('aria-describedby', [!$('#url-privacy-note').hidden && 'url-privacy-note', changed && 'origin-note'].filter(Boolean).join(' '));
  if (!url.getAttribute('aria-describedby')) url.removeAttribute('aria-describedby');
  for (const input of $$('#preset-settings input[data-secret]')) {
    const hint = $('#' + CSS.escape(input.id + '-hint')), reenter = changed && stock;
    input.placeholder = !item ? 'Required if the source needs it' : reenter ? 'Enter again for the new origin' : 'Leave blank to keep the saved value';
    input.classList.toggle('reenter', reenter);
    hint.classList.toggle('reenter', reenter);
    hint.textContent = !item ? 'Stored privately on this server and never shown again.' : reenter ? 'Not kept after an origin change. Enter it again if this source needs it.' : 'Saved secrets are never shown. Leave blank to keep the current value.';
  }
}
function suggestDefinition() {
  const f = $('#indexer-form'), field = f.elements.definition;
  if (state.editing || (field.value.trim() && field.value !== field.dataset.suggested)) return;
  const kind = f.elements.kind.value;
  let value = '';
  if (kind === 'json') value = JSON.stringify({items: '/items', title: '/name', download: '/torrent', size: '/bytes'}, null, 2);
  if (kind === 'html') value = JSON.stringify({rows: 'article', title: 'h2', download: 'a.torrent'}, null, 2);
  field.value = value;
  field.dataset.suggested = value;
  if (value) $('#advanced').open = true;
}
function formError(message, field) {
  const n = $('#indexer-error');
  n.replaceChildren();
  n.hidden = !message;
  if (message) n.append(icon('error'), el('span', '', message));
  if (field) { field.setAttribute('aria-invalid', 'true'); field.focus(); }
}
$('#indexer-form').elements.kind.addEventListener('change', () => {
  suggestDefinition();
  renderPreset($('#indexer-form').elements.kind.value === 'cardigann' ? currentPreset() : null, state.editing?.definition?.settings || {});
});
$('#indexer-form').elements.url.addEventListener('input', updateOriginNote);
$('#indexer-form').addEventListener('input', e => e.target.removeAttribute?.('aria-invalid'));
$('#add-indexer').addEventListener('click', e => openIndexer(null, e.currentTarget));
$('#catalog-search').addEventListener('input', renderCatalog);
$('#catalog-blocked').addEventListener('change', renderCatalog);
$('#step-back').addEventListener('click', () => { showStep('choose'); $('#catalog-search').focus(); });
$('#step-choose').addEventListener('keydown', e => {
  if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
  const stops = [$('#catalog-search'), ...$$('#catalog-list .option')], i = stops.indexOf(document.activeElement);
  if (i < 0) return;
  e.preventDefault();
  stops[Math.max(0, Math.min(stops.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))].focus();
});
$('#indexer-form').addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.currentTarget;
  if (state.step === 'choose') return $('#catalog-list .option')?.click();
  formError('');
  for (const n of [f.elements.name, f.elements.url, f.elements.request_delay_ms, f.elements.retries]) {
    if (n.checkValidity()) continue;
    if (n.closest('details')) $('#advanced').open = true;
    return formError(`${n.labels[0]?.textContent.trim() || n.name}: ${n.validationMessage}`, n);
  }
  let definition = null;
  if (f.elements.kind.value === 'cardigann') {
    if (!state.preset || state.preset.ready !== true) return formError('Choose a supported bundled preset.');
    definition = {builtin_id: state.preset.id, settings: presetValues()};
  }
  try { if (f.elements.kind.value !== 'cardigann' && f.elements.definition.value.trim()) definition = JSON.parse(f.elements.definition.value); } catch {
    $('#advanced').open = true;
    return formError('Definition must be valid JSON.', f.elements.definition);
  }
  const rawCategories = f.elements.categories.value.trim(), categories = rawCategories ? rawCategories.split(',').map(x => x.trim()) : [];
  if (categories.some(x => !/^\d+$/.test(x) || Number(x) > 4294967295)) {
    $('#advanced').open = true;
    return formError('Enter category IDs as whole numbers separated by commas.', f.elements.categories);
  }
  const data = Object.fromEntries(['name', 'kind', 'url', 'api_key', 'cookie'].map(k => [k, f.elements[k].value.trim()]));
  if (!/^https?:\/\//i.test(data.url)) return formError('Use an HTTP or HTTPS endpoint URL.', f.elements.url);
  Object.assign(data, {categories: categories.map(Number), enabled: state.editing?.enabled ?? true, tags: f.elements.tags.value.split(',').map(x => x.trim()).filter(Boolean), request_delay_ms: Number(f.elements.request_delay_ms.value), retries: Number(f.elements.retries.value), definition});
  const editing = state.editing;
  if (editing) {
    if (editing.url_redacted && data.url === editing.url) delete data.url;
    if (!data.api_key) delete data.api_key;
    if (!data.cookie) delete data.cookie;
    if (definition === null) delete data.definition;
  }
  let saved;
  await pending($('#save-indexer'), async () => {
    try { saved = await api('/api/indexers' + (editing ? '/' + encodeURIComponent(editing.id) : ''), {method: editing ? 'PATCH' : 'POST', body: JSON.stringify(data)}); } catch (err) { return formError(describe(err)); }
    await load();
    $('#indexer-dialog').close();
  }, 'Saving…');
  if (!saved) return;
  const item = saved.id ? {id: saved.id, name: data.name} : state.indexers.find(x => editing ? x.id === editing.id : x.name === data.name && x.url === data.url);
  toast(`${data.name} saved. Testing the connection…`, 'info');
  if (!item) return;
  const result = await testIndexer(item, {announce: false});
  if (result) toast(result.ok ? `${data.name} is connected and ready` : `${data.name} was saved, but its connection test failed: ${result.message}`, result.ok ? 'ok' : 'warn');
});
function openGuide(opener) { $('#guide-url').value = endpoint(); openDialog($('#guide-dialog'), opener); }
$('#open-guide').addEventListener('click', e => openGuide(e.currentTarget));
$('#settings-guide').addEventListener('click', e => openGuide(e.currentTarget));
$('#copy-url').addEventListener('click', () => copy(endpoint(), 'Torznab URL copied'));
$('#guide-settings').addEventListener('click', () => go('settings'));

/* Search */
function enabledSources() { return state.indexers.filter(x => x.enabled); }
function renderSourceChips() {
  const box = $('#search-sources'), enabled = enabledSources(), known = new Set(enabled.map(x => x.id)), chips = enabled.map(x => sourceChip(x.id, x.name, false));
  for (const id of state.searchSel) {
    if (known.has(id) || !state.loaded) continue;
    const x = state.indexers.find(i => i.id === id);
    chips.push(sourceChip(id, x ? `${x.name} (paused)` : `Unavailable source ${id}`, true));
  }
  box.replaceChildren(...chips);
  box.classList.toggle('many', chips.length > 12);
  updateSourcesHint();
}
function sourceChip(id, name, missing) {
  const label = el('label', 'chip' + (missing ? ' missing' : '')), input = el('input');
  input.type = 'checkbox';
  input.value = id;
  input.checked = state.searchSel.has(id);
  input.addEventListener('change', () => { input.checked ? state.searchSel.add(id) : state.searchSel.delete(id); updateSourcesHint(); });
  label.title = name;
  label.append(input, icon('check'), el('span', '', name));
  return label;
}
function updateFiltersSummary() {
  const sel = $('#search-category'), n = enabledSources().length, k = state.searchSel.size, custom = $('#search-cat-custom').value.trim();
  const category = sel.value === 'custom' ? (custom ? `Categories ${custom}` : 'Custom categories') : sel.value ? sel.selectedOptions[0].textContent.split(' · ')[0] : 'Any category';
  $('#filters-summary').textContent = `${category} · ${k ? `${num(k)} of ${count(n, 'source')}` : !state.loaded ? 'All sources' : n ? `All ${count(n, 'source')}` : 'No enabled sources'}`;
}
function updateSourcesHint() {
  const hint = $('#search-sources-hint'), n = enabledSources().length, k = state.searchSel.size;
  hint.replaceChildren();
  updateFiltersSummary();
  if (!state.loaded) return hint.append(state.loadError ? 'Sources could not be loaded. A search still uses every enabled source.' : 'Loading sources…');
  if (!n && !k) { const a = el('a', '', 'Add or enable one on Overview'); a.href = '#overview'; return hint.append('No sources are enabled. ', a, '.'); }
  if (!k) return hint.append(`No selection searches all ${count(n, 'enabled source')}.`);
  hint.append(`${num(k)} selected. `, button('Search all enabled sources instead', () => { state.searchSel.clear(); renderSourceChips(); }, {cls: 'btn link'}));
}
function setCategory(cat) {
  const sel = $('#search-category'), custom = $('#search-cat-custom'), known = cat && [...sel.options].some(o => o.value === cat && o.value !== 'custom');
  sel.value = !cat ? '' : known ? cat : 'custom';
  custom.hidden = sel.value !== 'custom';
  custom.value = sel.value === 'custom' ? cat : '';
  custom.removeAttribute('aria-invalid');
  updateFiltersSummary();
}
$('#search-category').addEventListener('change', e => { const custom = $('#search-cat-custom'); custom.hidden = e.target.value !== 'custom'; if (!custom.hidden) custom.focus(); updateFiltersSummary(); });
$('#search-cat-custom').addEventListener('input', updateFiltersSummary);
$('#filters-toggle').addEventListener('click', e => {
  const open = e.currentTarget.getAttribute('aria-expanded') !== 'true';
  e.currentTarget.setAttribute('aria-expanded', String(open));
  $('#search-options').classList.toggle('collapsed', !open);
});
function formParams() {
  const err = $('#search-form-error'), custom = $('#search-cat-custom'), isCustom = $('#search-category').value === 'custom';
  const cat = isCustom ? custom.value.replace(/\s+/g, '') : $('#search-category').value;
  err.hidden = true;
  custom.removeAttribute('aria-invalid');
  if (isCustom && !/^\d+(,\d+)*$/.test(cat)) {
    $('#filters-toggle').setAttribute('aria-expanded', 'true');
    $('#search-options').classList.remove('collapsed');
    err.textContent = 'Enter category IDs as whole numbers separated by commas, for example 2000,5040.';
    err.hidden = false;
    custom.setAttribute('aria-invalid', 'true');
    custom.focus();
    return null;
  }
  const p = new URLSearchParams({q: $('#search-query').value.trim()});
  if (state.searchSel.size) p.set('sources', [...state.searchSel].sort().join(','));
  if (cat) p.set('cat', cat);
  if (state.sort !== 'date') p.set('sort', state.sort);
  return p;
}
function searchKey(p) { return ['q', 'sources', 'cat'].map(k => k + '=' + (p.get(k) || '')).join('&'); }
function updateSearchNav() { for (const a of $$('a[data-nav="search"]')) a.setAttribute('href', state.searchHref || '#search'); }
function syncSearch(params) {
  if (location.hash === state.searchHref) { renderSourceChips(); renderResults(); return; }
  state.searchHref = location.hash;
  updateSearchNav();
  $('#search-query').value = params.get('q') || '';
  state.searchSel = new Set((params.get('sources') || '').split(',').filter(Boolean));
  setCategory(params.get('cat') || '');
  $('#search-form-error').hidden = true;
  state.sort = SORTS[params.get('sort')] ? params.get('sort') : 'date';
  renderSourceChips();
  if (!params.has('q')) {
    state.search.controller?.abort();
    state.search = {status: 'idle'};
    setSearchBusy(false);
    return renderResults();
  }
  if (state.search.key === searchKey(params) && state.search.status !== 'idle') return renderResults();
  runSearch(params);
}
$('#search-form').addEventListener('submit', e => {
  e.preventDefault();
  const params = formParams();
  if (!params) return;
  const href = routeHref('search', params);
  if (href !== location.hash) {
    history.pushState({entry: ++state.entry}, '', href);
    state.lastHash = state.searchHref = location.hash;
    updateSearchNav();
  }
  runSearch(params, {fresh: true});
});
function setSearchBusy(busy) {
  const b = $('#search-submit');
  b.setAttribute('aria-busy', String(busy));
  b.replaceChildren(busy ? el('span', 'spinner') : icon('search'), el('span', '', busy ? 'Searching…' : 'Search'));
}
function announce(text) { const n = $('#search-live'); n.textContent = ''; requestAnimationFrame(() => { n.textContent = text; }); }
async function runSearch(params, {fresh = false} = {}) {
  const key = searchKey(params), cached = !fresh && state.searchCache.get(key);
  state.search.controller?.abort();
  if (cached && Date.now() - cached.at < CACHE_AGE) {
    state.search = {...cached.search, filter: '', shown: RESULT_STEP};
    setSearchBusy(false);
    return renderResults();
  }
  const sourceCount = (params.get('sources') || '').split(',').filter(Boolean).length || (state.loaded ? enabledSources().length : null);
  if (sourceCount === 0) {
    state.search = {key, params, status: 'nosources'};
    setSearchBusy(false);
    return renderResults();
  }
  const s = state.search = {key, params, status: 'pending', startedAt: Date.now(), controller: new AbortController(), sourceCount, filter: '', shown: RESULT_STEP};
  setSearchBusy(true);
  renderResults();
  announce(sourceCount ? `Searching ${count(sourceCount, 'source')}…` : 'Searching…');
  try {
    const data = await api('/api/search?' + new URLSearchParams({q: params.get('q') || '', indexers: params.get('sources') || '', cat: params.get('cat') || ''}), {signal: s.controller.signal});
    if (state.search !== s) return;
    s.response = {results: data?.results || [], errors: data?.errors || [], elapsed_ms: data?.elapsed_ms ?? Date.now() - s.startedAt, cached: data?.cached || 0};
    s.sourceCount ??= state.loaded ? enabledSources().length : null;
    s.status = s.sourceCount === 0 && !s.response.results.length && !s.response.errors.length ? 'nosources' : 'complete';
    state.lastSearchMs = s.response.elapsed_ms;
    state.searchCache.set(key, {at: Date.now(), search: {key, params, status: s.status, sourceCount: s.sourceCount, response: s.response}});
    while (state.searchCache.size > 12) state.searchCache.delete(state.searchCache.keys().next().value);
    const failed = new Set(s.response.errors.map(x => x.indexer_id || x.indexer)).size;
    announce(`${count(s.response.results.length, 'release')} found.` + (failed ? ` ${count(failed, 'source')} failed.` : ''));
    load({quiet: true});
  } catch (e) {
    if (state.search !== s) return;
    s.status = e.name === 'AbortError' ? 'cancelled' : 'failed';
    s.error = e;
    announce(s.status === 'cancelled' ? 'Search cancelled.' : 'Search failed. ' + describe(e));
  } finally {
    if (state.search === s) {
      s.controller = null;
      setSearchBusy(false);
      renderResults();
    }
  }
}
function retrySearch() { const p = state.search.params; if (p) runSearch(p, {fresh: true}); }
function renderResults() {
  const box = $('#search-results'), s = state.search;
  clearInterval(state.searchTimer);
  box.replaceChildren();
  if (s.status === 'idle') {
    const n = enabledSources().length, ready = state.loaded && n;
    return box.append(panelWith(stateBox({ic: 'search', title: state.loaded && !n ? 'No sources to search yet' : 'Search across every enabled source', text: !state.loaded ? 'Enter a title, keywords or a release name. Results from all sources appear together.' : ready ? `Enter a title, keywords or a release name to search ${count(n, 'enabled source')} at once. Press / to jump to the search box.` : 'Add a source or enable a paused one, then come back to search.', actions: state.loaded && !n ? [{label: 'Add a source', ic: 'plus', cls: 'primary', fn: b => openIndexer(null, b)}] : []})));
  }
  if (s.status === 'nosources') {
    return box.append(panelWith(stateBox({tone: 'warn', ic: 'alert', title: 'No enabled sources were searched', text: 'This is not a “no matches” result: there was nothing to search. Add a source or enable a paused one, then search again.', actions: [{label: 'Go to Overview', fn: () => go('overview')}, {label: 'Add a source', ic: 'plus', cls: 'primary', fn: b => openIndexer(null, b)}]})));
  }
  if (s.status === 'pending') {
    const timeout = state.settings?.request_timeout_seconds, card = stateBox({ic: 'spinner', title: s.sourceCount ? `Searching ${count(s.sourceCount, 'source')}…` : 'Searching your sources…', text: ' ', actions: [{label: 'Cancel search', ic: 'close', fn: () => s.controller?.abort()}]}), p = card.querySelector('p');
    const tick = () => { p.textContent = `${Math.floor((Date.now() - s.startedAt) / 1000)} s elapsed` + (timeout ? ` · each source has up to ${timeout} s to respond` : ''); };
    tick();
    state.searchTimer = setInterval(tick, 1000);
    const panel = panelWith(card);
    panel.setAttribute('aria-busy', 'true');
    return box.append(panel);
  }
  if (s.status === 'failed' || s.status === 'cancelled') {
    const failed = s.status === 'failed';
    return box.append(panelWith(stateBox({tone: failed ? 'bad' : '', ic: failed ? 'error' : 'info', title: failed ? 'The search could not run' : 'Search cancelled', text: failed ? s.error.message : 'Results from the cancelled search were discarded. Sources that already received the request may still finish it.', code: failed && s.error.status ? `HTTP ${s.error.status}` : null, actions: [{label: failed ? 'Try again' : 'Search again', ic: 'refresh', cls: 'primary', fn: retrySearch}]})));
  }
  renderComplete(box, s);
}
function renderComplete(box, s) {
  const {results, errors, elapsed_ms, cached} = s.response, q = s.params.get('q') || '';
  const failed = new Set(errors.map(x => x.indexer_id || x.indexer)).size, total = Math.max(s.sourceCount || 0, failed), allFailed = total > 0 && failed >= total;
  const head = el('div', 'results-head'), h2 = el('h2', '', results.length ? count(results.length, 'release') : 'No releases'), meta = el('p', 'results-meta');
  h2.id = 'results-heading';
  h2.tabIndex = -1;
  const parts = [q ? `for “${q}”` : 'latest releases', total ? `${num(total - failed)} of ${count(total, 'source')} responded` : null, `in ${ms(elapsed_ms)}`, cached ? `${num(cached)} from cache` : null];
  meta.textContent = parts.filter(Boolean).join(' · ');
  head.append(h2, meta);
  box.append(head);
  if (s.stale) {
    const b = banner('warn', 'Download links in these results no longer work', 'The integration key changed after this search. Search again to get fresh links.');
    b.querySelector('.banner-body').append(button('Search again', retrySearch, {cls: 'btn link'}));
    box.append(b);
  }
  if (errors.length) box.append(sourceErrors(errors, failed, total, results.length));
  if (!results.length) {
    const panel = el('div', 'panel');
    panel.append(!errors.length ? stateBox({ic: 'search', title: q ? `Nothing matched “${q}”` : 'No releases returned', text: `${total ? (total === 1 ? 'The source' : `All ${num(total)} sources`) : 'Every source'} responded without matching releases. Try a broader query, another category or different sources.`})
      : allFailed ? stateBox({tone: 'bad', ic: 'error', title: total === 1 ? 'The source failed' : 'Every searched source failed', text: 'No source answered this search, so this is not a “no matches” result. Review the errors above, fix the sources, then try again.', actions: [{label: 'Try again', ic: 'refresh', cls: 'primary', fn: retrySearch}]})
      : stateBox({tone: 'warn', ic: 'alert', title: 'No releases from the sources that responded', text: `${count(failed, 'source')} failed, so matching releases may be missing. Fix them or try again.`, actions: [{label: 'Try again', ic: 'refresh', fn: retrySearch}]}));
    return box.append(panel);
  }
  const panel = el('div', 'panel'), bar = el('div', 'results-toolbar'), filterLabel = el('label', 'input-icon'), filter = el('input'), info = el('p', 'results-count'), sortLabel = el('label', 'sort', 'Sort'), sort = el('select');
  Object.assign(filter, {type: 'search', id: 'result-filter', placeholder: 'Filter these results', autocomplete: 'off', value: s.filter});
  filter.setAttribute('aria-label', 'Filter these results');
  filter.addEventListener('input', () => { s.filter = filter.value; s.shown = RESULT_STEP; renderResultRows(); });
  filterLabel.append(icon('filter'), filter);
  info.id = 'results-count';
  info.setAttribute('aria-live', 'polite');
  sort.id = 'result-sort';
  for (const [k, v] of Object.entries(SORTS)) sort.append(new Option(v.label, k));
  sort.value = state.sort;
  sort.addEventListener('change', () => setSort(sort.value));
  sortLabel.append(sort);
  bar.append(filterLabel, info, sortLabel);
  const table = el('table', 'results-table'), caption = el('caption', 'sr-only'), thead = el('thead'), tr = el('tr'), body = el('tbody');
  caption.id = 'results-caption';
  for (const [label, field, cls] of [['Release', 'title', ''], ['Size', 'size', 'num'], ['Seeders', 'seeders', 'num'], ['Leechers', null, 'num'], ['Age', 'date', ''], ['Download', null, '']]) {
    const th = el('th', cls);
    th.scope = 'col';
    if (field) {
      const b = el('button', 'th-sort');
      b.type = 'button';
      b.dataset.field = field;
      b.append(el('span', '', label), icon('sort-down'));
      b.addEventListener('click', () => setSort(toggledSort(field)));
      th.append(b);
    } else th.append(label === 'Download' ? el('span', 'sr-only', label) : label);
    tr.append(th);
  }
  body.id = 'results-body';
  thead.append(tr);
  table.append(caption, thead, body);
  const more = el('div', 'results-more');
  more.id = 'results-more';
  panel.append(bar, table, more);
  box.append(panel, el('p', 'footnote', 'Download links expire after two hours, when their source is edited or removed, or when Rackett restarts. Search again for fresh links. Magnet links open in your torrent app.'));
  renderResultRows();
}
function sourceErrors(errors, failed, total, found) {
  const b = banner(found ? 'warn' : failed >= total ? 'bad' : 'warn', found ? `${num(failed)} of ${count(total, 'source')} failed. These results may be incomplete.` : `${num(failed)} of ${count(total, 'source')} failed.`), list = el('ul');
  b.classList.add('source-errors');
  for (const e of errors) {
    const li = el('li'), x = state.indexers.find(i => i.id === e.indexer_id);
    li.append(el('strong', '', e.indexer || e.indexer_id || 'Source'), el('span', '', e.message || 'The source returned an error.'));
    if (x) li.append(button('Edit source', b2 => openIndexer(x, b2), {cls: 'btn link', aria: `Edit ${x.name}`}));
    list.append(li);
  }
  b.querySelector('.banner-body').append(list);
  return b;
}
function toggledSort(field) {
  const cur = SORTS[state.sort];
  if (cur.field === field) return Object.keys(SORTS).find(k => SORTS[k].field === field && SORTS[k].dir === -cur.dir);
  return field;
}
function setSort(key) {
  state.sort = SORTS[key] ? key : 'date';
  const r = parseRoute();
  if (r.name === 'search' && r.params.has('q')) {
    state.sort === 'date' ? r.params.delete('sort') : r.params.set('sort', state.sort);
    history.replaceState(history.state, '', routeHref('search', r.params));
    state.lastHash = state.searchHref = location.hash;
    updateSearchNav();
  }
  if ($('#result-sort')) $('#result-sort').value = state.sort;
  state.search.shown = RESULT_STEP;
  renderResultRows();
}
function sortRows(rows, key) {
  const {field, dir} = SORTS[key] || SORTS.date;
  const value = {date: r => timeOf(r.published), seeders: r => r.seeders ?? null, size: r => r.size ?? null, title: r => r.title || ''}[field];
  return rows.map((r, i) => [value(r), i, r]).sort((a, b) => {
    if (a[0] === null || b[0] === null) return a[0] === b[0] ? a[1] - b[1] : a[0] === null ? 1 : -1;
    return (field === 'title' ? collator.compare(a[0], b[0]) : a[0] - b[0]) * dir || a[1] - b[1];
  }).map(x => x[2]);
}
function renderResultRows() {
  const s = state.search, body = $('#results-body');
  if (!body || s.status !== 'complete') return;
  const all = s.response.results, terms = s.filter.trim().toLowerCase().split(/\s+/).filter(Boolean);
  const rows = sortRows(terms.length ? all.filter(r => terms.every(t => `${r.title} ${r.indexer}`.toLowerCase().includes(t))) : all, state.sort), {field, dir} = SORTS[state.sort];
  for (const b of $$('#search-results .th-sort')) {
    const th = b.parentElement, active = b.dataset.field === field;
    active ? th.setAttribute('aria-sort', dir < 0 ? 'descending' : 'ascending') : th.removeAttribute('aria-sort');
    b.querySelector('svg').replaceWith(icon(active && dir > 0 ? 'sort-up' : 'sort-down'));
  }
  $('#results-caption').textContent = `Search results, ${SORTS[state.sort].label.toLowerCase()}`;
  $('#results-count').textContent = terms.length ? `${num(rows.length)} of ${num(all.length)} match` : '';
  body.replaceChildren(...rows.slice(0, s.shown).map(resultRow));
  if (!rows.length) {
    const tr = el('tr'), td = el('td');
    td.colSpan = 6;
    td.append(stateBox({ic: 'filter', title: 'No results match this filter', actions: [{label: 'Clear filter', fn: () => { s.filter = ''; $('#result-filter').value = ''; renderResultRows(); $('#result-filter').focus(); }}]}));
    tr.append(td);
    body.append(tr);
  }
  const more = $('#results-more'), focusMore = document.activeElement?.dataset?.focusKey === 'results:more';
  more.replaceChildren();
  more.hidden = rows.length <= RESULT_STEP;
  if (rows.length > s.shown) {
    more.append(el('span', '', `Showing ${num(s.shown)} of ${num(rows.length)}`), button(`Show ${num(Math.min(RESULT_STEP, rows.length - s.shown))} more`, () => {
      const first = s.shown;
      s.shown += RESULT_STEP;
      renderResultRows();
      if (!$('#results-more button')) $(`#results-body tr:nth-child(${first + 1}) a`)?.focus();
    }, {cls: 'btn sm', key: 'results:more'}));
    if (focusMore) more.querySelector('button').focus();
  } else if (!more.hidden) more.append(el('span', '', `Showing all ${num(rows.length)}`));
}
function resultRow(r) {
  const tr = el('tr'), name = el('td', 'r-name'), details = safeUrl(r.details), titleText = r.title || 'Untitled release', meta = el('div', 'r-meta');
  let title;
  if (details && !details.startsWith('magnet:')) {
    title = el('a', 'r-title', titleText);
    Object.assign(title, {href: details, target: '_blank', rel: 'noopener noreferrer', title: 'Open the release page in a new tab'});
    title.append(icon('external'));
  } else title = el('span', 'r-title', titleText);
  meta.append(el('span', '', r.indexer || r.indexer_id || 'Unknown source'));
  if (r.categories?.length) { const c = el('span', '', [...new Set(r.categories.map(categoryName))].join(', ')); c.title = 'Categories ' + r.categories.join(', '); meta.append(c); }
  name.append(title, meta);
  const cell = (text, label, cls = 'num') => { const td = el('td', cls, text); td.dataset.label = label; return td; };
  const leech = r.peers !== null && r.peers !== undefined && r.seeders !== null && r.seeders !== undefined && r.peers >= r.seeders ? num(r.peers - r.seeders) : '—';
  const t = timeOf(r.published), age = cell(null, 'Age', 'r-age'), get = el('td', 'r-get'), href = safeUrl(r.link);
  if (t === null) age.textContent = '—'; else { const time = el('time', '', ago(t)); time.dateTime = new Date(t).toISOString(); time.title = stamp(t); age.append(time); }
  if (href) {
    const a = el('a', 'btn sm'), magnet = href.startsWith('magnet:');
    a.href = href;
    a.rel = 'noopener noreferrer';
    if (!magnet) a.target = '_blank';
    a.append(icon(magnet ? 'magnet' : 'download'), el('span', '', magnet ? 'Magnet' : 'Download'), el('span', 'sr-only', ' ' + titleText));
    get.append(a);
  } else get.append(el('span', 'r-dim', 'Unavailable'));
  tr.append(name, cell(size(r.size), 'Size'), cell(r.seeders === null || r.seeders === undefined ? '—' : num(r.seeders), 'Seeders'), cell(leech, 'Leechers'), age, get);
  return tr;
}

/* Activity */
async function loadActivity() {
  if (state.activityStatus === 'loading') return;
  state.activityStatus = 'loading';
  renderActivity();
  try {
    const data = await api('/api/activity');
    state.activity = Array.isArray(data) ? data : data?.activity || [];
    state.activityAt = Date.now();
    state.activityError = null;
    state.activityStatus = 'ready';
  } catch (e) {
    state.activityError = e;
    state.activityStatus = 'failed';
  }
  renderActivity();
}
function dayLabel(t) {
  const d = new Date(t), now = new Date(), start = x => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime(), diff = Math.round((start(now) - start(d)) / 864e5);
  return diff === 0 ? 'Today' : diff === 1 ? 'Yesterday' : d.toLocaleDateString(undefined, {weekday: 'long', month: 'long', day: 'numeric', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric'});
}
function renderActivity() {
  const box = $('#activity-list'), f = state.activityFilter, loading = state.activityStatus === 'loading';
  for (const b of $$('[data-activity]')) b.setAttribute('aria-pressed', String(b.dataset.activity === f));
  $('#activity-updated').textContent = loading ? 'Refreshing…' : state.activityAt ? `Updated ${new Date(state.activityAt).toLocaleTimeString(undefined, {timeStyle: 'short'})}` : '';
  box.setAttribute('aria-busy', String(loading));
  if (!state.activityAt && loading) return box.replaceChildren(stateBox({ic: 'spinner', title: 'Loading activity…'}));
  if (!state.activityAt && state.activityStatus === 'failed') return box.replaceChildren(stateBox({tone: 'bad', ic: 'error', title: 'Activity could not be loaded', text: state.activityError.message, code: state.activityError.status ? `HTTP ${state.activityError.status}` : null, actions: [{label: 'Try again', ic: 'refresh', fn: loadActivity}]}));
  const nodes = [], items = state.activity.filter(a => f === 'all' || a.kind === f);
  if (state.activityStatus === 'failed') nodes.push(banner('bad', 'Could not refresh activity', describe(state.activityError) + '. Showing the last events received.'));
  if (!items.length) nodes.push(stateBox({ic: 'activity', title: state.activity.length ? 'No events of this kind yet' : 'No activity yet', text: state.activity.length ? 'Choose All to see every event.' : 'Searches and source changes appear here as they happen.'}));
  let day = null, list = null;
  for (const a of items) {
    const ts = a.timestamp || a.created_at || a.time, t = timeOf(ts), label = t === null ? 'Earlier' : dayLabel(t);
    if (label !== day) { day = label; nodes.push(el('h2', 'activity-day', label)); list = el('ol'); nodes.push(list); }
    list.append(activityRow(a, ts, t));
  }
  box.replaceChildren(...nodes);
}
function activityRow(a, ts, t) {
  const kind = a.kind || 'event', li = el('li', 'activity-row'), badge = el('span', 'activity-icon ' + kind), body = el('div'), meta = el('p', 'activity-meta'), source = a.indexer && state.indexers.find(x => x.id === a.indexer);
  badge.append(icon(kind === 'search' ? 'search' : kind === 'configuration' ? 'settings' : 'activity'));
  const message = a.message || a.description || a.action || JSON.stringify(a);
  meta.append(el('span', '', kind === 'search' ? 'Search' : kind === 'configuration' ? 'Configuration' : kind));
  if (source && !message.includes(source.name)) meta.append(el('span', '', source.name));
  body.append(el('p', 'activity-msg', message), meta);
  const time = el('time', '', t === null ? String(ts || '') : ago(t) === 'just now' ? 'just now' : ago(t) + ' ago');
  if (t !== null) { time.dateTime = new Date(t).toISOString(); time.title = stamp(t); }
  li.append(badge, body, time);
  return li;
}
$$('[data-activity]').forEach(b => b.addEventListener('click', () => { state.activityFilter = b.dataset.activity; renderActivity(); }));
$('#refresh-activity').addEventListener('click', e => pending(e.currentTarget, loadActivity, 'Refreshing…'));

/* Settings */
const SETTING_FIELDS = ['cache_ttl_seconds', 'cache_stale_seconds', 'request_timeout_seconds'];
async function loadSettings() {
  try { state.settings = await api('/api/settings'); state.settingsError = null; $('#settings-error').hidden = true; } catch (e) { state.settingsError = e; }
  renderSettings();
  renderStats();
}
function settingsDirty() { const f = $('#settings-form'), s = state.settings; return !!s && SETTING_FIELDS.some(k => f.elements[k].value !== String(s[k] ?? '')); }
function renderSettings() {
  const s = state.settings, f = $('#settings-form'), err = $('#settings-error');
  for (const k of SETTING_FIELDS) f.elements[k].disabled = !s;
  if (!s) {
    err.hidden = !state.settingsError;
    err.textContent = state.settingsError ? 'Settings could not be loaded: ' + describe(state.settingsError) : '';
  } else if (!settingsDirty() || !f.dataset.touched) {
    for (const k of SETTING_FIELDS) f.elements[k].value = s[k] ?? '';
    delete f.dataset.touched;
  }
  $('#save-settings').disabled = !settingsDirty();
  $('#set-capacity').textContent = `${s?.max_concurrent ?? 8} simultaneous source requests`;
  $('#settings-key').value = s?.api_key || '';
  $('#settings-url').value = endpoint();
  const facts = $('#server-facts');
  facts.replaceChildren();
  for (const [k, v] of [['Listening on', s ? `127.0.0.1:${s.port}` : '—'], ['Public URL', s ? s.public_url || 'Not configured. Rackett is reachable on this computer only.' : '—'], ['Opened from', location.origin], ['Download links', 'Expire after two hours, source edits/removal, or restart']]) facts.append(el('dt', '', k), el('dd', '', v));
  renderCacheFacts();
}
function renderCacheFacts() {
  const s = state.stats, ttl = state.settings?.cache_ttl_seconds;
  $('#cache-facts').textContent = !state.loaded ? 'Cache details are unavailable until sources load.' : `${count(s.cache_entries ?? 0, 'stored result set')} · ${count(s.cache_hits ?? 0, 'cache hit')} since start` + (ttl === undefined ? '' : ttl ? ` · results kept for ${ttl} s` : ' · caching is off');
}
function maskKey() { const n = $('#settings-key'); if (n.type !== 'password') $('#reveal-key').click(); }
$('#settings-form').addEventListener('input', e => {
  const f = e.currentTarget, st = $('#settings-status');
  f.dataset.touched = '1';
  e.target.removeAttribute('aria-invalid');
  $('#settings-error').hidden = true;
  $('#save-settings').disabled = !settingsDirty();
  st.textContent = 'Saving also clears cached search results.';
  st.classList.remove('saved');
});
$('#settings-form').addEventListener('submit', async e => {
  e.preventDefault();
  const f = e.currentTarget, err = $('#settings-error');
  err.hidden = true;
  for (const n of SETTING_FIELDS.map(k => f.elements[k])) {
    if (n.checkValidity()) continue;
    n.setAttribute('aria-invalid', 'true');
    err.textContent = `${n.labels[0].textContent}: ${n.validationMessage}`;
    err.hidden = false;
    return n.focus();
  }
  const data = Object.fromEntries(SETTING_FIELDS.map(k => [k, Number(f.elements[k].value)]));
  await pending($('#save-settings'), async () => {
    try { state.settings = await api('/api/settings', {method: 'PATCH', body: JSON.stringify(data)}); if (!state.settings) state.settings = await api('/api/settings'); } catch (x) {
      err.textContent = 'Not saved: ' + describe(x);
      err.hidden = false;
      return;
    }
    delete f.dataset.touched;
    renderSettings();
    const st = $('#settings-status');
    st.textContent = 'Saved. Cached search results were cleared.';
    st.classList.add('saved');
    toast('Search behavior saved');
    load({quiet: true});
  }, 'Saving…');
  $('#save-settings').disabled = !settingsDirty();
});
$('#reveal-key').addEventListener('click', e => {
  const n = $('#settings-key'), show = n.type === 'password', b = e.currentTarget;
  n.type = show ? 'text' : 'password';
  b.setAttribute('aria-pressed', String(show));
  b.replaceChildren(icon(show ? 'eye-off' : 'eye'), el('span', '', show ? 'Hide' : 'Show'));
});
$('#copy-key').addEventListener('click', () => copy(state.settings?.api_key || '', 'API key copied'));
$('#copy-settings-url').addEventListener('click', () => copy(endpoint(), 'Torznab URL copied'));
$('#rotate-key').addEventListener('click', async e => {
  const b = e.currentTarget;
  if (!await confirmAction('Rotate the integration key?', 'The current key stops working immediately. Every connected app needs the new key, and download links from earlier searches stop working.', 'Rotate key')) return;
  await pending(b, async () => {
    try { await api('/api/settings/rotate-key', {method: 'POST'}); state.settings = await api('/api/settings'); } catch (x) { return toast('Could not rotate the key: ' + describe(x), 'bad'); }
    state.searchCache.clear();
    if (state.search.status === 'complete') state.search.stale = true;
    renderSettings();
    toast('New API key created. Update your connected apps.');
  }, 'Rotating…');
});
$('#clear-cache').addEventListener('click', e => pending(e.currentTarget, async () => {
  try { await api('/api/cache', {method: 'DELETE'}); } catch (x) { return toast('Could not clear the cache: ' + describe(x), 'bad'); }
  toast('Search cache cleared');
  await load({quiet: true});
}, 'Clearing…'));

/* Start */
if (history.state?.dialog) history.replaceState({entry: 0}, '', location.href);
applyRoute();
renderOverview();
renderSettings();
Promise.all([load(), loadSettings()]).then(() => { if (state.view === 'search' && state.search.status === 'idle') renderResults(); });
