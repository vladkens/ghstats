(() => {
  const DAY = 864e5;
  const dt = t => luxon.DateTime.fromMillis(t, { zone: 'utc' });
  const parse = s => Date.UTC(+s.slice(0, 4), +s.slice(5, 7) - 1, +s.slice(8, 10));
  const fd = t => dt(t).toFormat('LLL d');
  const fdy = t => dt(t).toFormat('LLL d, yyyy');
  const fm = t => `${dt(t).toFormat('LLL')} ’${dt(t).toFormat('yy')}`;
  const n = v => Math.round(v).toLocaleString('en-US');
  const compact = v =>
    v >= 1e6
      ? (v / 1e6).toFixed(1).replace(/\.0$/, '') + 'M'
      : v >= 1e3
        ? (v / 1e3).toFixed(v >= 1e4 ? 0 : 1).replace(/\.0$/, '') + 'K'
        : String(v);
  const css = v => getComputedStyle(document.documentElement).getPropertyValue(v).trim();

  // MARK: Buckets

  // a series is { start, days }: one array of values per day, or null for a day without data
  const periodRange = (series, period) => {
    const len = series.days.length;
    return { i0: period > 0 ? Math.max(0, len - period) : 0, i1: len - 1 };
  };

  const bucketKind = days => (days <= 92 ? 'day' : days <= 366 ? 'week' : 'month');

  const bucketKey = (t, kind) => {
    if (kind === 'day') return t;
    const d = new Date(t);
    if (kind === 'week') return t - ((d.getUTCDay() + 6) % 7) * DAY;
    return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), 1);
  };

  const bucketSize = (key, kind) => {
    if (kind === 'day') return 1;
    if (kind === 'week') return 7;
    const d = new Date(key);
    return new Date(Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 0)).getUTCDate();
  };

  const buckets = (series, period) => {
    const start = parse(series.start);
    const width = series.days.find(x => x)?.length ?? 0;
    const { i0, i1 } = periodRange(series, period);
    const kind = bucketKind(i1 - i0 + 1);
    const list = [];
    const map = new Map();

    for (let i = i0; i <= i1; i++) {
      const t = start + i * DAY;
      const key = bucketKey(t, kind);
      let b = map.get(key);
      if (!b) {
        b = { key, from: t, to: t, days: 0, have: 0, s: new Array(width).fill(0) };
        map.set(key, b);
        list.push(b);
      }

      b.to = t;
      b.days++;
      const v = series.days[i];
      if (v) {
        b.have++;
        for (let j = 0; j < width; j++) b.s[j] += v[j];
      }
    }

    // under half the days collected: drawn as a gap, not as a short bar
    for (const b of list) b.gap = b.have / b.days < 0.5;
    return { kind, list };
  };

  const bucketLabel = (b, kind) => (kind === 'month' ? fm(b.key) : fd(b.key));

  // GitHub's traffic for today and yesterday is still incomplete
  const recent = (t, today) => (t === today ? 'today' : t === today - DAY ? 'yesterday' : null);

  const bucketTitle = (b, kind, today) => {
    if (kind !== 'day') return `${fd(b.from)} – ${fdy(b.to)}`;
    const r = recent(b.from, today);
    return fdy(b.from) + (r ? ` (${r}, partial)` : '');
  };

  const bucketNotes = (b, kind, today) => {
    const out = [];
    if (b.days < bucketSize(b.key, kind)) out.push(`Partial ${kind}`);
    if (kind !== 'day' && b.to >= today - DAY) out.push('Last 2 days still incomplete');
    if (b.have < b.days) out.push(`${b.have} of ${b.days} days collected`);
    return out;
  };

  const gapRanges = list => {
    const out = [];
    let a = null;
    list.forEach((b, i) => {
      if (b.gap && a == null) a = i;
      if (!b.gap && a != null) {
        out.push([a, i - 1]);
        a = null;
      }
    });

    if (a != null) out.push([a, list.length - 1]);
    return out;
  };

  // MARK: Plugins

  const hatch = color => {
    const c = document.createElement('canvas');
    c.width = c.height = 6;
    const g = c.getContext('2d');
    g.strokeStyle = color;
    g.lineWidth = 1;
    g.beginPath();
    g.moveTo(0, 6);
    g.lineTo(6, 0);
    g.moveTo(-1, 1);
    g.lineTo(1, -1);
    g.moveTo(5, 7);
    g.lineTo(7, 5);
    g.stroke();
    return g.createPattern(c, 'repeat');
  };

  let theme = {};
  const readTheme = () => ({
    font: css('--font-ui'),
    rowbar: css('--rowbar'),
    muted: css('--muted'),
    lineStrong: css('--line-strong'),
    hatch: hatch(css('--gap-ink')),
  });

  const bandsPlugin = {
    id: 'bands',
    beforeDatasetsDraw(chart, _args, o) {
      const {
        ctx,
        chartArea: ca,
        scales: { x },
      } = chart;
      if (!o || !x) return;

      const half = o.category ? Math.abs(x.getPixelForValue(1) - x.getPixelForValue(0)) / 2 : 0;
      ctx.save();
      ctx.font = `500 11px ${theme.font}`;

      for (const w of o.windows || []) {
        const x0 = Math.max(ca.left, x.getPixelForValue(w.from));
        const x1 = Math.min(ca.right, x.getPixelForValue(w.to));
        ctx.fillStyle = theme.rowbar;
        ctx.fillRect(x0, ca.top, x1 - x0, ca.bottom - ca.top);
        ctx.fillStyle = theme.muted;
        ctx.textAlign = 'right';
        const inside = ctx.measureText(w.label).width + 16 < x1 - x0;
        ctx.fillText(w.label, inside ? x1 - 8 : x0 - 6, ca.bottom - 8);
      }

      for (const [a, b] of o.gaps || []) {
        const x0 = Math.max(ca.left, x.getPixelForValue(a) - half);
        const x1 = Math.min(ca.right, x.getPixelForValue(b) + half);
        if (x1 <= x0) continue;

        ctx.fillStyle = theme.hatch;
        ctx.fillRect(x0, ca.top, x1 - x0, ca.bottom - ca.top);
      }

      ctx.restore();
    },
  };

  const crosshair = {
    id: 'crosshair',
    afterDatasetsDraw(chart) {
      const active = chart.tooltip?.getActiveElements?.() || [];
      if (!active.length) return;

      const { ctx, chartArea: ca } = chart;
      const x = active[0].element.x;
      ctx.save();
      ctx.strokeStyle = theme.lineStrong;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(x, ca.top);
      ctx.lineTo(x, ca.bottom);
      ctx.stroke();
      ctx.restore();
    },
  };

  const baseOpts = () => {
    const muted = css('--muted');
    const font = css('--font-ui');
    const tick = { color: muted, font: { family: font, size: 11 } };
    return {
      responsive: true,
      maintainAspectRatio: false,
      animation: false,
      interaction: { mode: 'index', intersect: false },
      plugins: {
        legend: { display: false },
        tooltip: {
          backgroundColor: css('--card'),
          borderColor: css('--line-strong'),
          borderWidth: 1,
          titleColor: css('--head'),
          bodyColor: css('--ink'),
          footerColor: muted,
          titleFont: { family: font, weight: '600', size: 12 },
          bodyFont: { family: font, size: 12 },
          footerFont: { family: font, size: 11, weight: '400' },
          padding: 8,
          boxWidth: 9,
          boxHeight: 9,
          boxPadding: 4,
          cornerRadius: 4,
        },
      },
      scales: {
        x: {
          grid: { display: false },
          border: { color: css('--line-strong') },
          ticks: { ...tick, maxRotation: 0, autoSkipPadding: 14 },
        },
        y: {
          beginAtZero: true,
          grid: { color: css('--line') },
          border: { display: false },
          ticks: { ...tick, maxTicksLimit: 5, precision: 0, callback: v => compact(v) },
        },
      },
    };
  };

  // MARK: Charts

  let charts = [];

  // grouped: false draws unique inside total instead of next to it
  const trafficChart = (id, D, ti, ui) => {
    const el = document.getElementById(id);
    if (!el) return;

    const { kind, list } = buckets(D.daily, D.period);
    const today = parse(D.today);
    const o = baseOpts();
    o.plugins.bands = { category: true, gaps: gapRanges(list) };
    o.plugins.tooltip.itemSort = (a, b) => b.datasetIndex - a.datasetIndex;
    o.plugins.tooltip.callbacks = {
      title: it => bucketTitle(list[it[0].dataIndex], kind, today),
      label: it => ` ${it.dataset.label}: ${it.raw == null ? 'no data' : n(it.raw)}`,
      footer: it => bucketNotes(list[it[0].dataIndex], kind, today),
    };

    const bar = {
      grouped: false,
      borderRadius: { topLeft: 3, topRight: 3 },
      borderSkipped: 'start',
      maxBarThickness: 24,
      categoryPercentage: 0.86,
      barPercentage: 0.92,
    };

    const data = col => list.map(b => (b.gap ? null : col === ui ? Math.min(b.s[ui], b.s[ti]) : b.s[col]));
    charts.push(
      new Chart(el, {
        type: 'bar',
        data: {
          labels: list.map(b => bucketLabel(b, kind)),
          datasets: [
            { ...bar, label: 'Unique', data: data(ui), backgroundColor: css('--traffic'), order: 0 },
            { ...bar, label: 'Total', data: data(ti), backgroundColor: css('--traffic-tint'), order: 1 },
          ],
        },
        options: o,
        plugins: [bandsPlugin, crosshair],
      }),
    );

    const key = el.closest('section')?.querySelector('.gap-key');
    if (key) key.hidden = !list.some(b => b.gap);
  };

  // stacked by release: the top ones of the period, then the rest together
  const downloadsChart = (id, D) => {
    const el = document.getElementById(id);
    if (!el || !D.downloads) return;

    const { kind, list } = buckets(D.downloads, D.period);
    const today = parse(D.today);
    const o = baseOpts();
    o.scales.x.stacked = true;
    o.scales.y.stacked = true;
    o.plugins.bands = { category: true, gaps: gapRanges(list) };
    o.plugins.tooltip.filter = it => it.raw > 0;
    o.plugins.tooltip.callbacks = {
      title: it => bucketTitle(list[it[0].dataIndex], kind, today),
      label: it => ` ${it.dataset.label}: ${n(it.raw)}`,
      footer: it => [`Total: ${n(it.reduce((s, x) => s + x.raw, 0))}`, ...bucketNotes(list[it[0].dataIndex], kind, today)],
    };

    charts.push(
      new Chart(el, {
        type: 'bar',
        data: {
          labels: list.map(b => bucketLabel(b, kind)),
          datasets: D.downloads.labels.map((label, i) => ({
            label,
            data: list.map(b => (b.gap ? null : b.s[i])),
            backgroundColor: css(`--s${i + 1}`),
            maxBarThickness: 24,
            categoryPercentage: 0.86,
            barPercentage: 0.92,
          })),
        },
        options: o,
        plugins: [bandsPlugin, crosshair],
      }),
    );

    const key = el.closest('section')?.querySelector('.gap-key');
    if (key) key.hidden = !list.some(b => b.gap);
  };

  const monthTicks = (min, max, wide) => {
    if (max - min < 100 * DAY) {
      const out = [];
      const step = max - min < 40 * DAY ? 7 : 14;
      for (let t = max; t >= min; t -= step * DAY) out.unshift({ value: t });
      return out;
    }

    const months = (max - min) / (30 * DAY);
    const step = months > (wide ? 36 : 12) ? 6 : months > (wide ? 18 : 6) ? 3 : months > 8 ? 2 : 1;
    const out = [];
    const d = new Date(min);
    let t = Date.UTC(d.getUTCFullYear(), d.getUTCMonth() + 1, 1);
    while (t <= max) {
      const m = new Date(t).getUTCMonth();
      if (m % step === 0) out.push({ value: t });
      t = Date.UTC(new Date(t).getUTCFullYear(), m + 1, 1);
    }

    return out;
  };

  const starsPoints = D => {
    const gaps = D.gaps.map(([a, b]) => [parse(a), parse(b)]);
    const pts = [];
    for (const [d, s] of D.stars) {
      const x = parse(d);
      const last = pts.at(-1);
      const gap = last && gaps.find(([a, b]) => x >= b && last.x <= a);
      if (gap) {
        // so a lone point before a gap still draws a line
        if (last.x < gap[0]) pts.push({ x: gap[0], y: last.y });
        pts.push({ x: gap[0] + DAY, y: null });
      }

      pts.push({ x, y: s });
    }

    // a stale value ends the line; the server sends the gap after it
    const today = parse(D.today);
    const last = pts.at(-1);
    if (last && last.x < today && today - last.x <= 3 * DAY) pts.push({ x: today, y: last.y });
    return { pts, gaps };
  };

  const starsChart = (id, D) => {
    const el = document.getElementById(id);
    if (!el || !D.stars.length) return;

    const { pts, gaps } = starsPoints(D);
    const today = parse(D.today);
    const min = pts[0].x;
    const max = Math.max(today, min + DAY);
    const last = pts.length - 1;
    const wide = el.parentElement.clientWidth > 600;
    const from = D.period > 0 ? today - D.period * DAY : null;

    const o = baseOpts();
    o.scales.x = {
      ...o.scales.x,
      type: 'time',
      adapters: { date: { zone: 'utc' } },
      min,
      max,
      afterBuildTicks: s => {
        s.ticks = monthTicks(min, max, wide);
      },
      ticks: { ...o.scales.x.ticks, autoSkip: false, callback: v => (max - min < 100 * DAY ? fd(v) : fm(v)) },
    };

    const windows = from != null && from > min ? [{ from, to: today, label: D.window_label }] : [];
    o.plugins.bands = { gaps, windows };
    o.plugins.tooltip.callbacks = { title: it => fdy(it[0].raw.x), label: it => ` Stars: ${n(it.raw.y)}` };
    o.plugins.tooltip.filter = it => it.raw.y != null;

    const c = css('--stars');
    charts.push(
      new Chart(el, {
        type: 'line',
        data: {
          datasets: [
            {
              data: pts,
              parsing: false,
              spanGaps: false,
              borderColor: c,
              borderWidth: 2,
              tension: 0,
              fill: 'origin',
              backgroundColor: c + '22',
              pointRadius: ctx => (ctx.dataIndex === last ? 4 : 0),
              pointHoverRadius: 4,
              pointBackgroundColor: c,
              pointBorderColor: css('--card'),
              pointBorderWidth: 2,
            },
          ],
        },
        options: o,
        plugins: [bandsPlugin, crosshair],
      }),
    );
  };

  let current = null;

  const render = () => {
    charts.forEach(x => x.destroy());
    charts = [];
    if (!current) return;

    theme = readTheme();
    Chart.defaults.font.family = theme.font;
    if (current.daily.start) {
      trafficChart('chart_views', current, 2, 3);
      trafficChart('chart_clones', current, 0, 1);
    }

    starsChart('chart_stars', current);
    downloadsChart('chart_downloads', current);
  };

  window.renderRepo = data => {
    current = data;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', render, { once: true });
    else render();
  };

  // charts read colors from CSS: redraw on theme change
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', render);

  // an emptied search box leaves no "q=" in the URL
  document.addEventListener('htmx:configRequest', e => {
    const params = e.detail.parameters;
    for (const k of Object.keys(params)) if (params[k] === '') delete params[k];
  });

  // keyup, not search: htmx's "changed" would skip retyping the text that was cleared
  document.addEventListener('click', e => {
    const input = e.target.closest('[data-clear]')?.previousElementSibling;
    if (!input) return;

    input.value = '';
    input.focus();
    input.dispatchEvent(new Event('keyup'));
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });

  // MARK: Tables

  // tables opened with "Show all" stay open across htmx sort swaps
  const expanded = new Set();

  const showAll = (card, focus) => {
    const rows = [...card.querySelectorAll('tr[hidden]')];
    rows.forEach(x => {
      x.hidden = false;
    });
    card.querySelector('[data-show-all]')?.remove();

    if (focus && rows.length) {
      rows[0].tabIndex = -1;
      rows[0].focus();
    }
  };

  document.addEventListener('click', e => {
    const card = e.target.closest('[data-show-all]')?.closest('.card');
    if (!card) return;

    expanded.add(card.id);
    showAll(card, true);
  });

  document.addEventListener('htmx:load', e => {
    const root = e.detail.elt;
    for (const card of [root, ...root.querySelectorAll('.card')]) {
      if (card.id && expanded.has(card.id)) showAll(card);
    }
  });

  // MARK: Card builder

  window.cardBuilder = () => {
    const form = document.getElementById('card_form');
    const f = form.elements;
    const history = () => f.card.value === 'history';
    const defaultPeriod = () => (history() ? 'tracked' : '30d');
    let periodTouched = false;

    const names = form.dataset.repos.split(' ').filter(Boolean);
    const isRepo = x => names.includes(x);

    // through toISOString, as Date alone rolls 2023-02-30 over to March
    const isDate = x => {
      const date = new Date(x);
      return !isNaN(date) && date.toISOString().slice(0, 10) === x;
    };

    // the input's value for the URL: a wrong one is left out and marked
    const valid = (input, ok) => {
      const good = ok(input.value);
      input.setAttribute('aria-invalid', !good && input.value !== '');
      return good ? input.value : '';
    };

    const update = () => {
      const scope = history() ? f.scope.value : 'all';
      const compare = scope === 'compare';
      const since = history() ? valid(f.since, isDate) : '';

      const shown = {
        scope: history(),
        repo: scope !== 'all',
        compare,
        since: history(),
        period: !since,
        log: history(),
        align: compare,
      };

      form.querySelectorAll('[data-field]').forEach(x => {
        if (x.dataset.field in shown) x.hidden = !shown[x.dataset.field];
      });

      // only what differs from the card's defaults, so the embed stays short
      const query = [];
      const picks = [...f.repo].map(x => valid(x, isRepo));
      const repos = [...new Set(picks)].filter(Boolean);
      if (compare && repos.length) query.push(`repos=${repos.join(',')}`);
      if (since) query.push(`since=${since}`);
      else if (f.period.value !== defaultPeriod()) query.push(`period=${f.period.value}`);
      if (f.theme.value !== 'auto') query.push(`theme=${f.theme.value}`);
      if (history() && f.log.checked) query.push('log=1');
      if (compare && f.align.checked) query.push('align=1');

      const path = scope === 'one' && picks[0] ? `${picks[0]}/history` : f.card.value;
      const url = `${location.origin}/card/${path}.svg${query.length ? '?' : ''}${query.join('&')}`;
      const alt = form.querySelector('[name=card]:checked').labels[0].textContent;
      // 800 is the cards' own width, the embed needs none
      const width = f.width.value === '800' ? '' : f.width.value;
      const tag = `<img src="${url}"${width && ` width="${width}"`} alt="${alt}">`;

      const img = document.getElementById('card_preview');
      img.src = url;
      img.alt = alt;
      img.style.width = width && `${width}px`;

      document.getElementById('embed_md').textContent = width ? tag : `![${alt}](${url})`;
      document.getElementById('embed_note').hidden = !width;
      document.getElementById('embed_html').textContent = tag;

      form.querySelectorAll('[data-width]').forEach(x => {
        x.setAttribute('aria-pressed', x.dataset.width === f.width.value);
      });
    };

    // the repo inputs share one dropdown: it moves under the input in use
    const list = document.getElementById('repo_list');
    let cur;
    let active = -1;

    const mark = i => {
      list.children[active]?.removeAttribute('aria-selected');
      const row = list.children[i];
      row?.setAttribute('aria-selected', true);
      row?.scrollIntoView({ block: 'nearest' });
      cur.setAttribute('aria-activedescendant', row?.id ?? '');
      active = i;
    };

    const option = (name, i) => {
      const x = document.createElement('li');
      x.id = `repo_row_${i}`;
      x.setAttribute('role', 'option');
      x.textContent = name;
      return x;
    };

    const show = (input, query) => {
      cur = input;
      mark(-1);

      const q = query.toLowerCase();
      list.replaceChildren(...names.filter(x => x.toLowerCase().includes(q)).map(option));
      input.parentNode.append(list);
      list.hidden = !list.children.length;
      input.setAttribute('aria-expanded', !list.hidden);
    };

    const close = () => {
      mark(-1);
      list.hidden = true;
      cur.setAttribute('aria-expanded', false);
    };

    const pick = x => {
      cur.value = x.textContent;
      close();
      update();
    };

    for (const input of f.repo) {
      // a finished pick is no filter: the list offers the other repos too
      const open = () => show(input, isRepo(input.value) ? '' : input.value);

      input.addEventListener('focus', open);
      input.addEventListener('click', open);
      input.addEventListener('input', () => show(input, input.value));
      input.addEventListener('blur', close);

      input.addEventListener('keydown', e => {
        if (e.key === 'Escape') close();
        if (e.key === 'Enter' && active >= 0) pick(list.children[active]);
        if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;

        e.preventDefault();
        if (list.hidden) return open();

        const n = list.children.length;
        const next = e.key === 'ArrowDown' ? active + 1 : (active < 0 ? n : active) - 1;
        mark((next + n) % n);
      });
    }

    // the input keeps the focus, so only a click elsewhere blurs it and closes the list
    list.addEventListener('mousedown', e => e.preventDefault());

    // prevented as the list sits in a label, which would pass the click on to the input
    list.addEventListener('click', e => {
      e.preventDefault();
      const x = e.target.closest('li');
      if (x) pick(x);
    });

    // click, not input: picking the period that is already on is a choice too, widths are buttons
    form.addEventListener('click', e => {
      if (e.target.name === 'period') periodTouched = true;

      if (e.target.dataset.width) {
        f.width.value = e.target.dataset.width;
        update();
      }
    });

    form.addEventListener('input', e => {
      if (e.target.name === 'card' && !periodTouched) f.period.value = defaultPeriod();
      update();
    });

    form.addEventListener('submit', e => e.preventDefault());
    update();
  };

  // execCommand and not navigator.clipboard: that one is https only, ghstats often runs on http
  document.addEventListener('click', e => {
    const btn = e.target.closest('[data-copy]');
    if (!btn) return;

    getSelection().selectAllChildren(btn.closest('.card').querySelector('pre'));
    document.execCommand('copy');
    btn.textContent = 'Copied';
    setTimeout(() => {
      btn.textContent = 'Copy';
    }, 1500);
  });
})();
