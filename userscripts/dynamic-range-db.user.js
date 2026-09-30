// ==UserScript==
// @name         Dynamic Range DB ⇄ MusicBrainz (RustyDisc)
// @namespace    https://github.com/WB2024/DiscCTL
// @version      1.0.0
// @description  Shows dynamic range (DR) from dr.loudness-war.info on MusicBrainz release pages, adds MusicBrainz links to the Dynamic Range DB, and pre-fills the DB's upload form from a MusicBrainz release or from RustyDisc.
// @author       RustyDisc
// @license      MIT
// @homepageURL  https://github.com/WB2024/DiscCTL
// @supportURL   https://github.com/WB2024/DiscCTL/issues
// @updateURL    https://raw.githubusercontent.com/WB2024/DiscCTL/main/userscripts/dynamic-range-db.user.js
// @downloadURL  https://raw.githubusercontent.com/WB2024/DiscCTL/main/userscripts/dynamic-range-db.user.js
// @match        https://musicbrainz.org/release/*
// @match        https://beta.musicbrainz.org/release/*
// @match        https://dr.loudness-war.info/*
// @grant        GM_xmlhttpRequest
// @grant        GM_getValue
// @grant        GM_setValue
// @connect      dr.loudness-war.info
// @connect      musicbrainz.org
// @run-at       document-idle
// ==/UserScript==

(function () {
  'use strict';

  const DR = 'https://dr.loudness-war.info';
  const MB = location.hostname.endsWith('musicbrainz.org') ? location.origin : 'https://musicbrainz.org';
  const DAY = 24 * 3600 * 1000;

  // ── helpers ────────────────────────────────────────────────────────────────
  const norm = s => (s || '').toLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '').replace(/\([^)]*\)|\[[^\]]*\]/g, ' ').replace(/[^a-z0-9]+/g, ' ').trim();
  const digits = s => (s || '').replace(/[^0-9a-z]/gi, '').toLowerCase();
  const el = (tag, attrs = {}, ...kids) => {
    const e = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) { if (k === 'style') e.style.cssText = v; else if (k === 'text') e.textContent = v; else e.setAttribute(k, v); }
    kids.forEach(k => e.append(k));
    return e;
  };
  const gm = (url) => new Promise((resolve, reject) => {
    const req = typeof GM_xmlhttpRequest === 'function' ? GM_xmlhttpRequest : (typeof GM !== 'undefined' && GM.xmlHttpRequest);
    if (!req) return reject(new Error('GM_xmlhttpRequest is not available'));
    req({ method: 'GET', url, timeout: 20000, onload: r => (r.status >= 200 && r.status < 300 ? resolve(r.responseText) : reject(new Error('HTTP ' + r.status))), onerror: () => reject(new Error('network error')), ontimeout: () => reject(new Error('timed out')) });
  });
  const store = {
    get: (k) => { try { const v = JSON.parse(GM_getValue(k, 'null')); return v && Date.now() - v.t < DAY ? v.d : null; } catch { return null; } },
    set: (k, d) => { try { GM_setValue(k, JSON.stringify({ t: Date.now(), d })); } catch { /* caching is optional */ } },
  };
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const b64 = s => btoa(unescape(encodeURIComponent(s)));
  const unb64 = s => decodeURIComponent(escape(atob(s)));

  // ── the Dynamic Range DB ───────────────────────────────────────────────────
  const parseList = (html) => {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const rows = [...doc.querySelectorAll('tbody tr')].map(tr => {
      const td = [...tr.querySelectorAll('td')];
      const a = tr.querySelector('a[href*="/album/view/"]');
      if (!a || td.length < 8) return null;
      const n = i => td[i].textContent.trim();
      return { id: a.getAttribute('href').split('/').pop(), artist: n(0), album: a.textContent.trim(), year: n(2), dr: n(3), min: n(4), max: n(5), codec: n(6), source: n(7) };
    }).filter(Boolean);
    const next = doc.querySelector('a[rel="next"], li.next a, .pagination a[aria-label*="Next" i]');
    return { rows, hasNext: !!next };
  };
  const listArtist = async (artist) => {
    const key = 'list:' + norm(artist);
    const cached = store.get(key);
    if (cached) return cached;
    let all = [];
    for (let page = 1; page <= 6; page++) {
      const url = page === 1 ? `${DR}/?artist=${encodeURIComponent(artist)}` : `${DR}/album/list/${page}?artist=${encodeURIComponent(artist)}`;
      const { rows, hasNext } = parseList(await gm(url));
      all = all.concat(rows);
      if (!hasNext || !rows.length) break;
      await sleep(600); // be gentle with a volunteer-run site
    }
    store.set(key, all);
    return all;
  };
  const detail = async (id) => {
    const key = 'view:' + id;
    const cached = store.get(key);
    if (cached) return cached;
    const doc = new DOMParser().parseFromString(await gm(`${DR}/album/view/${id}`), 'text/html');
    const d = {};
    doc.querySelectorAll('table tr').forEach(tr => { const th = tr.querySelector('th'), td = tr.querySelector('td'); if (th && td) d[th.textContent.trim().toLowerCase()] = td.textContent.trim(); });
    const out = { barcode: d['bar code'] || '', catalog: d['catalog number'] || '', label: d['label'] || '', country: d['country'] || '', comment: d['comment'] || '' };
    store.set(key, out);
    return out;
  };

  const badge = (n) => {
    const v = parseInt(n, 10);
    const color = v >= 14 ? '#2e9d57' : v >= 11 ? '#6aa84f' : v >= 8 ? '#c9a227' : '#c0392b';
    return el('span', { style: `display:inline-block;min-width:1.9em;text-align:center;padding:0 .35em;border-radius:3px;font-weight:700;color:#fff;background:${color}`, text: 'DR' + (v || n) });
  };

  // ── MusicBrainz release page ───────────────────────────────────────────────
  async function onMusicBrainz() {
    const m = location.pathname.match(/^\/release\/([0-9a-f-]{36})/i);
    if (!m) return;
    const mbid = m[1];
    const side = document.querySelector('#sidebar') || document.querySelector('#content');
    if (!side || document.getElementById('rdb-panel')) return;
    const panel = el('div', { id: 'rdb-panel' });
    panel.append(el('h2', { text: 'Dynamic Range DB' }), el('p', { text: 'Looking up dynamic range…', style: 'color:#777' }));
    side.prepend(panel);
    const fill = (...nodes) => { panel.innerHTML = ''; panel.append(el('h2', { text: 'Dynamic Range DB' }), ...nodes); };

    let rel;
    try { rel = JSON.parse(await gm(`${MB}/ws/2/release/${mbid}?inc=labels+artist-credits&fmt=json`)); }
    catch (e) { return fill(el('p', { text: 'Could not read this release from MusicBrainz (' + e.message + ').', style: 'color:#c0392b' })); }
    const artist = (rel['artist-credit'] || []).map(c => c.name + (c.joinphrase || '')).join('').trim() || (rel['artist-credit']?.[0]?.artist?.name || '');
    const first = (rel['artist-credit'] || [])[0]?.artist?.name || artist;
    const info = {
      artist, album: rel.title, year: (rel.date || '').slice(0, 4), barcode: rel.barcode || '', country: rel.country || '',
      label: (rel['label-info'] || []).map(l => l.label?.name).filter(Boolean)[0] || '',
      catalog: (rel['label-info'] || []).map(l => l['catalog-number']).filter(Boolean)[0] || '',
      mb: `${MB}/release/${mbid}`,
    };
    const submit = el('a', { href: `${DR}/album/add#rdb=${b64(JSON.stringify({ artist: info.artist, album: info.album, year: info.year, codec: 'lossless', source: 'cdda', label: info.label, catalogNumber: info.catalog, barCode: info.barcode, link: info.mb, country: info.country }))}`, target: '_blank', rel: 'noopener', text: 'Submit this release to the Dynamic Range DB →' });
    const search = el('a', { href: `${DR}/?artist=${encodeURIComponent(first)}`, target: '_blank', rel: 'noopener', text: `All ${first} albums on the Dynamic Range DB →` });

    let rows;
    try { rows = await listArtist(first); }
    catch (e) { return fill(el('p', { text: 'The Dynamic Range DB could not be reached (' + e.message + ').', style: 'color:#c0392b' }), el('p', {}, search)); }

    const want = norm(info.album);
    let cands = rows.filter(r => { const t = norm(r.album); return t && (t === want || t.startsWith(want) || want.startsWith(t)); });
    // Edition details (barcode, catalogue number) say which pressing is which; fetch them for the few candidates.
    for (const c of cands.slice(0, 8)) {
      try { Object.assign(c, await detail(c.id)); } catch { /* the list row is still useful */ }
      await sleep(300);
    }
    const score = c => (info.barcode && digits(c.barcode) === digits(info.barcode) ? 4 : 0) + (info.catalog && digits(c.catalog) === digits(info.catalog) ? 3 : 0) + (c.year && c.year === info.year ? 1 : 0) + (norm(c.album) === want ? 1 : 0);
    cands.sort((a, b) => score(b) - score(a) || String(b.year).localeCompare(String(a.year)));

    if (!cands.length) return fill(el('p', { text: `No entry for “${info.album}” yet.`, style: 'color:#777' }), el('p', {}, submit), el('p', {}, search));
    const ul = el('ul', { style: 'list-style:none;margin:0;padding:0' });
    cands.slice(0, 10).forEach(c => {
      const exact = (info.barcode && digits(c.barcode) === digits(info.barcode)) ? 'same barcode' : (info.catalog && digits(c.catalog) === digits(info.catalog)) ? 'same catalogue number' : '';
      ul.append(el('li', { style: 'margin:0 0 .5em' },
        badge(c.dr), ' ', el('span', { style: 'color:#777', text: `(${c.min}–${c.max})` }), ' ',
        el('a', { href: `${DR}/album/view/${c.id}`, target: '_blank', rel: 'noopener', text: c.album }),
        el('span', { style: 'color:#777', text: ` · ${c.year} · ${c.codec} · ${c.source}${c.label ? ' · ' + c.label : ''}` }),
        exact ? el('span', { style: 'color:#2e9d57;font-weight:600', text: ' ✓ ' + exact }) : ''));
    });
    const note = el('p', { style: 'color:#777;font-size:.85em', text: 'DR = how far the peaks rise above the loud parts. 14+ is excellent; under 8 is a loudness-war master. Different pressings can differ a lot.' });
    fill(ul, note, el('p', {}, submit), el('p', {}, search));
  }

  // ── Dynamic Range DB pages ─────────────────────────────────────────────────
  const mbSearch = (artist, album) => `${MB}/search?type=release&method=advanced&query=${encodeURIComponent(`artist:"${artist}" AND release:"${album}"`)}`;
  const mbLink = (href, text = 'MusicBrainz') => el('a', { href, target: '_blank', rel: 'noopener', text, style: 'margin-left:.5em;font-size:.85em', title: 'Find this release on MusicBrainz' });

  function onListOrView() {
    // list pages: a MusicBrainz link beside every album
    document.querySelectorAll('tbody tr').forEach(tr => {
      const td = tr.querySelectorAll('td');
      const a = tr.querySelector('a[href*="/album/view/"]');
      if (!a || td.length < 3 || tr.querySelector('.rdb-mb')) return;
      const l = mbLink(mbSearch(td[0].textContent.trim(), a.textContent.trim().replace(/\s*\([^)]*\)\s*$/, '')));
      l.className = 'rdb-mb';
      td[1].append(l);
    });
    // detail page: search by barcode or catalogue number when the entry has one, which finds the exact edition
    const rows = {};
    document.querySelectorAll('table tr').forEach(tr => { const th = tr.querySelector('th'), td = tr.querySelector('td'); if (th && td) rows[th.textContent.trim().toLowerCase()] = td.textContent.trim(); });
    if (rows['artist'] && rows['album'] && !document.getElementById('rdb-mb-box') && document.querySelector('h1')?.textContent.includes('Album details')) {
      const box = el('div', { id: 'rdb-mb-box', style: 'margin:1em 0' }, el('strong', { text: 'MusicBrainz: ' }));
      if (rows['bar code']) box.append(mbLink(`${MB}/search?type=release&method=advanced&query=${encodeURIComponent('barcode:' + rows['bar code'].replace(/\s+/g, ''))}`, 'this barcode'), ' · ');
      if (rows['catalog number']) box.append(mbLink(`${MB}/search?type=release&method=advanced&query=${encodeURIComponent('catno:"' + rows['catalog number'] + '"')}`, 'this catalogue number'), ' · ');
      box.append(mbLink(mbSearch(rows['artist'], rows['album'].replace(/\s*\([^)]*\)\s*$/, '')), 'artist + album'));
      const h1 = document.querySelector('h1');
      h1.after(box);
    }
  }

  // upload page: fill the form from the #rdb=… link made by the MusicBrainz panel or by RustyDisc
  function onAddPage() {
    const m = location.hash.match(/^#rdb=(.+)$/);
    const form = document.getElementById('form');
    if (!m || !form) return;
    let data;
    try { data = JSON.parse(unb64(decodeURIComponent(m[1]))); } catch { return; }
    const set = (id, v) => { const e = document.getElementById(id); if (e && v != null && v !== '') { e.value = v; e.dispatchEvent(new Event('input', { bubbles: true })); e.dispatchEvent(new Event('change', { bubbles: true })); } };
    set('album_artist', data.artist); set('album_album', data.album); set('album_year', data.year);
    set('album_codec', data.codec); set('album_source', data.source);
    set('album_label', data.label); set('album_catalogNumber', data.catalogNumber); set('album_barCode', data.barCode);
    set('album_country', data.country); set('album_link', data.link); set('album_comment', data.comment);
    let attached = '';
    if (data.log) {
      try {
        const dt = new DataTransfer();
        dt.items.add(new File([data.log], 'dr.txt', { type: 'text/plain' }));
        const input = document.getElementById('album_log');
        input.files = dt.files;
        input.dispatchEvent(new Event('change', { bubbles: true }));
        attached = ' The log (dr.txt) is attached.';
      } catch { attached = ' Attach the DR log yourself (your browser did not allow it).'; }
    }
    const banner = el('div', { style: 'border:1px solid #c9a227;background:#fff8dc;color:#333;padding:.6em .9em;margin:0 0 1em;border-radius:4px' },
      el('strong', { text: 'Pre-filled by the Dynamic Range DB userscript. ' }),
      document.createTextNode('Check every field, and whether the log came from a meter you trust, before you submit.' + attached + (data.algorithm ? ' ' + data.algorithm : '')));
    form.before(banner);
  }

  // ── go ─────────────────────────────────────────────────────────────────────
  if (location.hostname.endsWith('musicbrainz.org')) {
    onMusicBrainz().catch(e => console.warn('[Dynamic Range DB userscript]', e));
  } else if (location.hostname === 'dr.loudness-war.info') {
    if (location.pathname.startsWith('/album/add')) onAddPage(); else onListOrView();
  }
})();
