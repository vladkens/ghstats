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

  // daily rows: [clones, clones_uniques, views, views_uniques] or null for a day without data
  const periodRange = (D, period) => {
    const len = D.daily.days.length;
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

  const buckets = D => {
    const start = parse(D.daily.start);
    const { i0, i1 } = periodRange(D, D.period);
    const kind = bucketKind(i1 - i0 + 1);
    const list = [];
    const map = new Map();

    for (let i = i0; i <= i1; i++) {
      const t = start + i * DAY;
      const key = bucketKey(t, kind);
      let b = map.get(key);
      if (!b) {
        b = { key, from: t, to: t, days: 0, have: 0, s: [0, 0, 0, 0] };
        map.set(key, b);
        list.push(b);
      }

      b.to = t;
      b.days++;
      const v = D.daily.days[i];
      if (v) {
        b.have++;
        for (let j = 0; j < 4; j++) b.s[j] += v[j];
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

  // colors and the hatch pattern for the plugins, read once per render (and on theme change)
  let theme = {};
  const readTheme = () => ({
    font: css('--font-ui'),
    rowbar: css('--rowbar'),
    muted: css('--muted'),
    card: css('--card'),
    lineStrong: css('--line-strong'),
    hatch: hatch(css('--gap-ink')),
  });

  // overlays: outages (hatched) and the selected period on the stars chart (wash)
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
        if (x1 - x0 > 46) {
          ctx.textAlign = 'center';
          const tw = ctx.measureText('no data').width;
          ctx.fillStyle = theme.card;
          ctx.fillRect((x0 + x1) / 2 - tw / 2 - 5, ca.top + 2, tw + 10, 15);
          ctx.fillStyle = theme.muted;
          ctx.fillText('no data', (x0 + x1) / 2, ca.top + 13);
        }
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
          ticks: { ...tick, maxTicksLimit: 5, callback: v => compact(v) },
        },
      },
    };
  };

  // MARK: Charts

  let charts = [];

  // unique is drawn inside total (overlapping bars), not stacked on top of it
  const trafficChart = (id, D, ti, ui) => {
    const el = document.getElementById(id);
    if (!el) return;

    const { kind, list } = buckets(D);
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

  // stars come from real rows only (get_star_rows): the line breaks across outages
  const starsPoints = D => {
    const gaps = D.gaps.map(([a, b]) => [parse(a), parse(b)]);
    const pts = [];
    for (const [d, s] of D.stars) {
      const x = parse(d);
      const last = pts.at(-1);
      const gap = last && gaps.find(([a, b]) => x >= b && last.x <= a);
      if (gap) {
        // hold the last value to the gap start, so a lone point before it still draws a line
        if (last.x < gap[0]) pts.push({ x: gap[0], y: last.y });
        pts.push({ x: gap[0] + DAY, y: null });
      }

      pts.push({ x, y: s });
    }

    // carry a fresh value to today; a stale one ends the line, the server sends the gap after it
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
              backgroundColor: c + '1a',
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
  };

  window.renderRepo = data => {
    current = data;
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', render, { once: true });
    else render();
  };

  // charts read colors from CSS tokens: redraw when the theme changes
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', render);

  // MARK: Tables

  // "Show all" reveals the rows past the first ten, and keeps them open across htmx sort swaps
  const expanded = new Set();

  const showAll = (card, focus) => {
    const rows = [...card.querySelectorAll('tr[hidden]')];
    rows.forEach(x => {
      x.hidden = false;
    });
    card.querySelector('[data-show-all]')?.remove();

    // the button is gone: keep the keyboard on the first revealed row
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
})();
